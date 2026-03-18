#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

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

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        let model = request.model.clone();
        Box::pin(stream::iter(vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: format!("runtime-backed [{model}]"),
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

fn spawn_test_server() -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
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

    let handle = tokio::spawn(async move {
        let _temp = temp;
        let reader = BufReader::new(server_reader);
        let mut server = RpcServer::new(reader, server_writer, runtime, config_store);
        server.run().await
    });

    (client_writer, BufReader::new(client_reader), handle)
}

async fn send_request(writer: &mut tokio::io::DuplexStream, request: &serde_json::Value) {
    let line = format!("{}\n", serde_json::to_string(request).expect("serialize"));
    writer.write_all(line.as_bytes()).await.expect("write line");
    writer.flush().await.expect("flush line");
}

async fn read_line_json(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read line");
    assert!(!line.is_empty(), "expected a JSONL line");
    serde_json::from_str(&line).expect("parse json")
}

async fn read_response(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    loop {
        let value = read_line_json(reader).await;
        if value.get("id").is_some() {
            return value;
        }
    }
}

#[tokio::test]
#[ignore = "Phase 1 red-ok server surface E2E suite"]
async fn runtime_backed_ingress_red_ok_rpc_session_create_and_turn_start_roundtrip() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session/create",
            "params": { "prompt": "hello" }
        }),
    )
    .await;
    let create = read_response(&mut reader).await;
    assert!(create["error"].is_null(), "session/create failed: {create}");
    let session_id = create["result"]["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/external_event",
            "params": {
                "session_id": session_id,
                "payload": { "alert": "runtime-backed rpc external event" },
                "source": "phase8"
            }
        }),
    )
    .await;
    let external_event = read_response(&mut reader).await;
    assert!(
        external_event["error"].is_null(),
        "session/external_event failed: {external_event}"
    );
    assert_eq!(external_event["result"]["outcome_type"], "accepted");

    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "turn/start",
            "params": { "session_id": session_id, "prompt": "continue" }
        }),
    )
    .await;
    let turn = read_response(&mut reader).await;
    assert!(turn["error"].is_null(), "turn/start failed: {turn}");

    drop(writer);
    server_handle
        .await
        .expect("join rpc server")
        .expect("rpc server");
}
