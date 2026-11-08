#![cfg(feature = "mob")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! `mob/stream_open` resumes a member's event stream after a cursor.
//!
//! A client that held a member stream open over a connection and reconnects
//! continues where it left off: each member `mob/stream_event` names the
//! sequence space (`epoch`) of its envelope's `seq`, and `mob/stream_open`
//! with `cursor: {epoch, seq}` replays the events the member retained after
//! that position, then streams live. The engine resumes the same way in Rust
//! (`MobHandle::subscribe_agent_events_from` with
//! `SessionEventCursor::After {epoch, seq}`), which the RPC surface wires
//! through.
//!
//! The provider is scripted; the RPC router, TCP connections, mob, member
//! session and its event stream are real.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::comms::{SessionEventCursor, SessionEventEpoch};
use meerkat_core::{BlobStore, Config, ConfigRuntime, ConfigStore, MemoryConfigStore, StopReason};
use meerkat_rpc::server::serve_tcp_connection;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const LIMIT: Duration = Duration::from_secs(10);
const MOB: &str = "stream_resume_mob";
const MEMBER: &str = "w1";

/// Answers every request with two text deltas and an end of turn, so each
/// member turn publishes several events.
struct TwoDeltaProvider;

#[async_trait]
impl LlmClient for TwoDeltaProvider {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "first half ".into(),
                meta: None,
            }),
            Ok(LlmEvent::TextDelta {
                delta: "second half".into(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Anthropic,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
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

/// One JSON-RPC client over a real TCP connection.
struct Connection {
    writer: OwnedWriteHalf,
    frames: mpsc::UnboundedReceiver<Value>,
    reader_task: JoinHandle<()>,
    next_id: u64,
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

    /// The response to `method`, with every notification that arrived before
    /// it handed to `notifications`.
    async fn call(&mut self, method: &str, params: Value, notifications: &mut Vec<Value>) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        tokio::time::timeout(LIMIT, async {
            loop {
                let frame = self.frames.recv().await.expect("connection open");
                if frame["id"] == json!(id) {
                    return frame;
                }
                notifications.push(frame);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("bounded wait for the {method} response"))
    }

    /// The successful result of `method`.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        notifications: &mut Vec<Value>,
    ) -> Value {
        let response = self.call(method, params, notifications).await;
        assert!(response["error"].is_null(), "{method} failed: {response}");
        response["result"].clone()
    }

    /// The next error response, whatever its id.
    async fn next_error(&mut self) -> Value {
        tokio::time::timeout(LIMIT, async {
            loop {
                let frame = self.frames.recv().await.expect("connection open");
                if !frame["error"].is_null() {
                    return frame;
                }
            }
        })
        .await
        .expect("bounded wait for an error response")
    }

    /// Every frame that arrives within `quiet` of the previous one.
    async fn drain(&mut self, quiet: Duration) -> Vec<Value> {
        let mut frames = Vec::new();
        while let Ok(Some(frame)) = tokio::time::timeout(quiet, self.frames.recv()).await {
            frames.push(frame);
        }
        frames
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    runtime: Arc<SessionRuntime>,
    config: Arc<dyn ConfigStore>,
    servers: Vec<JoinHandle<Result<(), meerkat_rpc::server::ServerError>>>,
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let blobs: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let runtime = SessionRuntime::new(
            AgentFactory::new(temp.path().join("sessions")),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(
                Arc::new(meerkat::MemoryStore::new()),
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                blobs,
            )
            .expect("construct runtime authority"),
            meerkat_rpc::router::NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        runtime.set_default_llm_client(Some(Arc::new(TwoDeltaProvider)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            config.clone(),
            temp.path().join("config_state.json"),
        )));
        Self {
            _temp: temp,
            runtime: Arc::new(runtime),
            config,
            servers: Vec::new(),
        }
    }

    /// A new client connection on the shared runtime, initialized.
    async fn connect(&mut self) -> Connection {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        self.servers.push(tokio::spawn(serve_tcp_connection(
            accepted,
            self.runtime.clone(),
            self.config.clone(),
            None,
        )));
        let (reader, writer) = stream.into_split();
        let (tx, frames) = mpsc::unbounded_channel();
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut connection = Connection {
            writer,
            frames,
            reader_task,
            next_id: 0,
        };
        connection
            .request("initialize", json!({}), &mut Vec::new())
            .await;
        connection
    }

    async fn clean(self) {
        for server in self.servers {
            server.abort();
            let _ = server.await;
        }
        tokio::time::timeout(LIMIT, self.runtime.try_shutdown())
            .await
            .expect("bounded runtime cleanup")
            .unwrap();
    }
}

/// Creates the mob and spawns its turn-driven member over `connection`.
async fn create_mob_with_member(connection: &mut Connection) {
    let mut ignored = Vec::new();
    connection
        .request(
            "mob/create",
            json!({"definition":{"id":MOB,"profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
            &mut ignored,
        )
        .await;
    connection
        .request(
            "mob/spawn",
            json!({"mob_id":MOB,"profile":"worker","agent_identity":MEMBER,"runtime_mode":"turn_driven"}),
            &mut ignored,
        )
        .await;
}

/// Runs one member turn over `connection` and returns the notifications that
/// arrived before its response, followed by those that trail it.
async fn member_turn(connection: &mut Connection, prompt: &str) -> Vec<Value> {
    let mut notifications = Vec::new();
    connection
        .request(
            "mob/turn_start",
            json!({"mob_id":MOB,"agent_identity":MEMBER,"prompt":prompt}),
            &mut notifications,
        )
        .await;
    notifications.extend(connection.drain(Duration::from_millis(500)).await);
    notifications
}

/// The `mob/stream_event` notifications of `stream_id` among `frames`.
fn stream_events<'a>(frames: &'a [Value], stream_id: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| {
            frame["method"] == "mob/stream_event" && frame["params"]["stream_id"] == stream_id
        })
        .collect()
}

/// The envelope sequence (`event.seq`) of each stream event.
fn envelope_seqs(events: &[&Value]) -> Vec<u64> {
    events
        .iter()
        .map(|event| {
            event["params"]["event"]["seq"].as_u64().unwrap_or_else(|| {
                panic!("a member stream event carries its envelope seq: {event}")
            })
        })
        .collect()
}

/// Whether a stream event's payload is the end of a run.
fn is_run_completed(event: &Value) -> bool {
    event["params"]["event"]["payload"]["type"] == "run_completed"
}

/// A client that reconnects resumes a member stream after the last event it
/// saw. Client A watches turn 1 and records the position of its last event;
/// it disconnects; turn 2 runs while no stream is open; client B opens the
/// member stream with a cursor after A's position and receives exactly turn
/// 2's events (no gap, no duplicate), then turn 3's live.
#[tokio::test]
async fn mob_stream_open_resumes_after_a_cursor_across_connections() {
    let mut h = Harness::new();
    let mut a = h.connect().await;
    create_mob_with_member(&mut a).await;

    let mut opened = Vec::new();
    let stream_a = a
        .request(
            "mob/stream_open",
            json!({"mob_id":MOB,"agent_identity":MEMBER}),
            &mut opened,
        )
        .await["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn_1 = member_turn(&mut a, "turn one").await;
    let seen_by_a = stream_events(&turn_1, &stream_a);
    // Setup took effect: A's stream carried turn 1 through its end.
    assert!(
        seen_by_a.iter().any(|event| is_run_completed(event)),
        "A's stream carries turn 1 to run_completed: {turn_1:?}"
    );
    let last = seen_by_a.last().unwrap();
    let last_seq = last["params"]["event"]["seq"].as_u64().unwrap();
    let epoch = last["params"]["epoch"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("mob/stream_event names the sequence space of its position: {last}")
        })
        .to_owned();
    drop(a);

    let mut b = h.connect().await;
    let turn_2 = member_turn(&mut b, "turn two").await;
    assert!(
        stream_events(&turn_2, &stream_a).is_empty(),
        "A's stream is gone with A: {turn_2:?}"
    );

    let mut replayed = Vec::new();
    let stream_b = b
        .request(
            "mob/stream_open",
            json!({"mob_id":MOB,"agent_identity":MEMBER,"cursor":{"epoch":epoch,"seq":last_seq}}),
            &mut replayed,
        )
        .await["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();
    replayed.extend(b.drain(Duration::from_millis(500)).await);
    replayed.extend(member_turn(&mut b, "turn three").await);
    let resumed = stream_events(&replayed, &stream_b);
    let seqs = envelope_seqs(&resumed);
    assert_eq!(
        seqs.first(),
        Some(&(last_seq + 1)),
        "the resumed stream starts right after A's last event: {seqs:?}"
    );
    assert!(
        seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "the resumed stream has no gap and no duplicate: {seqs:?}"
    );
    assert_eq!(
        resumed
            .iter()
            .filter(|event| is_run_completed(event))
            .count(),
        2,
        "the resumed stream carries turn 2 (replayed) and turn 3 (live): {seqs:?}"
    );
    h.clean().await;
}

/// A cursor the member's current sequence space cannot resume from replays
/// that space from its earliest retained event, and every replayed event
/// names the current epoch: a cursor of another epoch (one a restart or a
/// new host binding replaced) and a cursor ahead of the space's newest event.
#[tokio::test]
async fn mob_stream_open_replays_the_current_space_for_a_cursor_it_cannot_resume() {
    let mut h = Harness::new();
    let mut a = h.connect().await;
    create_mob_with_member(&mut a).await;
    member_turn(&mut a, "turn one").await;

    // The current space and every event it retains, read in Rust.
    let mob_state = h.runtime.mob_state().expect("the RPC mob state");
    let earliest = mob_state
        .subscribe_agent_events_from(
            &meerkat_mob::MobId::from(MOB),
            &meerkat_mob::AgentIdentity::from(MEMBER),
            SessionEventCursor::Earliest,
        )
        .await
        .expect("earliest subscription");
    let current = earliest
        .epoch
        .expect("the member's stream names its sequence space");
    let mut retained_stream = earliest.stream;
    let mut retained = Vec::new();
    while let Ok(Some(envelope)) =
        tokio::time::timeout(Duration::from_millis(300), retained_stream.next()).await
    {
        retained.push(envelope.seq);
    }
    drop(retained_stream);
    // Setup took effect: turn 1's events are retained.
    assert!(
        retained.len() > 1,
        "turn 1's events are retained: {retained:?}"
    );
    let newest = *retained.last().unwrap();

    let stale = SessionEventEpoch::new();
    assert_ne!(stale, current, "setup: a fresh epoch names another space");
    for (cursor, what) in [
        (
            json!({"epoch": stale.to_string(), "seq": newest}),
            "a cursor of another epoch",
        ),
        (
            json!({"epoch": current.to_string(), "seq": newest + 1000}),
            "a cursor ahead of the space's newest event",
        ),
    ] {
        let mut opened = Vec::new();
        let stream = a
            .request(
                "mob/stream_open",
                json!({"mob_id":MOB,"agent_identity":MEMBER,"cursor":cursor}),
                &mut opened,
            )
            .await["stream_id"]
            .as_str()
            .unwrap()
            .to_owned();
        opened.extend(a.drain(Duration::from_millis(500)).await);
        let replayed = stream_events(&opened, &stream);
        assert_eq!(
            envelope_seqs(&replayed),
            retained,
            "{what} replays the current space from its earliest retained event"
        );
        assert!(
            replayed
                .iter()
                .all(|event| event["params"]["epoch"] == json!(current.to_string())),
            "{what}: every replayed event names the current epoch {current}: {replayed:?}"
        );
        a.request(
            "mob/stream_close",
            json!({"stream_id": stream}),
            &mut Vec::new(),
        )
        .await;
    }
    h.clean().await;
}

/// A cursor names one member's sequence space: a mob-wide stream refuses
/// one, and a member stream refuses a cursor whose epoch is not a UUID, both
/// as invalid params answered under the request's id. A mob-wide stream's
/// events come from several members' spaces and name no epoch.
#[tokio::test]
async fn mob_stream_open_refuses_a_cursor_it_cannot_place() {
    let mut h = Harness::new();
    let mut a = h.connect().await;
    create_mob_with_member(&mut a).await;
    for (id, params, what) in [
        (
            100,
            json!({"mob_id":MOB,"cursor":{"epoch":SessionEventEpoch::new().to_string(),"seq":1}}),
            "a cursor on a mob-wide stream",
        ),
        (
            101,
            json!({"mob_id":MOB,"agent_identity":MEMBER,"cursor":{"epoch":"not-a-uuid","seq":1}}),
            "a cursor epoch that is not a UUID",
        ),
    ] {
        a.send(json!({"jsonrpc":"2.0","id":id,"method":"mob/stream_open","params":params}))
            .await;
        let refused = a.next_error().await;
        assert_eq!(
            refused["error"]["code"],
            json!(-32602),
            "{what} is invalid params: {refused}"
        );
        assert_eq!(refused["id"], json!(id), "{what}: {refused}");
        assert!(
            refused["result"].is_null(),
            "{what} opens nothing: {refused}"
        );
    }

    let mut ignored = Vec::new();
    let mob_wide = a
        .request("mob/stream_open", json!({"mob_id":MOB}), &mut ignored)
        .await["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let turn = member_turn(&mut a, "turn one").await;
    let events = stream_events(&turn, &mob_wide);
    assert!(
        events.iter().any(|event| {
            event["params"]["event"]["envelope"]["payload"]["type"] == "run_completed"
        }),
        "setup: the mob-wide stream carries the member's turn: {turn:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| event["params"].get("epoch").is_none()),
        "a mob-wide event names no epoch: {events:?}"
    );
    h.clean().await;
}

/// The engine already resumes: on the same member and runtime, the Rust
/// `subscribe_agent_events_from(.., After {epoch, seq})` replays exactly the
/// events after a position a previous subscription observed, across a turn
/// that ran while no subscription was open, then continues live. The ask is
/// wiring this through `mob/stream_open`, not a new capability.
#[tokio::test]
async fn rust_subscribe_agent_events_from_replays_the_tail_after_a_cursor() {
    let mut h = Harness::new();
    let mut a = h.connect().await;
    create_mob_with_member(&mut a).await;
    let mob_state = h.runtime.mob_state().expect("the RPC mob state");
    let mob_id = meerkat_mob::MobId::from(MOB);
    let member = meerkat_mob::AgentIdentity::from(MEMBER);

    let first = mob_state
        .subscribe_agent_events_from(&mob_id, &member, SessionEventCursor::Live)
        .await
        .expect("live subscription");
    let epoch = first
        .epoch
        .expect("the member's stream names its sequence space");
    let mut first_stream = first.stream;
    member_turn(&mut a, "turn one").await;
    let last_seq = loop {
        let envelope = tokio::time::timeout(LIMIT, first_stream.next())
            .await
            .expect("turn 1 events")
            .expect("stream open");
        if matches!(
            envelope.payload,
            meerkat_core::AgentEvent::RunCompleted { .. }
        ) {
            break envelope.seq;
        }
    };
    drop(first_stream);

    member_turn(&mut a, "turn two").await;

    let resumed = mob_state
        .subscribe_agent_events_from(
            &mob_id,
            &member,
            SessionEventCursor::After {
                epoch,
                seq: last_seq,
            },
        )
        .await
        .expect("resumed subscription");
    assert_eq!(resumed.epoch, Some(epoch), "same sequence space");
    let mut resumed_stream = resumed.stream;
    let mut seqs = Vec::new();
    let mut runs_completed = 0;
    let mut turn_3_started = false;
    while runs_completed < 2 {
        if runs_completed == 1 && !turn_3_started {
            // Turn 2 is fully replayed; turn 3 must arrive live.
            member_turn(&mut a, "turn three").await;
            turn_3_started = true;
        }
        let envelope = tokio::time::timeout(LIMIT, resumed_stream.next())
            .await
            .expect("replayed and live events")
            .expect("stream open");
        assert!(
            !matches!(
                envelope.payload,
                meerkat_core::AgentEvent::StreamTruncated { .. }
            ),
            "the retained window covers the gap"
        );
        seqs.push(envelope.seq);
        if matches!(
            envelope.payload,
            meerkat_core::AgentEvent::RunCompleted { .. }
        ) {
            runs_completed += 1;
        }
    }
    assert_eq!(
        seqs.first(),
        Some(&(last_seq + 1)),
        "replay starts right after the cursor: {seqs:?}"
    );
    assert!(
        seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "replay then live, no gap and no duplicate: {seqs:?}"
    );
    drop(a);
    h.clean().await;
}
