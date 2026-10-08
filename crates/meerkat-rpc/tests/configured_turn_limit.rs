#![allow(clippy::large_futures, clippy::unwrap_used, clippy::expect_used)]

//! No-provider regression for the production RPC config-store and realm path.

use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures::{Stream, stream};
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{
    Config, ConfigError, ConfigRuntime, ConfigStore, MemoryConfigStore, Message, Provider,
    RealmConfigSource, StopReason, TurnUsage, Usage,
};
use meerkat_rpc::{server::RpcServer, session_runtime::SessionRuntime};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

struct EmptyRealmSource;

#[async_trait]
impl RealmConfigSource for EmptyRealmSource {
    async fn config_for_realm(
        &self,
        _realm: &meerkat_core::connection::RealmId,
    ) -> Result<Option<Config>, ConfigError> {
        Ok(None)
    }
}

struct SyntheticClient {
    tool_responses: usize,
    calls: AtomicUsize,
}

#[async_trait]
impl LlmClient for SyntheticClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst);
        let (event, stop_reason) = if ordinal < self.tool_responses {
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "tool_catalog_search")
            );
            (
                LlmEvent::ToolCallComplete {
                    id: format!("synthetic-call-{ordinal}"),
                    name: "tool_catalog_search".into(),
                    args: json!({"query": "synthetic"}),
                    meta: None,
                },
                StopReason::ToolUse,
            )
        } else {
            (
                LlmEvent::TextDelta {
                    delta: "SYNTHETIC_OK".into(),
                    meta: None,
                },
                StopReason::EndTurn,
            )
        };
        Box::pin(stream::iter([
            Ok(event),
            Ok(LlmEvent::UsageUpdate {
                usage: TurnUsage::host_declared(
                    Provider::Gemini,
                    &request.model,
                    Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        ..Usage::default()
                    },
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn request(
    writer: &mut DuplexStream,
    reader: &mut BufReader<DuplexStream>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(!line.is_empty(), "RPC server ended before response");
        let response: Value = serde_json::from_str(&line).unwrap();
        if response["id"] == id {
            return response;
        }
        assert!(
            response.get("id").is_none(),
            "unexpected callback: {response}"
        );
    }
}

async fn configured_rpc_run(limit: u32, tool_responses: usize) -> (Value, usize) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::default();
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    let runtime = SessionRuntime::new_with_config_store(
        AgentFactory::new(temp.path().join("sessions")),
        config,
        Arc::clone(&config_store),
        1,
        meerkat::PersistenceBundle::new(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .unwrap(),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    // The executable always attaches a realm source. Even an isolated realm
    // composes its live head config through Config::merge before agent build.
    runtime.set_realm_config_source(Arc::new(EmptyRealmSource));
    let client = Arc::new(SyntheticClient {
        tool_responses,
        calls: AtomicUsize::new(0),
    });
    runtime.set_default_llm_client(Some(client.clone()));
    let (server_reader, mut writer) = tokio::io::duplex(4096);
    let (reader, server_writer) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let _temp = temp;
        RpcServer::new(
            BufReader::new(server_reader),
            server_writer,
            Arc::new(runtime),
            config_store,
        )
        .unwrap()
        .run()
        .await
    });
    let mut reader = BufReader::new(reader);
    let initialized = request(&mut writer, &mut reader, 1, "initialize", json!({})).await;
    assert!(initialized["error"].is_null(), "{initialized}");
    let patched = request(
        &mut writer,
        &mut reader,
        2,
        "config/patch",
        json!({"agent": {"max_turns": limit}}),
    )
    .await;
    assert!(patched["error"].is_null(), "{patched}");
    let readback = request(&mut writer, &mut reader, 3, "config/get", json!({})).await;
    assert_eq!(readback["result"]["config"]["agent"]["max_turns"], limit);
    let registered = request(
        &mut writer,
        &mut reader,
        4,
        "tools/register",
        json!({"tools": [
            {"name": "synthetic_one", "description": "Synthetic no-effect fixture", "input_schema": {"type": "object"}},
            {"name": "synthetic_two", "description": "Synthetic no-effect fixture", "input_schema": {"type": "object"}}
        ]}),
    )
    .await;
    assert!(registered["error"].is_null(), "{registered}");
    let created = request(
        &mut writer,
        &mut reader,
        5,
        "session/create",
        json!({
            "prompt": "Synthetic turn-limit regression",
            "initial_turn": "deferred",
            "model": "gemini-3.5-flash",
            "enable_builtins": false,
            "enable_shell": false,
            "enable_memory": false,
            "enable_mob": false
        }),
    )
    .await;
    assert!(created["error"].is_null(), "{created}");
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    let result = request(
        &mut writer,
        &mut reader,
        6,
        "turn/start",
        json!({"session_id": created["result"]["session_id"], "prompt": "Run synthetic fixture"}),
    )
    .await;
    drop(writer);
    server.await.unwrap().unwrap();
    (result, client.calls.load(Ordering::SeqCst))
}

#[tokio::test]
async fn realm_rpc_configured_turn_limits_reach_the_agent_loop() {
    for limit in [0, 2, 4] {
        let (response, calls) = configured_rpc_run(limit, 2).await;
        if limit < 3 {
            assert_eq!(
                calls, limit as usize,
                "configured limit {limit}: {response}"
            );
            assert_eq!(response["error"]["code"], -32603);
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("turn limit reached")
            );
        } else {
            assert!(response["error"].is_null(), "{response}");
            assert_eq!(calls, 3);
            assert_eq!(response["result"]["turns"], 3);
            assert_eq!(response["result"]["text"], "SYNTHETIC_OK");
            assert_eq!(
                response["result"]["request_usage"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
        }
    }
}

#[tokio::test]
async fn realm_rpc_configured_turn_limit_can_exceed_the_default() {
    let (response, calls) = configured_rpc_run(102, 100).await;
    assert!(response["error"].is_null(), "{response}");
    assert_eq!(calls, 101);
    assert_eq!(response["result"]["turns"], 101);
    assert_eq!(response["result"]["text"], "SYNTHETIC_OK");
    assert_eq!(
        response["result"]["request_usage"]
            .as_array()
            .unwrap()
            .len(),
        101
    );
    assert_eq!(response["result"]["run_usage"]["input_tokens"], 101);
    assert_eq!(response["result"]["run_usage"]["output_tokens"], 101);
    assert_eq!(response["result"]["run_usage"], response["result"]["usage"]);
}
