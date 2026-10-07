//! Assistant message identity across the full in-process RPC stack.
//!
//! A console pairs live `session/event` rows with `session/history` rows by
//! `assistant_message_id` alone. This drives a runtime-backed RPC session
//! whose scripted provider streams through the real `LlmClientAdapter`, and
//! proves the join holds without text or rank matching:
//!
//! - three turns answer with byte-identical text, yet every committed
//!   `block_assistant` row has its own id;
//! - a turn parked mid-stream has already announced the id its partial
//!   chunk belongs to, and commits exactly that id; a partial history page
//!   carries its rows' ids (history reads wait for an in-flight turn on this
//!   surface, so any reordering against live chunks is client-side, and the
//!   join is order-independent);
//! - each history row id matches exactly one `turn_started`..`turn_completed`
//!   group, its deltas, and the `run_completed` whose result repeats it.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat::{AgentFactory, Config};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{ConfigRuntime, MemoryConfigStore, Message, StopReason};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const WAIT: Duration = Duration::from_secs(60);
const ANSWER: &str = "same";

#[derive(Default)]
struct Hold {
    reached: AtomicBool,
    released: AtomicBool,
}

/// Every turn answers "same". The third call streams "sa", parks until the
/// test releases it, then streams "me".
struct IdenticalAnswers {
    calls: AtomicUsize,
    hold: Arc<Hold>,
}

fn done(model: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                model,
                meerkat_core::Usage::default(),
            ),
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn,
            },
        },
    ]
}

fn text(delta: &str) -> LlmEvent {
    LlmEvent::TextDelta {
        delta: delta.to_string(),
        meta: None,
    }
}

#[async_trait::async_trait]
impl LlmClient for IdenticalAnswers {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Key the script on the latest user prompt, not on call order.
        let rendered = format!("{:?}", request.messages);
        let call = if rendered.contains("third question") {
            2
        } else if rendered.contains("second question") {
            1
        } else {
            0
        };
        let model = request.model.clone();
        let hold = Arc::clone(&self.hold);
        let (head, tail): (Vec<LlmEvent>, Vec<LlmEvent>) = match call {
            0 => (
                vec![
                    LlmEvent::ReasoningDelta {
                        delta: "plan".to_string(),
                    },
                    LlmEvent::ReasoningComplete {
                        text: "plan".to_string(),
                        meta: None,
                    },
                    text(ANSWER),
                ],
                done(&model),
            ),
            2 => (vec![text("sa")], {
                let mut tail = vec![text("me")];
                tail.extend(done(&model));
                tail
            }),
            _ => (vec![text(ANSWER)], done(&model)),
        };
        let parks = call == 2;
        let head = futures::stream::iter(head.into_iter().map(Ok));
        let tail = futures::StreamExt::flat_map(
            futures::stream::once(async move {
                if parks {
                    hold.reached.store(true, Ordering::SeqCst);
                    while !hold.released.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                tail
            }),
            |events| futures::stream::iter(events.into_iter().map(Ok)),
        );
        Box::pin(futures::StreamExt::chain(head, tail))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct RpcClient {
    writer: tokio::io::DuplexStream,
    reader: BufReader<tokio::io::DuplexStream>,
    /// Every `session/event` payload read so far, in wire order.
    events: Vec<Value>,
    responses: BTreeMap<u64, Value>,
}

impl RpcClient {
    async fn send(&mut self, id: u64, method: &str, params: Value) {
        let line = format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        );
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn read_one(&mut self) {
        let mut line = String::new();
        tokio::time::timeout(WAIT, self.reader.read_line(&mut line))
            .await
            .expect("rpc line in time")
            .unwrap();
        assert!(!line.is_empty(), "rpc transport closed");
        let value: Value = serde_json::from_str(&line).unwrap();
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            self.responses.insert(id, value);
        } else if value["method"] == "session/event" {
            self.events
                .push(value["params"]["event"]["payload"].clone());
        }
    }

    async fn response(&mut self, id: u64) -> Value {
        while !self.responses.contains_key(&id) {
            self.read_one().await;
        }
        let response = self.responses.remove(&id).unwrap();
        assert!(response["error"].is_null(), "rpc {id} failed: {response}");
        response["result"].clone()
    }

    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(id, method, params).await;
        self.response(id).await
    }
}

fn spawn_server(client: Arc<dyn LlmClient>) -> (RpcClient, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
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
    );
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
    let (server_reader, client_writer) = tokio::io::duplex(1 << 16);
    let (client_reader, server_writer) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let mut server = RpcServer::new(
            BufReader::new(server_reader),
            server_writer,
            runtime,
            config_store,
        )
        .expect("construct runtime authority");
        let _ = server.run().await;
    });
    (
        RpcClient {
            writer: client_writer,
            reader: BufReader::new(client_reader),
            events: Vec::new(),
            responses: BTreeMap::new(),
        },
        temp,
    )
}

