#![allow(clippy::large_futures)]

//! Integration tests for the RPC server.
//!
//! These tests exercise the full roundtrip: write JSONL requests to a stream,
//! read JSONL responses/notifications back. Uses `tokio::io::duplex` for
//! in-process paired channels.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{BlobStore, Config, ConfigRuntime, MemoryConfigStore, StopReason};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_store::MemoryBlobStore;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ---------------------------------------------------------------------------
// Mock LLM client
// ---------------------------------------------------------------------------

fn provider_for_successful_rpc_test_model(model: &str) -> meerkat_core::Provider {
    if model.starts_with("claude-") {
        meerkat_core::Provider::Anthropic
    } else if model.starts_with("gpt-") || model.starts_with("o1-") {
        meerkat_core::Provider::OpenAI
    } else if model.starts_with("gemini-") {
        meerkat_core::Provider::Gemini
    } else {
        meerkat_core::Provider::Other
    }
}

/// Mock LLM client that echoes the model name from each request.
struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        let model = request.model.clone();
        Box::pin(stream::iter(vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: format!("Hello from mock [model={model}]"),
                meta: None,
            }),
            Ok(meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    provider_for_successful_rpc_test_model(&request.model),
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingToolClient {
    seen_tools: Mutex<Vec<Vec<String>>>,
}

impl RecordingToolClient {
    fn seen_tools(&self) -> Vec<Vec<String>> {
        self.seen_tools.lock().expect("recording lock").clone()
    }
}

#[async_trait]
impl LlmClient for RecordingToolClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        self.seen_tools.lock().expect("recording lock").push(
            request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
        );
        Box::pin(stream::iter(vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: "tools recorded".to_string(),
                meta: None,
            }),
            Ok(meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    provider_for_successful_rpc_test_model(&request.model),
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
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
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blob_store,
        )
        .expect("construct runtime authority"),
        meerkat_rpc::router::NotificationSink::noop(),
    )
    .expect("compose the RPC session runtime");
    let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
        Config::default(),
        meerkat_models::canonical(),
    ));
    runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
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
        let mut server = RpcServer::new(reader, server_writer, runtime, config_store)
            .expect("construct runtime authority");
        server.run().await
    });

    let client_reader = BufReader::new(client_reader);
    (client_writer, client_reader, server_handle)
}

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
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blob_store,
        )
        .expect("construct runtime authority"),
        meerkat_rpc::router::NotificationSink::noop(),
    )
    .expect("compose the RPC session runtime");
    let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
        Config::default(),
        meerkat_models::canonical(),
    ));
    runtime.set_default_llm_client(Some(client));
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
        let mut server = RpcServer::new(reader, server_writer, runtime, config_store)
            .expect("construct runtime authority");
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
        "Expected success, got error: {response}"
    );
    assert_eq!(response["result"]["server_info"]["name"], "meerkat-rpc");
    assert!(response["result"]["server_info"]["version"].is_string());

    let methods = response["result"]["methods"].as_array().unwrap();
    let method_names: Vec<&str> = methods.iter().map(|m| m.as_str().unwrap()).collect();
    assert!(method_names.contains(&"session/create"));
    assert!(method_names.contains(&"session/external_event"));
    assert!(method_names.contains(&"session/peer_response_terminal"));
    assert!(method_names.contains(&"session/inject_context"));
    assert!(method_names.contains(&"turn/start"));
    assert!(method_names.contains(&"config/get"));
    #[cfg(feature = "mcp")]
    {
        assert!(method_names.contains(&"mcp/add"));
        assert!(method_names.contains(&"mcp/remove"));
        assert!(method_names.contains(&"mcp/reload"));
    }
    #[cfg(not(feature = "mcp"))]
    {
        assert!(!method_names.contains(&"mcp/add"));
        assert!(!method_names.contains(&"mcp/remove"));
        assert!(!method_names.contains(&"mcp/reload"));
    }
    #[cfg(feature = "mob")]
    {
        assert!(method_names.contains(&"mob/spawn_helper"));
        assert!(method_names.contains(&"mob/fork_helper"));
        assert!(method_names.contains(&"mob/force_cancel"));
        assert!(method_names.contains(&"mob/member_status"));
        assert!(method_names.contains(&"mob/ingress_interaction"));
    }

    // Close to trigger EOF
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn deferred_callback_direct_sessions_expose_control_plane_tools_on_first_turn() {
    let client = Arc::new(RecordingToolClient::default());
    let (mut writer, mut reader, server_handle) = spawn_test_server_with_client(client.clone());

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    )
    .await;
    let init_resp = read_response(&mut reader).await;
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {init_resp}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/register",
            "params": {
                "tools": [{
                    "name": "secret_lookup",
                    "description": "Look up a secret value through a deferred catalog.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"}
                        },
                        "required": ["key"]
                    }
                }, {
                    "name": "secret_audit",
                    "description": "Audit a secret value through the same deferred catalog.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"}
                        },
                        "required": ["key"]
                    }
                }]
            }
        }),
    )
    .await;
    let register_resp = read_response(&mut reader).await;
    assert!(
        register_resp["error"].is_null(),
        "tools/register failed: {register_resp}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/create",
            "params": {
                "prompt": "Bootstrap deferred session",
                "initial_turn": "deferred",
                "enable_builtins": false,
                "enable_shell": false,
                "enable_memory": false,
                "enable_mob": false
            }
        }),
    )
    .await;
    let create_resp = read_response(&mut reader).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Inspect the deferred control plane."
            }
        }),
    )
    .await;
    let turn_resp = read_response(&mut reader).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );

    let seen = client.seen_tools();
    assert_eq!(seen.len(), 1, "expected exactly one LLM call, got {seen:?}");
    let first_call = &seen[0];
    assert!(
        first_call.iter().any(|name| name == "tool_catalog_search"),
        "direct deferred sessions should expose tool_catalog_search, got {first_call:?}"
    );
    assert!(
        first_call.iter().any(|name| name == "tool_catalog_load"),
        "direct deferred sessions should expose tool_catalog_load, got {first_call:?}"
    );
    assert!(
        !first_call.iter().any(|name| name == "secret_lookup"),
        "deferred tools should remain hidden before load, got {first_call:?}"
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn deferred_inline_external_tools_accept_explicit_keep_alive_false_turn() {
    let client = Arc::new(RecordingToolClient::default());
    let (mut writer, mut reader, server_handle) = spawn_test_server_with_client(client.clone());

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    )
    .await;
    let init_resp = read_response(&mut reader).await;
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {init_resp}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/create",
            "params": {
                "prompt": "Bootstrap deferred session",
                "initial_turn": "deferred",
                "enable_builtins": false,
                "enable_shell": false,
                "enable_memory": false,
                "enable_mob": false,
                "keep_alive": false,
                "external_tools": [{
                    "name": "linear_graphql",
                    "description": "Execute GraphQL",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"},
                            "variables": {"type": "object"}
                        },
                        "required": ["query"],
                        "additionalProperties": false
                    }
                }]
            }
        }),
    )
    .await;
    let create_resp = read_response(&mut reader).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Inspect the deferred inline callback tool surface.",
                "keep_alive": false
            }
        }),
    )
    .await;
    let turn_resp = read_response(&mut reader).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );

    let seen = client.seen_tools();
    assert_eq!(seen.len(), 1, "expected exactly one LLM call, got {seen:?}");

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn pipelined_tools_register_commits_before_session_create() {
    let client = Arc::new(RecordingToolClient::default());
    let (mut writer, mut reader, server_handle) = spawn_test_server_with_client(client.clone());

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    )
    .await;
    let init_resp = read_response(&mut reader).await;
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {init_resp}"
    );

    let register_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/register",
        "params": {
            "tools": [{
                "name": "pipelined_secret_lookup",
                "description": "Look up a secret value registered immediately before create.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"}
                    },
                    "required": ["key"]
                }
            }]
        }
    });
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/create",
        "params": {
            "prompt": "Start after a pipelined callback tool registration.",
            "enable_builtins": false,
            "enable_shell": false,
            "enable_memory": false,
            "enable_mob": false
        }
    });
    let pipelined = format!(
        "{}\n{}\n",
        serde_json::to_string(&register_req).unwrap(),
        serde_json::to_string(&create_req).unwrap()
    );
    writer.write_all(pipelined.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();

    let register_resp = read_response(&mut reader).await;
    assert_eq!(
        register_resp["id"], 2,
        "tools/register must complete before the following pipelined create response: {register_resp}"
    );
    assert!(
        register_resp["error"].is_null(),
        "tools/register failed: {register_resp}"
    );

    let create_resp = read_response(&mut reader).await;
    assert_eq!(create_resp["id"], 3);
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );

    let seen = client.seen_tools();
    assert_eq!(
        seen.len(),
        1,
        "expected exactly one LLM call from session/create, got {seen:?}"
    );
    assert!(
        seen[0].iter().any(|name| name == "pipelined_secret_lookup"),
        "session/create first turn must see the immediately preceding tools/register, got {:?}",
        seen[0]
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn late_registered_deferred_callbacks_keep_control_plane_after_inline_build() {
    let client = Arc::new(RecordingToolClient::default());
    let (mut writer, mut reader, server_handle) = spawn_test_server_with_client(client.clone());

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    )
    .await;
    let init_resp = read_response(&mut reader).await;
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {init_resp}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/register",
            "params": {
                "tools": [{
                    "name": "secret_lookup",
                    "description": "Look up a secret value through a deferred catalog.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"}
                        },
                        "required": ["key"]
                    }
                }]
            }
        }),
    )
    .await;
    let register_resp = read_response(&mut reader).await;
    assert!(
        register_resp["error"].is_null(),
        "initial tools/register failed: {register_resp}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/create",
            "params": {
                "prompt": "Bootstrap late deferred session",
                "initial_turn": "deferred",
                "enable_builtins": false,
                "enable_shell": false,
                "enable_memory": false,
                "enable_mob": false
            }
        }),
    )
    .await;
    let create_resp = read_response(&mut reader).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Inspect the current inline callback tool surface."
            }
        }),
    )
    .await;
    let first_turn_resp = read_response(&mut reader).await;
    assert!(
        first_turn_resp["error"].is_null(),
        "initial turn/start failed: {first_turn_resp}"
    );

    let seen = client.seen_tools();
    assert_eq!(
        seen.len(),
        1,
        "expected exactly one initial LLM call, got {seen:?}"
    );
    let initial_call = &seen[0];
    assert!(
        initial_call.iter().any(|name| name == "secret_lookup"),
        "the session should start inline before the adaptive deferred threshold is crossed, got {initial_call:?}"
    );
    assert!(
        initial_call
            .iter()
            .any(|name| name == "tool_catalog_search"),
        "dynamically defer-capable sessions should still precompose tool_catalog_search, got {initial_call:?}"
    );
    assert!(
        initial_call.iter().any(|name| name == "tool_catalog_load"),
        "dynamically defer-capable sessions should still precompose tool_catalog_load, got {initial_call:?}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/register",
            "params": {
                "tools": [{
                    "name": "secret_audit",
                    "description": "Audit a secret value through the same deferred catalog.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"}
                        },
                        "required": ["key"]
                    }
                }]
            }
        }),
    )
    .await;
    let late_register_resp = read_response(&mut reader).await;
    assert!(
        late_register_resp["error"].is_null(),
        "late tools/register failed: {late_register_resp}"
    );

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Inspect the deferred control plane after a late registration."
            }
        }),
    )
    .await;
    let turn_resp = read_response(&mut reader).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );

    let seen = client.seen_tools();
    assert_eq!(
        seen.len(),
        2,
        "expected exactly two LLM calls, got {seen:?}"
    );
    let first_call = &seen[1];
    assert!(
        first_call.iter().any(|name| name == "tool_catalog_search"),
        "late-switch deferred sessions should expose tool_catalog_search, got {first_call:?}"
    );
    assert!(
        first_call.iter().any(|name| name == "tool_catalog_load"),
        "late-switch deferred sessions should expose tool_catalog_load, got {first_call:?}"
    );
    assert!(
        !first_call.iter().any(|name| name == "secret_audit"),
        "late-added deferred tools should remain hidden until loaded, got {first_call:?}"
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// mcp/* methods are registered and return contract-typed placeholder responses.
#[cfg(feature = "mcp")]
#[tokio::test]
async fn mcp_live_methods_roundtrip_and_validation() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // Create session for validation against an existing session ID
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/create",
        "params": {"prompt": "hello"}
    });
    send_request(&mut writer, &create_req).await;
    let create_resp = read_response(&mut reader).await;
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    // mcp/add success
    let add_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "mcp/add",
        "params": {
            "session_id": session_id,
            "server_config": {
                "name": "filesystem",
                "command": "echo",
                "args": [],
                "env": {}
            },
            "persisted": false
        }
    });
    send_request(&mut writer, &add_req).await;
    let add_resp = read_response(&mut reader).await;
    assert!(add_resp["error"].is_null(), "mcp/add failed: {add_resp}");
    assert_eq!(add_resp["result"]["operation"], "add");
    assert_eq!(add_resp["result"]["status"], "staged");
    assert_eq!(add_resp["result"]["persisted"], false);
    assert!(add_resp["result"]["applied_at_turn"].is_null());

    // mcp/reload success (null server_name = reload all)
    let reload_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "mcp/reload",
        "params": {
            "session_id": add_resp["result"]["session_id"],
            "persisted": false
        }
    });
    send_request(&mut writer, &reload_req).await;
    let reload_resp = read_response(&mut reader).await;
    assert!(
        reload_resp["error"].is_null(),
        "mcp/reload failed: {reload_resp}"
    );
    assert_eq!(reload_resp["result"]["operation"], "reload");
    assert_eq!(reload_resp["result"]["status"], "staged");

    // mcp/remove success (persisted=true is accepted but response is persisted=false)
    let remove_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "mcp/remove",
        "params": {
            "session_id": reload_resp["result"]["session_id"],
            "server_name": "filesystem",
            "persisted": true
        }
    });
    send_request(&mut writer, &remove_req).await;
    let remove_resp = read_response(&mut reader).await;
    assert!(
        remove_resp["error"].is_null(),
        "mcp/remove failed: {remove_resp}"
    );
    assert_eq!(remove_resp["result"]["operation"], "remove");
    assert_eq!(remove_resp["result"]["status"], "staged");
    assert_eq!(remove_resp["result"]["persisted"], false);

    // invalid params are rejected
    let invalid_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "mcp/add",
        "params": {
            "session_id": "not-a-session-id",
            "server_config": {}
        }
    });
    send_request(&mut writer, &invalid_req).await;
    let invalid_resp = read_response(&mut reader).await;
    assert!(invalid_resp["result"].is_null());
    assert!(invalid_resp["error"].is_object());

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
        "session/create failed: {create_resp}"
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
        "turn/start failed: {turn_resp}"
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
        "Expected error response, got: {error_resp}"
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
        "Expected success after recovery, got: {response}"
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
    assert!(get_resp["error"].is_null(), "config/get failed: {get_resp}");
    let initial_max_tokens = get_resp["result"]["config"]["max_tokens"]
        .as_u64()
        .unwrap_or(8192); // optional: absent when unset (None)

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
        "config/patch failed: {patch_resp}"
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
    assert!(resp1["error"].is_null(), "First create failed: {resp1}");
    let sid1 = resp1["result"]["session_id"].as_str().unwrap().to_string();

    let create2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/create",
        "params": {"prompt": "Second"}
    });
    send_request(&mut writer, &create2).await;
    let resp2 = read_response(&mut reader).await;
    assert!(resp2["error"].is_null(), "Second create failed: {resp2}");
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
        "session/list failed: {list_resp}"
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

