#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]

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
                delta: format!("runtime-backed [{model}]"),
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
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
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

fn spawn_test_server() -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().expect("tempdir");
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
        ),
        meerkat_rpc::router::NotificationSink::noop(),
    );
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
                "kind": "generic_json",
                "event_type": "phase8",
                "payload": { "alert": "runtime-backed rpc external event" }
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

#[cfg(feature = "mob")]
#[tokio::test]
async fn runtime_skill_refs_reach_canonical_history_and_provider_bytes() {
    use axum::extract::State;
    use axum::response::{Sse, sse::Event};
    use axum::routing::post;
    use meerkat_core::service::SessionHistoryQuery;
    use meerkat_core::skills::{SkillKey, SkillName, SkillRef, SourceUuid};
    use meerkat_core::{ContentBlock, Message};
    use serde_json::{Value, json};
    use std::convert::Infallible;
    use std::sync::Mutex;
    use std::time::Duration;

    const MODEL: &str = "claude-sonnet-4-6";
    const BODY: &str = "SYNTHETIC_SKILL_BODY_START\nPreserve this complete harmless body.\nSYNTHETIC_SKILL_BODY_END";
    const SECOND_BODY: &str =
        "SECOND_SKILL_BODY_START\nRetain canonical source identity.\nSECOND_SKILL_BODY_END";

    assert!(
        !meerkat_models::capabilities_for(meerkat_core::Provider::Anthropic, MODEL)
            .unwrap()
            .supports_mid_conversation_system_messages,
        "continued skill invocation must work without nonleading System support",
    );

    async fn provider(
        State(requests): State<Arc<Mutex<Vec<Vec<u8>>>>>,
        body: axum::body::Bytes,
    ) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
        requests.lock().unwrap().push(body.to_vec());
        let events = [
            json!({"type":"message_start","message":{"id":"msg-local","type":"message","role":"assistant","model":MODEL,"content":[],"usage":{"input_tokens":10,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"SYNTHETIC_OK"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
            json!({"type":"message_stop"}),
        ];
        Sse::new(stream::iter(events.into_iter().map(|value| {
            Ok(Event::default()
                .event(value["type"].as_str().unwrap())
                .json_data(value)
                .unwrap())
        })))
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        let directory = tempfile::Builder::new()
            .prefix(".runtime-skills-")
            .tempdir_in(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .unwrap();
        let names = ["runtime-invoke-regression", "runtime-second-regression"];
        for (name, body) in names.iter().zip([BODY, SECOND_BODY]) {
            let path = directory.path().join(".rkat/skills").join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Synthetic local regression skill.\n---\n\n{body}\n"),
            )
            .unwrap();
        }
        let keys = names.map(|name| SkillKey::new(
            SourceUuid::project_local(),
            SkillName::parse(name).unwrap(),
        ));
        let refs = keys.clone().map(SkillRef::Structured);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route("/v1/messages", post(provider))
            .with_state(requests.clone());
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let provider_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async { let _ = stop_rx.await; })
                .await
                .unwrap();
        });
        let client = meerkat_anthropic::AnthropicClient::builder("synthetic-local-key".into())
            .base_url(base_url)
            .request_timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let factory = AgentFactory::new(directory.path().join("sessions"))
            .context_root(directory.path())
            .user_config_root(directory.path().join("user"))
            .project_root(directory.path())
            .memory(false);
        let config = Config::default();
        let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(MemoryConfigStore::new(
            config.clone(), meerkat_models::canonical(),
        ));
        let runtime = Arc::new(SessionRuntime::new(
            factory,
            config,
            4,
            meerkat::PersistenceBundle::new(
                Arc::new(meerkat::MemoryStore::new()),
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                Arc::new(MemoryBlobStore::new()),
            ),
            meerkat_rpc::router::NotificationSink::noop(),
        ));
        runtime.set_default_llm_client(Some(Arc::new(client)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            config_store.clone(), directory.path().join("config-state.json"),
        )));
        let (server_reader, mut writer) = tokio::io::duplex(65536);
        let (reader, server_writer) = tokio::io::duplex(65536);
        let mut reader = BufReader::new(reader);
        let server_runtime = runtime.clone();
        let rpc_task = tokio::spawn(async move {
            RpcServer::new(BufReader::new(server_reader), server_writer, server_runtime, config_store)
                .run().await.unwrap();
        });

        send_request(&mut writer, &json!({
            "jsonrpc":"2.0", "id":1, "method":"session/create",
            "params":{"model":MODEL, "prompt":"First harmless prompt.", "skill_refs":[refs[0]],
                "injected_context":["First ambient context."]}
        })).await;
        let created = read_response(&mut reader).await;
        assert!(created["error"].is_null(), "{created}");
        let session_id = created["result"]["session_id"].as_str().unwrap().to_owned();
        let mut history_prefix = Vec::new();
        for turn in 0..3 {
            if turn > 0 {
                let mut params = json!({"session_id":session_id,
                    "prompt": if turn == 1 { "Second harmless prompt." } else { "No activation prompt." },
                    "injected_context":["Later ambient context."]});
                if turn == 1 {
                    params["skill_refs"] = json!(refs);
                }
                send_request(&mut writer, &json!({
                    "jsonrpc":"2.0", "id":turn + 1, "method":"turn/start", "params":params
                })).await;
                let response = read_response(&mut reader).await;
                assert!(response["error"].is_null(), "{response}");
            }
            let history = runtime.session_service().read_history(
                &meerkat_core::SessionId::parse(&session_id).unwrap(), SessionHistoryQuery::default(),
            ).await.unwrap();
            assert_eq!(&history.messages[..history_prefix.len()], history_prefix);
            let users: Vec<_> = history.messages.iter().filter_map(|message| {
                if let Message::User(user) = message { Some(user) } else { None }
            }).collect();
            assert_eq!(users.len(), (turn + 1) * 2, "ambient and conversational rows remain separate");
            let mut activations = Vec::new();
            for user in &users {
                for block in &user.content {
                    if let ContentBlock::SkillContext { skill_key, text } = block {
                        assert!(user.transcript_role.is_conversational());
                        activations.push((skill_key, text));
                    }
                }
            }
            let expected = if turn == 0 { vec![&keys[0]] } else { vec![&keys[0], &keys[0], &keys[1]] };
            assert_eq!(activations.iter().map(|(key, _)| *key).collect::<Vec<_>>(), expected);
            for (key, text) in &activations {
                let body = if *key == &keys[0] { BODY } else { SECOND_BODY };
                assert_eq!(text.as_str(), format!(
                    "<skill source_uuid=\"{}\" skill_name=\"{}\">\n{body}\n\n</skill>",
                    key.source_uuid, key.skill_name,
                ));
            }
            let request: Value = {
                let captured = requests.lock().unwrap();
                assert_eq!(captured.len(), turn + 1, "one actual provider request per turn");
                serde_json::from_slice(&captured[turn]).unwrap()
            };
            let projected_text: String = request["messages"].as_array().unwrap().iter()
                .flat_map(|message| {
                    let content = &message["content"];
                    content.as_str().into_iter().chain(
                        content.as_array().into_iter().flatten()
                            .filter_map(|block| block["text"].as_str())
                    )
                })
                .collect::<Vec<_>>().join("\n");
            assert_eq!(projected_text.matches(BODY).count(), if turn == 0 { 1 } else { 2 });
            assert_eq!(projected_text.matches(SECOND_BODY).count(), usize::from(turn > 0));
            for (_, body) in activations {
                assert!(projected_text.contains(body), "provider bytes must include full canonical rendered context");
            }
            assert!(!request["system"].to_string().contains("SYNTHETIC_SKILL_BODY_START"),
                "per-turn activation is user context, never nonleading System");
            let systems = history.messages.iter().filter(|message| matches!(message, Message::System(_))).count();
            assert_eq!(systems, 1, "activation must not add System rows");
            history_prefix = history.messages;
        }
        drop(writer);
        rpc_task.await.unwrap();
        let _ = stop_tx.send(());
        provider_task.await.unwrap();
    }).await.expect("bounded loopback RPC skill regression");
}