fn assistant_rows(history: &Value) -> Vec<(Option<String>, String)> {
    history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["role"] == "block_assistant")
        .map(|row| {
            let text = row["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["block_type"] == "text")
                .filter_map(|block| block["data"]["text"].as_str())
                .collect::<String>();
            (
                row["assistant_message_id"].as_str().map(str::to_string),
                text,
            )
        })
        .collect()
}

fn event_id(event: &Value) -> Option<String> {
    event["assistant_message_id"].as_str().map(str::to_string)
}

fn events_of<'a>(events: &'a [Value], kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    events.iter().filter(move |event| event["type"] == kind)
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_assistant_message_ids_join_live_events_to_history() {
    let hold = Arc::new(Hold::default());
    let provider = Arc::new(IdenticalAnswers {
        calls: AtomicUsize::new(0),
        hold: Arc::clone(&hold),
    });
    let (mut rpc, _temp) = spawn_server(provider);

    let created = rpc
        .call(1, "session/create", json!({"prompt": "first question"}))
        .await;
    let session_id = created["session_id"].as_str().unwrap().to_string();
    rpc.call(
        2,
        "turn/start",
        json!({"session_id": &session_id, "prompt": "second question"}),
    )
    .await;

    // The third turn parks mid-stream after its first chunk: its id is
    // announced before any delta, so a console already knows which message
    // the partial chunk belongs to.
    rpc.send(
        3,
        "turn/start",
        json!({"session_id": &session_id, "prompt": "third question"}),
    )
    .await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !hold.reached.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "third turn never parked"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    while !events_of(&rpc.events, "text_delta").any(|event| event["delta"] == "sa") {
        rpc.read_one().await;
    }
    let in_flight = events_of(&rpc.events, "turn_started")
        .last()
        .and_then(event_id)
        .expect("the parked turn announced its id before any delta");
    let partial_chunk = events_of(&rpc.events, "text_delta")
        .find(|event| event["delta"] == "sa")
        .and_then(event_id);
    assert_eq!(partial_chunk.as_deref(), Some(in_flight.as_str()));

    hold.released.store(true, Ordering::SeqCst);
    let third = rpc.response(3).await;
    assert_eq!(third["text"], ANSWER);

    // History is read before the client processes anything else; the join
    // below is by id only, so it holds whichever arrives first.
    let history = rpc
        .call(4, "session/history", json!({"session_id": &session_id}))
        .await;
    let page = rpc
        .call(
            5,
            "session/history",
            json!({"session_id": &session_id, "offset": 3, "limit": 2}),
        )
        .await;

    // Every committed row answers "same", and every row is its own message.
    let rows = assistant_rows(&history);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|(_, text)| text == ANSWER));
    let ids = rows
        .iter()
        .map(|(id, _)| id.clone().expect("every committed row carries an id"))
        .collect::<Vec<_>>();
    assert_eq!(
        ids.iter().collect::<BTreeSet<_>>().len(),
        3,
        "identical text never shares an id: {ids:?}"
    );
    assert_eq!(
        in_flight, ids[2],
        "the parked turn commits under its announced id"
    );

    // A partial page carries its rows' ids; position is irrelevant.
    let page_rows = assistant_rows(&page);
    assert_eq!(
        page_rows.len(),
        1,
        "offset 3, limit 2 holds the second answer"
    );
    assert_eq!(page_rows[0].0.as_deref(), Some(ids[1].as_str()));

    // Each history row matches exactly one live turn, joined by id alone.
    let events = rpc.events.clone();
    for id in &ids {
        let of_id = |kind| {
            events_of(&events, kind)
                .filter(|event| event_id(event).as_deref() == Some(id.as_str()))
                .count()
        };
        assert_eq!(of_id("turn_started"), 1, "{id}: one turn_started");
        assert_eq!(of_id("turn_completed"), 1, "{id}: one turn_completed");
        assert_eq!(of_id("text_complete"), 1, "{id}: one text_complete");
        let streamed = events_of(&events, "text_delta")
            .filter(|event| event_id(event).as_deref() == Some(id.as_str()))
            .filter_map(|event| event["delta"].as_str())
            .collect::<String>();
        assert_eq!(streamed, ANSWER, "{id}: its deltas rebuild its row");
    }
    for kind in ["reasoning_delta", "reasoning_complete"] {
        assert!(events_of(&events, kind).count() > 0, "{kind} streamed");
        assert!(
            events_of(&events, kind).all(|event| event_id(event).as_deref() == Some(&ids[0])),
            "{kind} belongs to the first message"
        );
    }
    let run_completed = events_of(&events, "run_completed")
        .map(event_id)
        .collect::<Vec<_>>();
    assert_eq!(
        run_completed,
        ids.iter().cloned().map(Some).collect::<Vec<_>>(),
        "each run_completed names the row its result repeats"
    );
    let known = ids.iter().collect::<BTreeSet<_>>();
    assert!(
        events
            .iter()
            .filter_map(event_id)
            .all(|id| known.contains(&id)),
        "no live event names a message that history does not hold"
    );
}