/// Regression: mcp/add with persisted=true should NOT be rejected — the field
/// is accepted as a real persistence request. This fake session never reaches
/// config mutation because session validation fails first.
/// Without the `mcp` feature the method returns METHOD_NOT_FOUND.
#[tokio::test]
async fn test_mcp_add_persisted_true_not_rejected() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &init).await;
    let _init_resp = read_response(&mut reader).await;

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "mcp/add",
        "params": {
            "session_id": "00000000-0000-0000-0000-000000000001",
            "server_config": {"name": "test-server", "command": "echo", "args": [], "env": {}},
            "persisted": true
        }
    });
    send_request(&mut writer, &req).await;

    let response = read_response(&mut reader).await;
    assert_eq!(response["id"], 1);
    // Without `mcp` feature: METHOD_NOT_FOUND. With `mcp` feature: session not found
    // (the fake session_id doesn't exist). Either way it's an error, but NOT because
    // of persisted=true.
    assert!(
        response["error"].is_object(),
        "expected error (session not found or method not found): {response}"
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// Regression: mcp/add staged response reports persisted: false when the caller
/// requested a live-only mutation.
#[cfg(feature = "mcp")]
#[tokio::test]
async fn test_mcp_add_staged_response_has_persisted_false() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &init).await;
    let _init_resp = read_response(&mut reader).await;

    let create = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/create",
        "params": { "prompt": "hello" }
    });
    send_request(&mut writer, &create).await;
    let mut session_id = None;
    loop {
        let msg = read_response(&mut reader).await;
        if msg.get("id") == Some(&serde_json::json!(1)) {
            if let Some(result) = msg.get("result") {
                session_id = result
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            break;
        }
    }
    let session_id = session_id.expect("session_id from create");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "mcp/add",
        "params": {
            "session_id": session_id,
            "server_config": {"name": "test-server", "command": "echo", "args": [], "env": {}},
            "persisted": false
        }
    });
    send_request(&mut writer, &req).await;

    let response = read_response(&mut reader).await;
    assert_eq!(response["id"], 2);
    let result = &response["result"];
    assert_eq!(result["status"], "staged");
    assert_eq!(result["persisted"], false);

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// 11. In-session model switching via turn/start with model override.
///
/// Create a session -> run first turn -> verify default model ->
/// run second turn with model override -> verify model name changed.
#[tokio::test]
async fn in_session_model_switch_via_turn_start() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // 1. session/create with initial prompt (materializes the session)
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/create",
        "params": {"prompt": "Hello"}
    });
    send_request(&mut writer, &create_req).await;
    let create_resp = read_response(&mut reader).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_text = create_resp["result"]["text"].as_str().unwrap();
    assert!(
        first_text.contains("Hello from mock"),
        "First turn should produce mock text, got: {first_text}"
    );
    // Default model is the catalog-owned global default; verify it.
    let expected_default = format!("model={}", meerkat_models::global_default_model());
    assert!(
        first_text.contains(&expected_default),
        "First turn should use the catalog global default model, got: {first_text}"
    );

    // 2. turn/start with model override on the materialized session.
    // Model-only overrides may switch providers between turns when the
    // target model has catalog ownership.
    let turn_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "turn/start",
        "params": {
            "session_id": session_id,
            "prompt": "Follow up with new model",
            "model": "claude-sonnet-4-5"
        }
    });
    send_request(&mut writer, &turn_req).await;
    let turn_resp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_response(&mut reader),
    )
    .await
    .expect("runtime-backed model override must not deadlock on the turn-finalization boundary");
    assert!(
        turn_resp["error"].is_null(),
        "turn/start with model override failed: {turn_resp}"
    );
    let switched_text = turn_resp["result"]["text"].as_str().unwrap();
    assert!(
        switched_text.contains("model=claude-sonnet-4-5"),
        "After model switch, response should use claude-sonnet-4-5, got: {switched_text}"
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// Issue 1451: actual TCP callback ownership regressions.
mod tcp_callback_ownership {
    //! Issue 1451: real TCP connections must not share callback ownership.
    //! The provider is scripted; RPC, session, Agent, dispatcher and TCP owners are real.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::large_futures
    )]

    use async_trait::async_trait;
    use futures::FutureExt;
    use meerkat::AgentFactory;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{
        BlobStore, Config, ConfigRuntime, ConfigStore, MemoryConfigStore, Message, StopReason,
        ToolResult,
    };
    use meerkat_rpc::{
        server::{ServerError, serve_tcp_connection},
        session_runtime::SessionRuntime,
    };
    use serde_json::{Value, json};
    use std::{
        collections::{HashMap, VecDeque},
        panic::AssertUnwindSafe,
        pin::Pin,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::{TcpListener, TcpStream, tcp::OwnedWriteHalf},
        sync::{Semaphore, mpsc},
        task::JoinHandle,
    };

    const LIMIT: Duration = Duration::from_secs(10);

    #[derive(Clone)]
    enum Step {
        Load {
            tool: &'static str,
            call_id: &'static str,
        },
        Call {
            tool: &'static str,
            call_id: &'static str,
        },
        Finish {
            gate: Option<Arc<Semaphore>>,
        },
    }
    /// The deferred create prompt the harness sends, as the runtime merges it
    /// into the first turn's user message.
    const DEFERRED_CREATE_PROMPT_PREFIX: &str = "deferred bootstrap\n\n";
    #[derive(Debug)]
    struct Seen {
        /// The exact last user message of the model request.
        raw_prompt: String,
        /// The plan key it matched (`raw_prompt` minus the exact deferred
        /// create prefix, when present).
        prompt: String,
        tools: Vec<String>,
        results: Vec<ToolResult>,
    }
    struct ScriptedProvider {
        plans: Mutex<HashMap<String, VecDeque<Step>>>,
        seen: mpsc::UnboundedSender<Seen>,
    }
    impl ScriptedProvider {
        fn plan(&self, prompt: &str, steps: impl IntoIterator<Item = Step>) {
            assert!(
                self.plans
                    .lock()
                    .unwrap()
                    .insert(prompt.into(), steps.into_iter().collect())
                    .is_none()
            );
        }
    }
    #[async_trait]
    impl LlmClient for ScriptedProvider {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }
        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            let prompt = request
                .messages
                .iter()
                .rev()
                .find_map(|m| match m {
                    Message::User(user) => Some(user.text_content()),
                    _ => None,
                })
                .expect("real user prompt");
            // A deferred session/create merges its exact create prompt into
            // the first turn's user message. Strip exactly that prefix (and
            // nothing else) to recover the planned turn key; any other prompt
            // change stays visible as an unplanned request.
            let raw_prompt = prompt;
            let prompt = raw_prompt
                .strip_prefix(DEFERRED_CREATE_PROMPT_PREFIX)
                .unwrap_or(&raw_prompt)
                .to_owned();
            eprintln!("scripted provider receipt: raw_prompt={raw_prompt:?} plan_key={prompt:?}");
            let step = self
                .plans
                .lock()
                .unwrap()
                .get_mut(&prompt)
                .expect("no unplanned session request")
                .pop_front()
                .expect("no extra model attempt");
            let results = request
                .messages
                .iter()
                .flat_map(|m| match m {
                    Message::ToolResults { results, .. } => results.clone(),
                    _ => vec![],
                })
                .collect();
            self.seen
                .send(Seen {
                    raw_prompt,
                    prompt: prompt.clone(),
                    tools: request.tools.iter().map(|t| t.name.to_string()).collect(),
                    results,
                })
                .unwrap();
            Box::pin(async_stream::stream! {
                let stop_reason = match step {
                    Step::Load { tool, call_id } => {
                        yield Ok(LlmEvent::ToolCallComplete { id: call_id.into(), name: "tool_catalog_load".into(), args: json!({"names":[tool]}), meta: None });
                        StopReason::ToolUse
                    }
                    Step::Call { tool, call_id } => {
                        yield Ok(LlmEvent::ToolCallComplete { id: call_id.into(), name: tool.into(), args: json!({}), meta: None });
                        StopReason::ToolUse
                    }
                    Step::Finish { gate } => {
                        if let Some(gate) = gate { gate.acquire().await.unwrap().forget(); }
                        yield Ok(LlmEvent::TextDelta { delta: format!("finished:{prompt}"), meta: None });
                        StopReason::EndTurn
                    }
                };
                // Matches the existing RPC scripted-provider fixture's attribution.
                let provider = if request.model.starts_with("claude-") { meerkat_core::Provider::Anthropic }
                    else if request.model.starts_with("gpt-") || request.model.starts_with("o1-") { meerkat_core::Provider::OpenAI }
                    else if request.model.starts_with("gemini-") { meerkat_core::Provider::Gemini }
                    else { meerkat_core::Provider::Other };
                yield Ok(LlmEvent::UsageUpdate { usage: meerkat_core::TurnUsage::host_declared(provider, &request.model, meerkat_core::Usage::default()) });
                yield Ok(LlmEvent::Done { outcome: LlmDoneOutcome::success(stop_reason) });
            })
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }
        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct Connection {
        writer: OwnedWriteHalf,
        frames: mpsc::UnboundedReceiver<Value>,
        reader_task: JoinHandle<()>,
    }
    impl Drop for Connection {
        fn drop(&mut self) {
            self.reader_task.abort();
        }
    }
    impl Connection {
        async fn send(&mut self, frame: Value) {
            self.writer
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
            self.writer.flush().await.unwrap();
        }
        async fn next_significant(&mut self) -> Value {
            tokio::time::timeout(LIMIT, async {
                loop {
                    let frame = self
                        .frames
                        .recv()
                        .await
                        .expect("TCP closed before expected frame");
                    if !frame["id"].is_null() {
                        return frame;
                    }
                }
            })
            .await
            .expect("bounded TCP response/callback wait")
        }
        async fn response(&mut self, id: u64) -> Value {
            let frame = self.next_significant().await;
            assert_eq!(
                frame["id"],
                json!(id),
                "unexpected callback/response: {frame}"
            );
            assert!(frame["error"].is_null(), "RPC error: {frame}");
            frame["result"].clone()
        }
        async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await;
            self.response(id).await
        }
        async fn register(&mut self, tool: &str) {
            let result = self.request(2, "tools/register", json!({"tools":[{"name":tool,"description":"connection-owned synthetic callback","input_schema":{"type":"object","properties":{}}}]})).await;
            assert!(result["registered"].as_u64().unwrap() > 0);
        }
        async fn create(&mut self) -> String {
            self.request(3, "session/create", json!({"prompt":"deferred bootstrap","initial_turn":"deferred","enable_builtins":false,"enable_shell":false,"enable_memory":false,"enable_mob":false})).await["session_id"].as_str().unwrap().to_owned()
        }
        async fn start(&mut self, session: &str, prompt: &str) {
            self.send(json!({"jsonrpc":"2.0","id":4,"method":"turn/start","params":{"session_id":session,"prompt":prompt}})).await;
        }
        async fn answer(&mut self, callback: &Value, content: &str) {
            assert_eq!(callback["method"], "tool/execute", "{callback}");
            self.send(json!({"jsonrpc":"2.0","id":callback["id"],"result":{"content":content,"is_error":false}})).await;
        }
    }
    struct Harness {
        _temp: tempfile::TempDir,
        runtime: Arc<SessionRuntime>,
        config: Arc<dyn ConfigStore>,
        provider: Arc<ScriptedProvider>,
        seen: mpsc::UnboundedReceiver<Seen>,
        servers: Vec<Option<JoinHandle<Result<(), ServerError>>>>,
        stores: HarnessStores,
    }
    /// The durable stores a restarted runtime reopens.
    #[derive(Clone)]
    struct HarnessStores {
        sessions: Arc<dyn meerkat::SessionStore>,
        runtime: Arc<dyn meerkat_runtime::RuntimeStore>,
        blobs: Arc<dyn BlobStore>,
    }
    impl Harness {
        fn new() -> Self {
            Self::with_config(|_| {
                Arc::new(MemoryConfigStore::new(
                    Config::default(),
                    meerkat_models::canonical(),
                ))
            })
        }
        /// A harness whose mob state persists its mobs under the harness
        /// root, so [`Self::restart`] restores them.
        fn new_persistent() -> Self {
            Self::with_config(|root| {
                Arc::new(meerkat_core::TaggedConfigStore::new(
                    Arc::new(MemoryConfigStore::new(
                        Config::default(),
                        meerkat_models::canonical(),
                    )),
                    meerkat_core::ConfigStoreMetadata {
                        realm_id: None,
                        instance_id: None,
                        backend: None,
                        resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                            root: root.display().to_string(),
                            manifest_path: root.join("realm.toml").display().to_string(),
                            config_path: root.join("config.toml").display().to_string(),
                            sessions_sqlite_path: None,
                            sessions_jsonl_dir: root.join("sessions").display().to_string(),
                        }),
                    },
                ))
            })
        }
        fn with_config(config: impl FnOnce(&std::path::Path) -> Arc<dyn ConfigStore>) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let (tx, seen) = mpsc::unbounded_channel();
            let provider = Arc::new(ScriptedProvider {
                plans: Mutex::new(HashMap::new()),
                seen: tx,
            });
            let stores = HarnessStores {
                sessions: Arc::new(meerkat::MemoryStore::new()),
                runtime: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                blobs: Arc::new(meerkat_store::MemoryBlobStore::new()),
            };
            let config = config(temp.path());
            let runtime = Self::start_runtime(temp.path(), &stores, &config, &provider);
            Self {
                _temp: temp,
                runtime,
                config,
                provider,
                seen,
                servers: vec![],
                stores,
            }
        }
        fn start_runtime(
            root: &std::path::Path,
            stores: &HarnessStores,
            config: &Arc<dyn ConfigStore>,
            provider: &Arc<ScriptedProvider>,
        ) -> Arc<SessionRuntime> {
            let runtime = SessionRuntime::new(
                AgentFactory::new(root.join("sessions")),
                Config::default(),
                10,
                meerkat::PersistenceBundle::new(
                    Arc::clone(&stores.sessions),
                    Arc::clone(&stores.runtime),
                    Arc::clone(&stores.blobs),
                )
                .expect("construct runtime authority"),
                meerkat_rpc::router::NotificationSink::noop(),
            )
            .expect("compose the RPC session runtime");
            runtime.set_default_llm_client(Some(provider.clone()));
            runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
                config.clone(),
                root.join("config_state.json"),
            )));
            Arc::new(runtime)
        }
        /// Stop every connection and the runtime, then start a new runtime
        /// over the same stores and config, as a process restart would.
        async fn restart(&mut self) {
            for server in &mut self.servers {
                if let Some(server) = server.take() {
                    server.abort();
                    let _ = server.await;
                }
            }
            // The old host's mobs release their process-wide supervisor
            // routes, as the old process's exit would, before the new host
            // restores them. Their durable state is untouched.
            #[cfg(feature = "mob")]
            if let Some(mob_state) = self.runtime.mob_state() {
                for (mob_id, _) in mob_state.mob_list().await.expect("the old host's mobs") {
                    let handle = mob_state
                        .handle_for(&mob_id)
                        .await
                        .expect("the old host's mob");
                    tokio::time::timeout(LIMIT, handle.shutdown())
                        .await
                        .expect("bounded mob shutdown before the restart")
                        .expect("the old host's mob shuts down");
                }
            }
            tokio::time::timeout(LIMIT, self.runtime.try_shutdown())
                .await
                .expect("bounded runtime shutdown before the restart")
                .unwrap();
            self.runtime = Self::start_runtime(
                self._temp.path(),
                &self.stores,
                &self.config,
                &self.provider,
            );
        }
        async fn connect(&mut self) -> (usize, Connection) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let stream = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (accepted, _) = listener.accept().await.unwrap();
            let runtime = self.runtime.clone();
            let config = self.config.clone();
            let index = self.servers.len();
            self.servers.push(Some(tokio::spawn(serve_tcp_connection(
                accepted, runtime, config, None,
            ))));
            let (reader, writer) = stream.into_split();
            let (tx, frames) = mpsc::unbounded_channel();
            let reader_task = tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Some(line) = lines.next_line().await.unwrap() {
                    if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                        break;
                    }
                }
            });
            let mut connection = Connection {
                writer,
                frames,
                reader_task,
            };
            connection.request(1, "initialize", json!({})).await;
            (index, connection)
        }
        async fn seen(&mut self, prompt: &str) -> Seen {
            let seen = tokio::time::timeout(LIMIT, self.seen.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                seen.prompt, prompt,
                "plan key mismatch; raw request prompt: {:?}",
                seen.raw_prompt
            );
            seen
        }
        async fn disconnected(&mut self, index: usize) {
            let mut server = self.servers[index].take().unwrap();
            match tokio::time::timeout(LIMIT, &mut server).await {
                // Abrupt client closure can surface as EOF, a reset while
                // reading, or a broken pipe while writing. Accept only those
                // peer-close outcomes; parse, size, timeout, and other I/O
                // errors still fail with their real details.
                Ok(result) => match result.unwrap() {
                    Ok(()) => {}
                    Err(
                        ServerError::Io(error)
                        | ServerError::Transport(meerkat_rpc::transport::TransportError::Io(error)),
                    ) if matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ) => {}
                    Err(other) => panic!("TCP owner failed: {other:?}"),
                },
                Err(_) => {
                    server.abort();
                    let _ = server.await;
                    panic!("TCP owner did not close");
                }
            }
        }
        async fn clean(&mut self) {
            for server in &mut self.servers {
                if let Some(server) = server.take() {
                    server.abort();
                    let _ = server.await;
                }
            }
            tokio::time::timeout(LIMIT, self.runtime.try_shutdown())
                .await
                .expect("bounded runtime cleanup")
                .unwrap();
        }
        fn exhausted(&self) {
            assert!(
                self.provider
                    .plans
                    .lock()
                    .unwrap()
                    .values()
                    .all(VecDeque::is_empty)
            );
        }
    }
    fn finish() -> Step {
        Step::Finish { gate: None }
    }
    fn assert_result(seen: &Seen, id: &str, expected: &str) {
        let results: Vec<_> = seen
            .results
            .iter()
            .filter(|r| r.tool_use_id == id)
            .collect();
        assert_eq!(results.len(), 1, "{seen:?}");
        assert!(!results[0].is_error, "{seen:?}");
        assert_eq!(results[0].text_content(), expected);
    }
    fn assert_catalog_load(seen: &Seen, id: &str, tool: &str) {
        let results: Vec<_> = seen
            .results
            .iter()
            .filter(|r| r.tool_use_id == id)
            .collect();
        assert_eq!(results.len(), 1, "{seen:?}");
        assert!(!results[0].is_error, "{seen:?}");
        let payload: Value = serde_json::from_str(&results[0].text_content()).unwrap();
        assert_eq!(payload["catalog_exact"], true, "{payload}");
        let resolutions = payload["resolutions"].as_array().unwrap();
        assert_eq!(resolutions.len(), 1, "{payload}");
        assert_eq!(resolutions[0]["name"], tool, "{payload}");
        assert_eq!(resolutions[0]["accepted"], true, "{payload}");
        assert!(resolutions[0]["rejected_reason"].is_null(), "{payload}");
        // Both newly accepted deferred loads and accepted inline no-ops are valid.
    }
    async fn run_case(case: u8) {
        let mut h = Harness::new();
        let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(50), async {
            match case {
                0 => registry_case(&mut h).await,
                1 => route_case(&mut h).await,
                2 => disconnect_case(&mut h, false).await,
                3 => disconnect_case(&mut h, true).await,
                #[cfg(feature = "mob")]
                4 => mob_member_case(&mut h, true).await,
                #[cfg(feature = "mob")]
                5 => mob_member_case(&mut h, false).await,
                #[cfg(feature = "mob")]
                6 => mob_foreign_refused_spawn_case(&mut h).await,
                #[cfg(feature = "mob")]
                7 => mob_recreated_case(&mut h, true).await,
                #[cfg(feature = "mob")]
                8 => mob_recreated_case(&mut h, false).await,
                #[cfg(feature = "mob")]
                9 => mob_claim_refused_while_owner_connected_case(&mut h).await,
                #[cfg(feature = "mob")]
                10 => mob_claim_after_owner_disconnect_case(&mut h).await,
                #[cfg(feature = "mob")]
                11 => mob_claim_needs_the_creators_token_case(&mut h).await,
                #[cfg(feature = "mob")]
                12 => mob_claim_token_of_a_destroyed_mob_is_refused_case(&mut h).await,
                #[cfg(feature = "mob")]
                13 => mob_created_in_process_is_not_claimable_case(&mut h).await,
                #[cfg(feature = "mob")]
                14 => host_mints_a_claim_token_for_a_tokenless_mob_case(&mut h).await,
                _ => unreachable!(),
            }
            h.exhausted();
        }))
        .catch_unwind()
        .await;
        h.clean().await;
        match result {
            Ok(result) => result.expect("bounded complete scenario"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    async fn registry_case(h: &mut Harness) {
        for prompt in ["a-before-b", "a-after-b", "b-positive"] {
            h.provider.plan(prompt, [finish()]);
        }
        let (_, mut a) = h.connect().await;
        a.register("a_lookup").await;
        let first = a.create().await;
        a.start(&first, "a-before-b").await;
        a.response(4).await;
        let before = h.seen("a-before-b").await;
        assert!(
            before.tools.iter().any(|t| t == "a_lookup"),
            "positive: {before:?}"
        );
        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        let later = a.create().await;
        a.start(&later, "a-after-b").await;
        a.response(4).await;
        let after = h.seen("a-after-b").await;
        let b_session = b.create().await;
        b.start(&b_session, "b-positive").await;
        b.response(4).await;
        let b_seen = h.seen("b-positive").await;
        assert!(
            b_seen.tools.iter().any(|t| t == "b_lookup"),
            "positive: {b_seen:?}"
        );
        assert!(!b_seen.tools.iter().any(|t| t == "a_lookup"), "{b_seen:?}");
        assert!(
            after.tools.iter().any(|t| t == "a_lookup"),
            "B cleared A's registry: {after:?}"
        );
        assert!(
            !after.tools.iter().any(|t| t == "b_lookup"),
            "B's tools leaked to A: {after:?}"
        );
    }
    async fn route_case(h: &mut Harness) {
        h.provider.plan(
            "route-a",
            [
                Step::Load {
                    tool: "a_lookup",
                    call_id: "load-a",
                },
                Step::Call {
                    tool: "a_lookup",
                    call_id: "call-a",
                },
                finish(),
            ],
        );
        h.provider.plan(
            "route-b",
            [
                Step::Load {
                    tool: "b_lookup",
                    call_id: "load-b",
                },
                Step::Call {
                    tool: "b_lookup",
                    call_id: "call-b",
                },
                finish(),
            ],
        );
        let (_, mut a) = h.connect().await;
        let (_, mut b) = h.connect().await;
        // Register after both constructors so the baseline reaches actual misrouting,
        // independently of the previous test's registry-reset failure.
        a.register("a_lookup").await;
        b.register("b_lookup").await;
        let a_session = a.create().await;
        a.start(&a_session, "route-a").await;
        let a_initial = h.seen("route-a").await;
        assert!(
            a_initial
                .tools
                .iter()
                .any(|name| name == "tool_catalog_load"),
            "{a_initial:?}"
        );
        let a_loaded = h.seen("route-a").await;
        assert_catalog_load(&a_loaded, "load-a", "a_lookup");
        assert!(
            a_loaded.tools.iter().any(|name| name == "a_lookup"),
            "real catalog load: {a_loaded:?}"
        );
        let (owner, callback) = tokio::select! {
            frame = a.next_significant() => ("A", frame),
            frame = b.next_significant() => ("B", frame),
        };
        assert_eq!(callback["params"]["name"], "a_lookup", "{callback}");
        assert_eq!(callback["params"]["tool_use_id"], "call-a", "{callback}");
        // Finish the synthetic operation even on the wrong baseline route so B's
        // positive and A's actual retained result are checked before the RED oracle.
        if owner == "A" {
            a.answer(&callback, "a-synthetic-result").await;
        } else {
            b.answer(&callback, "a-synthetic-result").await;
        }
        a.response(4).await;
        assert_result(&h.seen("route-a").await, "call-a", "a-synthetic-result");
        let b_session = b.create().await;
        b.start(&b_session, "route-b").await;
        let b_initial = h.seen("route-b").await;
        assert!(
            b_initial
                .tools
                .iter()
                .any(|name| name == "tool_catalog_load"),
            "{b_initial:?}"
        );
        let b_loaded = h.seen("route-b").await;
        assert_catalog_load(&b_loaded, "load-b", "b_lookup");
        assert!(
            b_loaded.tools.iter().any(|name| name == "b_lookup"),
            "real catalog load: {b_loaded:?}"
        );
        let callback_b = b.next_significant().await;
        assert_eq!(callback_b["params"]["name"], "b_lookup");
        b.answer(&callback_b, "b-synthetic-result").await;
        b.response(4).await;
        assert_result(&h.seen("route-b").await, "call-b", "b-synthetic-result");
        assert_eq!(
            owner, "A",
            "A's real callback arrived on B's TCP connection: {callback}"
        );
    }
    /// `require_completion_after_eof` additionally asserts that A's accepted
    /// turn completes after its connection closed (T3b, issue #1458).
    async fn disconnect_case(h: &mut Harness, require_completion_after_eof: bool) {
        let finish_a = Arc::new(Semaphore::new(0));
        h.provider.plan(
            "orphan-a",
            [
                Step::Call {
                    tool: "a_lookup",
                    call_id: "orphan-call-a",
                },
                Step::Finish {
                    gate: Some(finish_a.clone()),
                },
            ],
        );
        h.provider.plan(
            "reconnected-b",
            [
                Step::Call {
                    tool: "b_lookup",
                    call_id: "new-call-b",
                },
                finish(),
            ],
        );
        h.provider.plan("warm-a", [finish()]);
        let (a_index, mut a) = h.connect().await;
        a.register("a_lookup").await;
        let a_session = a.create().await;
        // Materialize A's session with one completed turn first, so the
        // orphaned turn runs on a live session rather than a deferred
        // first-turn promotion (whose request owns the pending session).
        a.start(&a_session, "warm-a").await;
        a.response(4).await;
        h.seen("warm-a").await;
        // The session's callbacks stay on A, the connection that created it,
        // whichever connection starts the turn. T3b starts the orphaned turn
        // on A itself; this case starts it from a third connection that stays
        // open. A's EOF also cancels the requests A still has in flight
        // (issue #1458), and that cancel races the turn's next model request.
        let mut requester = if require_completion_after_eof {
            None
        } else {
            Some(h.connect().await.1)
        };
        match requester.as_mut() {
            Some(requester) => requester.start(&a_session, "orphan-a").await,
            None => a.start(&a_session, "orphan-a").await,
        }
        h.seen("orphan-a").await;
        let old_callback = a.next_significant().await;
        assert_eq!(old_callback["params"]["tool_use_id"], "orphan-call-a");
        assert_eq!(old_callback["id"], "srv-0");
        drop(a);
        h.disconnected(a_index).await;
        // Reaching the next actual model request after EOF proves accepted A work
        // survived its callback owner. Keep its final stream pending.
        let a_continuation = h.seen("orphan-a").await;
        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        let b_session = b.create().await;
        b.start(&b_session, "reconnected-b").await;
        h.seen("reconnected-b").await;
        let new_callback = b.next_significant().await;
        assert_eq!(
            new_callback["params"]["tool_use_id"], "new-call-b",
            "A callback escaped to B: {new_callback}"
        );
        assert_eq!(
            new_callback["id"], old_callback["id"],
            "exercise real reused callback sequence ID"
        );
        b.answer(&new_callback, "B-ONLY-RESPONSE").await;
        b.response(4).await;
        assert_result(
            &h.seen("reconnected-b").await,
            "new-call-b",
            "B-ONLY-RESPONSE",
        );
        // A's continuation after EOF carries exactly one failed result for its
        // orphaned call, typed with the existing tool-unavailability contract.
        // Baseline emitted execution_failed; this is a distinct improvement,
        // not proof of ID crossover.
        let results: Vec<_> = a_continuation
            .results
            .iter()
            .filter(|r| r.tool_use_id == "orphan-call-a")
            .collect();
        assert_eq!(results.len(), 1, "{a_continuation:?}");
        assert!(results[0].is_error);
        let payload: Value = serde_json::from_str(&results[0].text_content()).unwrap();
        assert_eq!(
            payload["error"], "tool_unavailable",
            "closed owner needs typed local feedback: {payload}"
        );
        let sid = meerkat_core::SessionId::parse(&a_session).unwrap();
        let read_history = |h: &Harness| {
            let runtime = h.runtime.clone();
            let sid = sid.clone();
            async move {
                let history = runtime
                    .read_session_history_rich(&sid, Default::default())
                    .await
                    .unwrap()
                    .unwrap();
                serde_json::to_value(history).unwrap()
            }
        };
        let history = read_history(&*h).await;
        assert!(
            !history.to_string().contains("B-ONLY-RESPONSE"),
            "B settled A: {history}"
        );
        finish_a.add_permits(1);
        if !require_completion_after_eof {
            return;
        }
        let history = tokio::time::timeout(LIMIT, async {
            loop {
                let value = read_history(&*h).await;
                if value.to_string().contains("finished:orphan-a") {
                    break value;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("A accepted work finishes after EOF");
        assert!(
            !history.to_string().contains("B-ONLY-RESPONSE"),
            "B settled A: {history}"
        );
    }

    #[tokio::test]
    async fn tcp_callback_registry_stays_with_registering_connection() {
        run_case(0).await;
    }
    #[tokio::test]
    async fn tcp_callback_for_new_a_session_never_routes_to_b() {
        run_case(1).await;
    }
    #[tokio::test]
    async fn tcp_callback_disconnect_and_reused_id_keep_old_work_isolated() {
        run_case(2).await;
    }
    /// T3b: the same disconnect scenario, additionally requiring A's accepted
    /// turn to complete after its connection closed. A clean EOF aborts the
    /// connection's in-flight turn/start after the server's 5 s graceful
    /// window, so A's gated final stream never commits; that behaviour is
    /// issue #1458, separate from callback ownership.
    #[tokio::test]
    #[ignore = "#1458: TCP EOF aborts the connection's in-flight turn/start; out of scope for #1451"]
    async fn tcp_callback_disconnected_owner_accepted_work_finishes_after_eof() {
        run_case(3).await;
    }

    /// The base64 bytes of the image the `snap` callback tool returns.
    const SNAP_IMAGE: &str = "U05BUC1JTUFHRS1PTkUtQ09QWQ==";

    /// One tool result image reaches the client once. The turn's request
    /// opts out of its `session/event` copies, so the server writes none of
    /// them; the session-wide subscription carries every event once; and the
    /// tool's execution fact carries no copy of the result body. Before, the
    /// same turn wrote the image four times: on both tool events, each as a
    /// `session/event` and as a `session/stream_event`.
    #[tokio::test]
    async fn an_opted_out_turns_image_tool_result_reaches_the_client_once() {
        let mut h = Harness::new();
        h.provider.plan(
            "show the image",
            [
                Step::Call {
                    tool: "snap",
                    call_id: "call-snap",
                },
                Step::Finish { gate: None },
            ],
        );
        let (_index, mut c) = h.connect().await;
        c.register("snap").await;
        let session = c.create().await;
        let opened = c
            .request(5, "session/stream_open", json!({"session_id": session}))
            .await;
        let stream_id = opened["stream_id"].as_str().unwrap().to_owned();
        c.send(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "turn/start",
            "params": {"session_id": session, "prompt": "show the image", "session_events": false}
        }))
        .await;

        // Every frame until the turn's response and the subscription's
        // run_completed, answering the tool's callback with the image.
        let mut frames = Vec::new();
        let mut responded = false;
        let mut stream_completed = false;
        tokio::time::timeout(LIMIT, async {
            while !(responded && stream_completed) {
                let frame = c.frames.recv().await.expect("TCP closed mid-turn");
                if frame["method"] == "tool/execute" {
                    c.send(json!({
                        "jsonrpc": "2.0",
                        "id": frame["id"],
                        "result": {
                            "content": [{
                                "type": "image",
                                "media_type": "image/png",
                                "source": "inline",
                                "data": SNAP_IMAGE
                            }],
                            "is_error": false
                        }
                    }))
                    .await;
                } else if frame["id"] == json!(6) {
                    assert!(frame["error"].is_null(), "turn/start failed: {frame}");
                    responded = true;
                } else if frame["method"] == "session/stream_event"
                    && frame["params"]["event"]["payload"]["type"] == "run_completed"
                {
                    stream_completed = true;
                }
                frames.push(frame);
            }
        })
        .await
        .expect("the turn finishes and its stream delivers run_completed");

        let session_events = frames
            .iter()
            .filter(|frame| frame["method"] == "session/event")
            .count();
        assert_eq!(
            session_events, 0,
            "the opted-out turn wrote session/event copies"
        );
        let copies: Vec<&Value> = frames
            .iter()
            .filter(|frame| frame.to_string().contains(SNAP_IMAGE))
            .collect();
        assert_eq!(
            copies.len(),
            1,
            "the image reached the client {} times: {copies:?}",
            copies.len()
        );
        assert_eq!(copies[0]["method"], "session/stream_event", "{}", copies[0]);
        assert_eq!(copies[0]["params"]["stream_id"], json!(stream_id));
        assert_eq!(
            copies[0]["params"]["event"]["payload"]["type"], "tool_result_received",
            "{}",
            copies[0]
        );
        let completion = frames
            .iter()
            .find(|frame| {
                frame["method"] == "session/stream_event"
                    && frame["params"]["event"]["payload"]["type"] == "tool_execution_completed"
            })
            .expect("the subscription carries the tool's execution fact");
        assert!(
            completion["params"]["event"]["payload"]
                .get("content")
                .is_none(),
            "the execution fact carries no result body: {completion}"
        );
    }

    /// A `turn/start` without the opt-out still writes its events as
    /// `session/event` notifications.
    #[tokio::test]
    async fn a_default_turn_start_still_writes_session_events() {
        let mut h = Harness::new();
        h.provider.plan("plain turn", [Step::Finish { gate: None }]);
        let (_index, mut c) = h.connect().await;
        let session = c.create().await;
        c.send(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "turn/start",
            "params": {"session_id": session, "prompt": "plain turn"}
        }))
        .await;
        let mut session_events = 0;
        tokio::time::timeout(LIMIT, async {
            loop {
                let frame = c.frames.recv().await.expect("TCP closed mid-turn");
                if frame["method"] == "session/event" {
                    session_events += 1;
                }
                if frame["id"] == json!(6) {
                    assert!(frame["error"].is_null(), "turn/start failed: {frame}");
                    break;
                }
            }
        })
        .await
        .expect("the turn finishes");
        assert!(
            session_events > 0,
            "a default turn/start wrote no session/event"
        );
    }

    /// The `durable` detached callback tool as a connection registers it:
    /// one runner and contract, and the description that tells two
    /// declarations apart.
    fn durable_tool(description: &str) -> Value {
        json!([{
            "name": "durable",
            "description": description,
            "input_schema": {"type": "object", "properties": {}},
            "execution": {
                "mode": "detached",
                "runner": {"name": "host.durable", "version": "v1"},
                "restart_class": "adoptable",
                "idempotency_scope": "tool_call",
                "submission_timeout_ms": 5000
            }
        }])
    }

    impl Connection {
        /// Register `tools`. A registration reconciles the detached callback
        /// jobs its tools serve concurrently with its response: the server
        /// requests that arrive before the response are returned.
        async fn register_tools_and_collect_requests(
            &mut self,
            id: u64,
            tools: Value,
        ) -> Vec<Value> {
            self.send(
                json!({"jsonrpc":"2.0","id":id,"method":"tools/register","params":{"tools":tools}}),
            )
            .await;
            let mut requests = Vec::new();
            loop {
                let frame = self.next_significant().await;
                if frame["method"].is_string() {
                    requests.push(frame);
                    continue;
                }
                assert_eq!(frame["id"], json!(id), "{frame}");
                assert!(frame["error"].is_null(), "RPC error: {frame}");
                assert!(
                    frame["result"]["registered"].as_u64().unwrap() > 0,
                    "{frame}"
                );
                return requests;
            }
        }

        /// The job's `jobs/get` summary, or the server request that arrived
        /// before the response.
        async fn get_job(&mut self, id: u64, job_id: &str) -> Result<Value, Value> {
            self.send(
                json!({"jsonrpc":"2.0","id":id,"method":"jobs/get","params":{"job_id":job_id}}),
            )
            .await;
            let frame = self.next_significant().await;
            if frame["method"].is_string() {
                return Err(frame);
            }
            assert_eq!(frame["id"], json!(id), "{frame}");
            assert!(frame["error"].is_null(), "RPC error: {frame}");
            Ok(frame["result"]["job"].clone())
        }
    }

    /// RED 5 (#222) over real connections: a detached callback job whose
    /// submitting connection closed is reconciled only by a connection whose
    /// tool declares what the job recorded. The creator's second turn calls
    /// its detached `durable` tool (the turn's caller context gives it the
    /// runtime-owned interaction lineage a detached call records); the
    /// creator takes the attempt's start and closes without answering it,
    /// and the attempt was claimed (with a 120 s lease) before its start was
    /// sent, so the job stays running until a
    /// connection serving the tool reconciles it. A connection registering
    /// the creator's declaration is offered the job, and it keeps running
    /// once reported live; one registering the same name and runner with
    /// another description is offered nothing, and the job goes to
    /// needs_attention (`callback_binding_changed`).
    #[tokio::test]
    async fn tcp_detached_callback_job_is_reconciled_only_under_the_declaration_it_recorded() {
        let mut h = Harness::new();
        h.runtime.set_realm_context(
            Some(meerkat_core::connection::RealmId::global()),
            None,
            None,
        );
        let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(50), async {
            h.provider.plan("warm-durable", [finish()]);
            h.provider.plan(
                "submit-durable",
                [
                    Step::Load {
                        tool: "durable",
                        call_id: "load-durable",
                    },
                    Step::Call {
                        tool: "durable",
                        call_id: "call-durable",
                    },
                    finish(),
                ],
            );
            let (creator_index, mut creator) = h.connect().await;
            let offered = creator
                .register_tools_and_collect_requests(2, durable_tool("the creator's durable tool"))
                .await;
            assert!(offered.is_empty(), "{offered:?}");
            let session = creator.create().await;
            creator.start(&session, "warm-durable").await;
            creator.response(4).await;
            h.seen("warm-durable").await;
            creator
                .send(json!({"jsonrpc":"2.0","id":5,"method":"turn/start","params":{"session_id":session,"prompt":"submit-durable","transient_turn_context":"submit-durable"}}))
                .await;
            h.seen("submit-durable").await;
            assert_catalog_load(
                &h.seen("submit-durable").await,
                "load-durable",
                "durable",
            );
            let submitted = h.seen("submit-durable").await;
            let results: Vec<_> = submitted
                .results
                .iter()
                .filter(|result| result.tool_use_id == "call-durable")
                .collect();
            assert_eq!(results.len(), 1, "{submitted:?}");
            assert!(!results[0].is_error, "{submitted:?}");
            let receipt: Value = serde_json::from_str(&results[0].text_content()).unwrap();
            let job_id = receipt["job_id"].as_str().unwrap().to_owned();
            let mut start = None;
            let mut turn_done = false;
            while start.is_none() || !turn_done {
                let frame = creator.next_significant().await;
                if frame["method"] == "callback/job/start" {
                    assert!(start.is_none(), "a second attempt start: {frame}");
                    start = Some(frame);
                } else {
                    assert_eq!(frame["id"], json!(5), "{frame}");
                    assert!(frame["error"].is_null(), "RPC error: {frame}");
                    turn_done = true;
                }
            }
            let start = start.unwrap();
            assert_eq!(
                start["params"]["authority"]["job_id"],
                json!(job_id),
                "{start}"
            );
            drop(creator);
            h.disconnected(creator_index).await;

            // The creator's declaration on another connection: the job is
            // offered to it and keeps running once reported live.
            let (unchanged_index, mut unchanged) = h.connect().await;
            let mut offered = unchanged
                .register_tools_and_collect_requests(2, durable_tool("the creator's durable tool"))
                .await;
            if offered.is_empty() {
                offered.push(unchanged.next_significant().await);
            }
            assert_eq!(offered.len(), 1, "{offered:?}");
            let reconcile = &offered[0];
            assert_eq!(reconcile["method"], "callback/job/reconcile", "{reconcile}");
            let attempts = reconcile["params"]["attempts"].as_array().unwrap();
            assert_eq!(attempts.len(), 1, "{reconcile}");
            assert_eq!(
                attempts[0]["authority"],
                start["params"]["authority"],
                "{reconcile}"
            );
            unchanged
                .send(json!({"jsonrpc":"2.0","id":reconcile["id"],"result":{"live_attempts":[attempts[0]["authority"]]}}))
                .await;
            let job = unchanged
                .get_job(3, &job_id)
                .await
                .unwrap_or_else(|request| panic!("a second request: {request}"));
            assert_eq!(job["phase"], "running", "{job}");
            drop(unchanged);
            h.disconnected(unchanged_index).await;

            // The same name and runner under another description.
            let (_, mut changed) = h.connect().await;
            let offered = changed
                .register_tools_and_collect_requests(2, durable_tool("another connection's durable tool"))
                .await;
            assert!(
                offered.is_empty(),
                "the job is offered to a connection declaring another tool: {offered:?}"
            );
            let job = tokio::time::timeout(LIMIT, async {
                let mut id = 2;
                loop {
                    id += 1;
                    let job = changed.get_job(id, &job_id).await.unwrap_or_else(|request| {
                        panic!("the job is offered to a connection declaring another tool: {request}")
                    });
                    if job["phase"] != "running" {
                        break job;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the job leaves running");
            assert_eq!(job["phase"], "needs_attention", "{job}");
            assert_eq!(
                job["terminal_result"],
                json!({"kind": "needs_attention", "reason": "callback_binding_changed"}),
                "{job}"
            );
            h.exhausted();
        }))
        .catch_unwind()
        .await;
        h.clean().await;
        match result {
            Ok(result) => result.expect("bounded complete scenario"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// A mob member's callback tools are the tools registered on the
    /// connection that created its mob: the member sees the tool, loads it
    /// and its call reaches that connection, whether the tool was registered
    /// before the member was spawned (`register_first`) or after. Each variant
    /// names its own mob: the in-process route registry is process-wide, so
    /// under plain `cargo test` the two variants run side by side.
    #[cfg(feature = "mob")]
    async fn mob_member_case(h: &mut Harness, register_first: bool) {
        let (prompt, tool, mob) = if register_first {
            (
                "mob-register-then-spawn",
                "early_lookup",
                "callback_mob_early",
            )
        } else {
            (
                "mob-spawn-then-register",
                "late_lookup",
                "callback_mob_late",
            )
        };
        h.provider.plan(
            prompt,
            [
                Step::Load {
                    tool,
                    call_id: "load-member",
                },
                Step::Call {
                    tool,
                    call_id: "call-member",
                },
                finish(),
            ],
        );
        let (_, mut a) = h.connect().await;
        if register_first {
            a.register(tool).await;
        }
        let mob_id = a
            .request(
                5,
                "mob/create",
                json!({"definition":{"id":mob,"profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
            )
            .await["mob_id"]
            .as_str()
            .unwrap()
            .to_owned();
        a.request(
            6,
            "mob/spawn",
            json!({"mob_id":mob_id,"profile":"worker","agent_identity":"w1","runtime_mode":"turn_driven"}),
        )
        .await;
        if !register_first {
            a.register(tool).await;
        }
        a.send(json!({"jsonrpc":"2.0","id":7,"method":"mob/turn_start","params":{"mob_id":mob_id,"agent_identity":"w1","prompt":prompt}}))
            .await;
        let initial = h.seen(prompt).await;
        assert!(
            initial.tools.iter().any(|name| name == "tool_catalog_load"),
            "the member has the connection's deferred callback tools: {initial:?}"
        );
        let loaded = h.seen(prompt).await;
        assert_catalog_load(&loaded, "load-member", tool);
        assert!(
            loaded.tools.iter().any(|name| name == tool),
            "real catalog load: {loaded:?}"
        );
        let callback = a.next_significant().await;
        assert_eq!(callback["params"]["name"], tool, "{callback}");
        assert_eq!(
            callback["params"]["tool_use_id"], "call-member",
            "{callback}"
        );
        a.answer(&callback, "member-synthetic-result").await;
        a.response(7).await;
        assert_result(
            &h.seen(prompt).await,
            "call-member",
            "member-synthetic-result",
        );
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_tools_registered_before_spawn_reach_the_mob_member() {
        run_case(4).await;
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_tools_registered_after_spawn_reach_the_mob_member() {
        run_case(5).await;
    }

    /// A refused member-building call from another connection binds nothing:
    /// after B's malformed mob/spawn on A's mob, A's member still sees and
    /// calls A's tool, and the call arrives on A.
    #[cfg(feature = "mob")]
    async fn mob_foreign_refused_spawn_case(h: &mut Harness) {
        let (prompt, tool) = ("mob-after-foreign-refused-spawn", "owner_lookup");
        h.provider.plan(
            prompt,
            [
                Step::Load {
                    tool,
                    call_id: "load-owner",
                },
                Step::Call {
                    tool,
                    call_id: "call-owner",
                },
                finish(),
            ],
        );
        let (_, mut a) = h.connect().await;
        let (_, mut b) = h.connect().await;
        a.register(tool).await;
        let mob_id = a
            .request(
                5,
                "mob/create",
                json!({"definition":{"id":"owned_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
            )
            .await["mob_id"]
            .as_str()
            .unwrap()
            .to_owned();
        // No profile or agent identity: the handler refuses it.
        b.send(json!({"jsonrpc":"2.0","id":6,"method":"mob/spawn","params":{"mob_id":mob_id}}))
            .await;
        let refused = b.next_significant().await;
        assert_eq!(refused["id"], json!(6), "{refused}");
        assert!(!refused["error"].is_null(), "{refused}");
        a.request(
            7,
            "mob/spawn",
            json!({"mob_id":mob_id,"profile":"worker","agent_identity":"w1","runtime_mode":"turn_driven"}),
        )
        .await;
        a.send(json!({"jsonrpc":"2.0","id":8,"method":"mob/turn_start","params":{"mob_id":mob_id,"agent_identity":"w1","prompt":prompt}}))
            .await;
        let loaded = {
            let initial = h.seen(prompt).await;
            assert!(
                initial.tools.iter().any(|name| name == "tool_catalog_load"),
                "{initial:?}"
            );
            h.seen(prompt).await
        };
        assert_catalog_load(&loaded, "load-owner", tool);
        let (owner, callback) = tokio::select! {
            frame = a.next_significant() => ("A", frame),
            frame = b.next_significant() => ("B", frame),
        };
        assert_eq!(callback["params"]["name"], tool, "{callback}");
        if owner == "A" {
            a.answer(&callback, "owner-synthetic-result").await;
        } else {
            b.answer(&callback, "owner-synthetic-result").await;
        }
        a.response(8).await;
        assert_result(
            &h.seen(prompt).await,
            "call-owner",
            "owner-synthetic-result",
        );
        assert_eq!(owner, "A", "A's member's callback arrived on B: {callback}");
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_refused_foreign_spawn_leaves_the_mob_route_with_its_creator() {
        run_case(6).await;
    }

    /// A mob's callback route belongs to the incarnation its connection
    /// created. A creates M, registers its tool and destroys M; M is then
    /// re-created, over RPC by B (`over_rpc`) or in-process, and B spawns a
    /// member. That member never reaches A's tool: over RPC it has B's, and
    /// in-process (no creating connection) it has no callback tools at all.
    /// Each variant names its own mob: the in-process route registry is
    /// process-wide, so under plain `cargo test` the two variants run side by
    /// side.
    #[cfg(feature = "mob")]
    async fn mob_recreated_case(h: &mut Harness, over_rpc: bool) {
        let (prompt, session) = if over_rpc {
            ("mob-recreated-over-rpc", "stale-over-rpc")
        } else {
            ("mob-recreated-in-process", "stale-in-process")
        };
        // The id `MobDefinition::implicit(session, ..)` gives.
        let mob = format!("implicit-{session}");
        if over_rpc {
            h.provider.plan(
                prompt,
                [
                    Step::Load {
                        tool: "b_lookup",
                        call_id: "load-b-member",
                    },
                    Step::Call {
                        tool: "b_lookup",
                        call_id: "call-b-member",
                    },
                    finish(),
                ],
            );
        } else {
            h.provider.plan(prompt, [finish()]);
        }
        let (_, mut a) = h.connect().await;
        let (_, mut b) = h.connect().await;
        a.register("a_lookup").await;
        b.register("b_lookup").await;
        let definition = json!({"id":mob,"profiles":{"delegate":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}});
        a.request(5, "mob/create", json!({ "definition": definition }))
            .await;
        a.request(6, "mob/destroy", json!({"mob_id":mob})).await;
        if over_rpc {
            b.request(7, "mob/create", json!({ "definition": definition }))
                .await;
        } else {
            h.runtime
                .mob_state()
                .expect("the RPC mob state")
                .mob_create_definition(meerkat_mob::MobDefinition::implicit(
                    session,
                    "claude-sonnet-4-6",
                ))
                .await
                .expect("re-created in-process");
        }
        b.request(
            8,
            "mob/spawn",
            json!({"mob_id":mob,"profile":"delegate","agent_identity":"w1","runtime_mode":"turn_driven"}),
        )
        .await;
        b.send(json!({"jsonrpc":"2.0","id":9,"method":"mob/turn_start","params":{"mob_id":mob,"agent_identity":"w1","prompt":prompt}}))
            .await;
        let initial = h.seen(prompt).await;
        if !over_rpc {
            // Its route slot is unclaimed: no connection's tools reach it.
            assert!(
                !initial.tools.iter().any(|name| name.ends_with("_lookup")),
                "a mob no connection created or claimed reached callback tools: {initial:?}"
            );
            b.response(9).await;
            return;
        }
        let loaded = h.seen(prompt).await;
        assert_catalog_load(&loaded, "load-b-member", "b_lookup");
        assert!(
            !loaded.tools.iter().any(|name| name == "a_lookup"),
            "A's tool reached the re-created mob: {loaded:?}"
        );
        let (owner, callback) = tokio::select! {
            frame = a.next_significant() => ("A", frame),
            frame = b.next_significant() => ("B", frame),
        };
        assert_eq!(callback["params"]["name"], "b_lookup", "{callback}");
        if owner == "B" {
            b.answer(&callback, "b-member-result").await;
        } else {
            a.answer(&callback, "b-member-result").await;
        }
        b.response(9).await;
        assert_result(&h.seen(prompt).await, "call-b-member", "b-member-result");
        assert_eq!(
            owner, "B",
            "the re-created mob's callback arrived on A: {callback}"
        );
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_mob_recreated_over_rpc_never_reaches_the_old_creator() {
        run_case(7).await;
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_mob_recreated_in_process_never_reaches_the_old_creator() {
        run_case(8).await;
    }

    /// The mob definition the claim cases create under `mob_id`.
    #[cfg(feature = "mob")]
    fn claim_mob_definition(mob_id: &str) -> Value {
        json!({"definition":{"id":mob_id,"profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}})
    }

    #[cfg(feature = "mob")]
    fn member_turn(id: u64, mob_id: &str, prompt: &str) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":"mob/turn_start","params":{"mob_id":mob_id,"agent_identity":"w1","prompt":prompt}})
    }

    /// The claim token `mob/create` returned for a mob.
    #[cfg(feature = "mob")]
    fn claim_token(created: &Value) -> Value {
        created["claim_token"].clone()
    }

    #[cfg(feature = "mob")]
    fn claim_params(mob_id: &str, token: &Value) -> Value {
        json!({"mob_id": mob_id, "claim_token": token})
    }

    /// A claim refused with `-32002` for `reason`.
    #[cfg(feature = "mob")]
    fn assert_claim_refused(refused: &Value, id: u64, mob_id: &str, reason: &str) {
        assert_eq!(refused["id"], json!(id), "{refused}");
        assert_eq!(
            refused["error"]["code"],
            json!(meerkat_rpc::error::SESSION_BUSY),
            "the claim is refused: {refused}"
        );
        assert_eq!(
            refused["error"]["data"],
            json!({"mob_id": mob_id, "reason": reason}),
            "the refusal carries its typed data: {refused}"
        );
    }

    /// A connection cannot take a mob's callback route from an owner whose
    /// connection is still open, even with the mob's claim token: B's claim
    /// of A's mob is refused with a typed error, while A's own claim of the
    /// route it holds is accepted.
    #[cfg(feature = "mob")]
    async fn mob_claim_refused_while_owner_connected_case(h: &mut Harness) {
        let (_, mut a) = h.connect().await;
        let (_, mut b) = h.connect().await;
        a.register("a_lookup").await;
        let created = a
            .request(5, "mob/create", claim_mob_definition("held_mob"))
            .await;
        let token = claim_token(&created);
        b.send(json!({"jsonrpc":"2.0","id":6,"method":"mob/claim_callback_route","params":claim_params("held_mob", &token)}))
            .await;
        let refused = b.next_significant().await;
        assert_eq!(refused["id"], json!(6), "{refused}");
        assert_eq!(
            refused["error"]["code"],
            json!(meerkat_rpc::error::SESSION_BUSY),
            "a claim of a route its open owner holds is refused: {refused}"
        );
        assert_eq!(
            refused["error"]["data"],
            json!({"mob_id": "held_mob", "reason": "held_by_open_connection"}),
            "the refusal carries its typed data: {refused}"
        );
        let held = a
            .request(
                7,
                "mob/claim_callback_route",
                claim_params("held_mob", &token),
            )
            .await;
        assert_eq!(held["mob_id"], "held_mob", "{held}");
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_claim_is_refused_while_the_owner_is_connected() {
        run_case(9).await;
    }

    /// When a mob's owning connection closes, its members' callbacks fail
    /// typed and reach no connection, and another connection may then claim
    /// the route. A's member calls A's tool; A disconnects; the member's next
    /// call of that tool fails `tool_unavailable` naming the unclaimed route;
    /// B claims the route, and the same live member loads and calls B's tool,
    /// whose call reaches B.
    #[cfg(feature = "mob")]
    async fn mob_claim_after_owner_disconnect_case(h: &mut Harness) {
        h.provider.plan(
            "handoff-owner",
            [
                Step::Load {
                    tool: "a_lookup",
                    call_id: "load-owner",
                },
                Step::Call {
                    tool: "a_lookup",
                    call_id: "call-owner",
                },
                finish(),
            ],
        );
        h.provider.plan(
            "handoff-unclaimed",
            [
                Step::Call {
                    tool: "a_lookup",
                    call_id: "call-unclaimed",
                },
                finish(),
            ],
        );
        h.provider.plan(
            "handoff-claimed",
            [
                Step::Load {
                    tool: "b_lookup",
                    call_id: "load-claimer",
                },
                Step::Call {
                    tool: "b_lookup",
                    call_id: "call-claimer",
                },
                finish(),
            ],
        );
        let (a_index, mut a) = h.connect().await;
        a.register("a_lookup").await;
        let token = claim_token(
            &a.request(5, "mob/create", claim_mob_definition("handoff_mob"))
                .await,
        );
        a.request(
            6,
            "mob/spawn",
            json!({"mob_id":"handoff_mob","profile":"worker","agent_identity":"w1","runtime_mode":"turn_driven"}),
        )
        .await;
        a.send(member_turn(7, "handoff_mob", "handoff-owner")).await;
        h.seen("handoff-owner").await;
        let loaded = h.seen("handoff-owner").await;
        assert_catalog_load(&loaded, "load-owner", "a_lookup");
        let callback = a.next_significant().await;
        assert_eq!(callback["params"]["name"], "a_lookup", "{callback}");
        a.answer(&callback, "owner-result").await;
        a.response(7).await;
        assert_result(&h.seen("handoff-owner").await, "call-owner", "owner-result");
        drop(a);
        h.disconnected(a_index).await;

        // No connection holds the route: the member's call fails typed.
        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        b.send(member_turn(8, "handoff_mob", "handoff-unclaimed"))
            .await;
        h.seen("handoff-unclaimed").await;
        let after_call = h.seen("handoff-unclaimed").await;
        let results: Vec<_> = after_call
            .results
            .iter()
            .filter(|result| result.tool_use_id == "call-unclaimed")
            .collect();
        assert_eq!(results.len(), 1, "{after_call:?}");
        assert!(results[0].is_error, "{after_call:?}");
        let payload: Value = serde_json::from_str(&results[0].text_content()).unwrap();
        assert_eq!(payload["error"], "tool_unavailable", "{payload}");
        assert!(
            payload["message"]
                .as_str()
                .is_some_and(|message| message.contains("no connected client holds")),
            "the failure names the unclaimed callback route: {payload}"
        );
        b.response(8).await;

        // B claims the route with the mob's claim token; the same live
        // member now reaches B.
        let claimed = b
            .request(
                9,
                "mob/claim_callback_route",
                claim_params("handoff_mob", &token),
            )
            .await;
        assert_eq!(claimed["mob_id"], "handoff_mob", "{claimed}");
        b.send(member_turn(10, "handoff_mob", "handoff-claimed"))
            .await;
        h.seen("handoff-claimed").await;
        let loaded = h.seen("handoff-claimed").await;
        assert_catalog_load(&loaded, "load-claimer", "b_lookup");
        let callback = b.next_significant().await;
        assert_eq!(callback["params"]["name"], "b_lookup", "{callback}");
        b.answer(&callback, "claimer-result").await;
        b.response(10).await;
        assert_result(
            &h.seen("handoff-claimed").await,
            "call-claimer",
            "claimer-result",
        );
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_route_is_claimed_after_the_owner_disconnects() {
        run_case(10).await;
    }

    /// Once a mob's creator has closed its connection, only a connection
    /// presenting the mob's claim token takes the mob's callback route: a
    /// claim without a token or with a malformed one is invalid, and one
    /// with another well-formed token is refused, typed; the creator's token
    /// claims it.
    #[cfg(feature = "mob")]
    async fn mob_claim_needs_the_creators_token_case(h: &mut Harness) {
        let (a_index, mut a) = h.connect().await;
        a.register("a_lookup").await;
        let token = claim_token(
            &a.request(5, "mob/create", claim_mob_definition("token_mob"))
                .await,
        );
        drop(a);
        h.disconnected(a_index).await;

        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        let presented = token
            .as_str()
            .expect("mob/create returns the mob's claim token")
            .to_owned();
        for (id, params) in [
            (6, json!({"mob_id": "token_mob"})),
            (7, claim_params("token_mob", &json!("not-a-token"))),
            // The creator's token with a stray character.
            (
                10,
                claim_params("token_mob", &json!(format!("{presented} "))),
            ),
        ] {
            b.send(json!({"jsonrpc":"2.0","id":id,"method":"mob/claim_callback_route","params":params}))
                .await;
            let refused = b.next_significant().await;
            assert!(
                !refused.to_string().contains(&presented),
                "an invalid claim never echoes the token it was given (id {id})"
            );
            assert_eq!(refused["id"], json!(id), "{refused}");
            assert_eq!(
                refused["error"]["code"],
                json!(-32602),
                "a claim needs a well-formed claim token: {refused}"
            );
        }
        assert!(
            token
                .as_str()
                .is_some_and(|token| token.starts_with("mct_") && token.len() == 47),
            "mob/create returns the mob's claim token: {token}"
        );
        b.send(json!({"jsonrpc":"2.0","id":8,"method":"mob/claim_callback_route","params":claim_params("token_mob", &json!(format!("mct_{}", "A".repeat(43))))}))
            .await;
        assert_claim_refused(
            &b.next_significant().await,
            8,
            "token_mob",
            "claim_token_mismatch",
        );
        let claimed = b
            .request(
                9,
                "mob/claim_callback_route",
                claim_params("token_mob", &token),
            )
            .await;
        assert_eq!(claimed["mob_id"], "token_mob", "{claimed}");
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_route_claim_needs_the_creators_token() {
        run_case(11).await;
    }

    /// A mob destroyed and created again under its id gets a new claim
    /// token, and the destroyed mob's token claims nothing.
    #[cfg(feature = "mob")]
    async fn mob_claim_token_of_a_destroyed_mob_is_refused_case(h: &mut Harness) {
        let (a_index, mut a) = h.connect().await;
        a.register("a_lookup").await;
        let destroyed = claim_token(
            &a.request(5, "mob/create", claim_mob_definition("recreated_mob"))
                .await,
        );
        a.request(6, "mob/destroy", json!({"mob_id": "recreated_mob"}))
            .await;
        let current = claim_token(
            &a.request(7, "mob/create", claim_mob_definition("recreated_mob"))
                .await,
        );
        assert_ne!(destroyed, current, "a new mob gets a new claim token");
        drop(a);
        h.disconnected(a_index).await;

        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        b.send(json!({"jsonrpc":"2.0","id":8,"method":"mob/claim_callback_route","params":claim_params("recreated_mob", &destroyed)}))
            .await;
        assert_claim_refused(
            &b.next_significant().await,
            8,
            "recreated_mob",
            "claim_token_mismatch",
        );
        let claimed = b
            .request(
                9,
                "mob/claim_callback_route",
                claim_params("recreated_mob", &current),
            )
            .await;
        assert_eq!(claimed["mob_id"], "recreated_mob", "{claimed}");
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_route_claim_refuses_a_destroyed_mobs_token() {
        run_case(12).await;
    }

    /// A mob created other than by `mob/create` has no claim token, so no
    /// token claims its callback route: the claim is refused as
    /// `not_claimable`.
    #[cfg(feature = "mob")]
    async fn mob_created_in_process_is_not_claimable_case(h: &mut Harness) {
        // The router composes the runtime's mob state when it serves its
        // first connection.
        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        h.runtime
            .mob_state()
            .expect("the RPC mob state")
            .mob_create_definition(meerkat_mob::MobDefinition::implicit(
                "tokenless",
                "claude-sonnet-4-6",
            ))
            .await
            .expect("created in-process");
        b.send(json!({"jsonrpc":"2.0","id":6,"method":"mob/claim_callback_route","params":claim_params("implicit-tokenless", &json!(format!("mct_{}", "A".repeat(43))))}))
            .await;
        assert_claim_refused(
            &b.next_significant().await,
            6,
            "implicit-tokenless",
            "not_claimable",
        );
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_route_of_a_mob_created_in_process_is_not_claimable() {
        run_case(13).await;
    }

    /// The host mints a claim token for a mob that has none, once, through
    /// its Rust API (no wire method mints one): the connection it hands the
    /// token to claims the route, and a second mint yields nothing, keeping
    /// the first token.
    #[cfg(feature = "mob")]
    async fn host_mints_a_claim_token_for_a_tokenless_mob_case(h: &mut Harness) {
        // The router composes the runtime's mob state when it serves its
        // first connection.
        let (_, mut b) = h.connect().await;
        b.register("b_lookup").await;
        let state = h.runtime.mob_state().expect("the RPC mob state");
        let mob_id = state
            .mob_create_definition(meerkat_mob::MobDefinition::implicit(
                "minted",
                "claude-sonnet-4-6",
            ))
            .await
            .expect("created in-process");
        let token = state
            .mint_callback_claim_token(&mob_id)
            .await
            .expect("mint")
            .expect("a mob without a claim token takes one");
        assert!(
            state
                .mint_callback_claim_token(&mob_id)
                .await
                .expect("mint")
                .is_none(),
            "a mob's claim token is minted once"
        );
        let claimed = b
            .request(
                6,
                "mob/claim_callback_route",
                claim_params(mob_id.as_str(), &json!(token.expose_secret())),
            )
            .await;
        assert_eq!(claimed["mob_id"], json!(mob_id.as_str()), "{claimed}");
    }
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_route_takes_a_claim_token_the_host_minted() {
        run_case(14).await;
    }

    /// A mob restored after a restart has no callback route until a
    /// connection claims it with the claim token its creator received before
    /// the restart, and the claim reaches its restored member. A creates the
    /// mob and spawns its member; the runtime restarts over the same stores
    /// and restores the mob; B registers its tool, is refused with another
    /// token, and claims the restored mob's route with A's; the restored
    /// member then loads and calls B's tool, whose call reaches B.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn tcp_callback_restored_mob_member_reaches_the_connection_that_claims_it() {
        let mut h = Harness::new_persistent();
        let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(50), async {
            h.provider.plan(
                "restored-claimed",
                [
                    Step::Load {
                        tool: "b_lookup",
                        call_id: "load-restored",
                    },
                    Step::Call {
                        tool: "b_lookup",
                        call_id: "call-restored",
                    },
                    finish(),
                ],
            );
            let (a_index, mut a) = h.connect().await;
            a.register("a_lookup").await;
            let token = claim_token(
                &a.request(5, "mob/create", claim_mob_definition("restored_mob"))
                    .await,
            );
            a.request(
                6,
                "mob/spawn",
                json!({"mob_id":"restored_mob","profile":"worker","agent_identity":"w1","runtime_mode":"turn_driven"}),
            )
            .await;
            drop(a);
            h.disconnected(a_index).await;
            h.restart().await;

            let (_, mut b) = h.connect().await;
            b.register("b_lookup").await;
            b.send(json!({"jsonrpc":"2.0","id":17,"method":"mob/claim_callback_route","params":claim_params("restored_mob", &json!(format!("mct_{}", "A".repeat(43))))}))
                .await;
            assert_claim_refused(
                &b.next_significant().await,
                17,
                "restored_mob",
                "claim_token_mismatch",
            );
            let claimed = b
                .request(
                    7,
                    "mob/claim_callback_route",
                    claim_params("restored_mob", &token),
                )
                .await;
            assert_eq!(claimed["mob_id"], "restored_mob", "{claimed}");
            b.send(member_turn(8, "restored_mob", "restored-claimed"))
                .await;
            let initial = h.seen("restored-claimed").await;
            assert!(
                initial.tools.iter().any(|name| name == "tool_catalog_load"),
                "the restored member has the claimer's deferred callback tools: {initial:?}"
            );
            let loaded = h.seen("restored-claimed").await;
            assert_catalog_load(&loaded, "load-restored", "b_lookup");
            let callback = b.next_significant().await;
            assert_eq!(callback["params"]["name"], "b_lookup", "{callback}");
            b.answer(&callback, "restored-result").await;
            b.response(8).await;
            assert_result(
                &h.seen("restored-claimed").await,
                "call-restored",
                "restored-result",
            );
            h.exhausted();
        }))
        .catch_unwind()
        .await;
        h.clean().await;
        match result {
            Ok(result) => result.expect("bounded complete scenario"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// The detached callback tool `name` a connection registers. Each case
    /// registers its own name and runner, so a job one case queues never
    /// routes to another case's connection.
    fn detached_tool(name: &str) -> Value {
        json!([{
            "name": name,
            "description": "connection-owned detached callback",
            "input_schema": {"type": "object", "properties": {}},
            "execution": {
                "mode": "detached",
                "runner": {"name": format!("host.{name}"), "version": "v1"},
                "restart_class": "adoptable",
                "idempotency_scope": "tool_call",
                "submission_timeout_ms": 5000
            }
        }])
    }

    /// Plan the turn keyed `key`: load the deferred `tool`, call it once,
    /// finish.
    fn detached_plan(
        h: &Harness,
        key: &str,
        tool: &'static str,
        load: &'static str,
        call: &'static str,
    ) {
        h.provider.plan(
            key,
            [
                Step::Load {
                    tool,
                    call_id: load,
                },
                Step::Call {
                    tool,
                    call_id: call,
                },
                finish(),
            ],
        );
    }

    /// The result the turn keyed `key` got for its call `call`, as the model
    /// request after the call carries it.
    async fn detached_call_result(h: &mut Harness, key: &str, call: &str) -> ToolResult {
        h.seen(key).await;
        h.seen(key).await;
        let seen = h.seen(key).await;
        let mut results = seen
            .results
            .iter()
            .filter(|result| result.tool_use_id == call)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1, "{seen:?}");
        results.remove(0)
    }

    /// Whether `result` is a detached call's receipt: its job was queued.
    fn queued(result: &ToolResult) -> bool {
        !result.is_error
            && serde_json::from_str::<Value>(&result.text_content())
                .is_ok_and(|receipt| receipt["status"] == "queued" && receipt["job_id"].is_string())
    }

    /// The user row `session/history` shows for `prompt`, once an assistant
    /// row of the same turn carries its interaction id too (a row without one
    /// is returned at once). Read on a connection of its own: no callback
    /// ever routes to it.
    async fn history_user_row(h: &mut Harness, session: &str, prompt: &str) -> Value {
        let (_, mut reader) = h.connect().await;
        let mut last = Value::Null;
        for id in 100..400 {
            let history = reader
                .request(id, "session/history", json!({ "session_id": session }))
                .await;
            let messages = history["messages"].as_array().cloned().unwrap_or_default();
            if let Some(user) = messages.iter().position(|message| {
                message["role"] == "user" && message["content"].to_string().contains(prompt)
            }) {
                last = messages[user].clone();
                let interaction = &last["interaction_id"];
                let answered = messages[user + 1..].iter().any(|message| {
                    message["role"] == "block_assistant"
                        && message["interaction_id"] == *interaction
                });
                if interaction.is_null() || answered {
                    return last;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no answered user row {prompt:?} in session {session}: last {last}");
    }

    impl Connection {
        async fn register_tools(&mut self, id: u64, tools: Value) {
            let result = self
                .request(id, "tools/register", json!({ "tools": tools }))
                .await;
            assert!(result["registered"].as_u64().unwrap() > 0, "{result}");
        }
    }

    /// A detached callback call queues its job in every RPC turn, not only in
    /// a turn whose caller context gives it an interaction lineage: the
    /// deferred first turn (A), a plain turn/start on a live session (B), the
    /// immediate turn of session/create (C) and a mob member's turn (E). A
    /// turn/start with a transient turn context (D) is the control.
    #[tokio::test]
    async fn tcp_callback_detached_call_queues_its_job_in_every_rpc_turn() {
        let mut h = Harness::new();
        h.runtime.set_realm_context(
            Some(meerkat_core::connection::RealmId::global()),
            None,
            None,
        );
        let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(50), async {
            let mut outcomes: Vec<(&str, ToolResult)> = Vec::new();
            detached_plan(&h, "lineage-deferred", "durable_a", "load-a", "call-a");
            detached_plan(&h, "lineage-plain", "durable_b", "load-b", "call-b");
            detached_plan(&h, "lineage-immediate", "durable_c", "load-c", "call-c");
            detached_plan(&h, "lineage-context", "durable_d", "load-d", "call-d");
            h.provider.plan("lineage-warm-b", [finish()]);
            h.provider.plan("lineage-warm-d", [finish()]);

            let (_, mut deferred) = h.connect().await;
            deferred.register_tools(2, detached_tool("durable_a")).await;
            let session = deferred.create().await;
            deferred.start(&session, "lineage-deferred").await;
            outcomes.push((
                "A deferred first turn",
                detached_call_result(&mut h, "lineage-deferred", "call-a").await,
            ));
            let mut history = vec![(
                "A deferred first turn",
                history_user_row(&mut h, &session, "lineage-deferred").await,
            )];

            let (_, mut plain) = h.connect().await;
            plain.register_tools(2, detached_tool("durable_b")).await;
            let session = plain.create().await;
            plain.start(&session, "lineage-warm-b").await;
            plain.response(4).await;
            h.seen("lineage-warm-b").await;
            plain.send(json!({"jsonrpc":"2.0","id":5,"method":"turn/start","params":{"session_id":session,"prompt":"lineage-plain"}}))
                .await;
            outcomes.push((
                "B plain turn/start",
                detached_call_result(&mut h, "lineage-plain", "call-b").await,
            ));
            history.push((
                "B plain turn/start",
                history_user_row(&mut h, &session, "lineage-plain").await,
            ));

            let (_, mut immediate) = h.connect().await;
            immediate.register_tools(2, detached_tool("durable_c")).await;
            immediate.send(json!({"jsonrpc":"2.0","id":3,"method":"session/create","params":{"prompt":"lineage-immediate","enable_builtins":false,"enable_shell":false,"enable_memory":false,"enable_mob":false}}))
                .await;
            outcomes.push((
                "C session/create immediate turn",
                detached_call_result(&mut h, "lineage-immediate", "call-c").await,
            ));

            let (_, mut context) = h.connect().await;
            context.register_tools(2, detached_tool("durable_d")).await;
            let session = context.create().await;
            context.start(&session, "lineage-warm-d").await;
            context.response(4).await;
            h.seen("lineage-warm-d").await;
            context.send(json!({"jsonrpc":"2.0","id":5,"method":"turn/start","params":{"session_id":session,"prompt":"lineage-context","transient_turn_context":"lineage-context"}}))
                .await;
            outcomes.push((
                "D turn/start with transient_turn_context (control)",
                detached_call_result(&mut h, "lineage-context", "call-d").await,
            ));

            #[cfg(feature = "mob")]
            {
                detached_plan(&h, "lineage-mob", "durable_e", "load-e", "call-e");
                let (_, mut member) = h.connect().await;
                member.register_tools(2, detached_tool("durable_e")).await;
                member.request(5, "mob/create", claim_mob_definition("lineage_mob"))
                    .await;
                member.request(
                    6,
                    "mob/spawn",
                    json!({"mob_id":"lineage_mob","profile":"worker","agent_identity":"w1","runtime_mode":"turn_driven"}),
                )
                .await;
                member.send(member_turn(7, "lineage_mob", "lineage-mob")).await;
                outcomes.push((
                    "E mob/turn_start member turn",
                    detached_call_result(&mut h, "lineage-mob", "call-e").await,
                ));
            }

            for (case, result) in &outcomes {
                eprintln!(
                    "LINEAGE {case}: queued={} is_error={} {}",
                    queued(result),
                    result.is_error,
                    result.text_content()
                );
            }
            for (case, row) in &history {
                eprintln!(
                    "HISTORY {case}: interaction_id={} run_id={}",
                    row["interaction_id"], row["run_id"]
                );
            }
            let refused = outcomes
                .iter()
                .filter(|(_, result)| !queued(result))
                .map(|(case, result)| format!("{case}: {}", result.text_content()))
                .collect::<Vec<_>>();
            assert!(
                refused.is_empty(),
                "detached callback calls that did not queue their job:\n{}",
                refused.join("\n")
            );
            // The plain prompt's rows carry its interaction in history.
            for (case, row) in &history {
                assert!(
                    row["interaction_id"]
                        .as_str()
                        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()),
                    "{case}: history shows no interaction id: {row}"
                );
            }
            h.exhausted();
        }))
        .catch_unwind()
        .await;
        h.clean().await;
        match result {
            Ok(result) => result.expect("bounded complete scenario"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
