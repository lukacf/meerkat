//! Router parse tests for `realtime/*` target payloads.
//!
//! Guards the typed truth of `target.type`: only `session_target` is accepted.
//! Any deprecated or unknown tag must produce JSON-RPC `INVALID_PARAMS` with
//! a message that mentions the offending value.

#![cfg(not(feature = "mini-surface"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{BlobStore, Config, ConfigRuntime, MemoryConfigStore, StopReason};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_store::MemoryBlobStore;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        Box::pin(stream::iter(vec![Ok(meerkat_client::LlmEvent::Done {
            outcome: meerkat_client::LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn,
            },
        })]))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn spawn_server() -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(store, None, blob_store),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
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

    (client_writer, BufReader::new(client_reader), server_handle)
}

async fn send(writer: &mut tokio::io::DuplexStream, request: &serde_json::Value) {
    let line = format!("{}\n", serde_json::to_string(request).unwrap());
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

async fn read_response(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(!line.is_empty(), "expected JSONL line but got EOF");
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        if value.get("id").is_some() {
            return value;
        }
    }
}

/// `realtime/open_info` with the removed `mob_member_target` tag must be
/// rejected with `INVALID_PARAMS`; it must NOT be silently accepted.
#[tokio::test]
async fn realtime_open_info_rejects_mob_member_target() {
    let (mut writer, mut reader, server_handle) = spawn_server();

    send(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    )
    .await;
    let init = read_response(&mut reader).await;
    assert!(init["error"].is_null(), "initialize failed: {init}");

    send(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "realtime/open_info",
            "params": {
                "target": {"type": "mob_member_target"},
                "role": "primary",
                "turning_mode": "provider_managed"
            }
        }),
    )
    .await;
    let response = read_response(&mut reader).await;
    assert_eq!(response["id"], 2);
    let error = response["error"]
        .as_object()
        .unwrap_or_else(|| panic!("expected error response, got: {response}"));
    assert_eq!(error["code"].as_i64(), Some(-32602), "response: {response}");
    let message = error["message"].as_str().unwrap_or("");
    assert!(
        message.contains("unsupported target.type"),
        "message should mention unsupported target.type, got: {message}"
    );
    assert!(
        message.contains("mob_member_target"),
        "message should echo the offending tag, got: {message}"
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}
