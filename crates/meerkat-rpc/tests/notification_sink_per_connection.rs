#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! `session/event` notifications follow the last connection, not the one
//! that started the turn.
//!
//! `SessionRuntime` holds one process-wide `NotificationSink`. Every
//! `MethodRouter::new` (one per accepted connection on a shared runtime, as
//! `serve_tcp_with_options_until` and `serve_tcp_connection` build them)
//! replaces it, and a session's runtime executor reads the current one when
//! it applies a turn. With two connections open, the `session/event`
//! notifications of a turn connection A started go to connection B; once B
//! closes, they go nowhere, while A is still connected.
//!
//! The provider is scripted; the RPC routers, TCP connections, session
//! runtime and executors are real.

use async_trait::async_trait;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
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
/// How long a connection is watched for trailing notifications.
const QUIET: Duration = Duration::from_millis(500);

/// Answers every request with a text delta and an end of turn.
struct TextProvider;

#[async_trait]
impl LlmClient for TextProvider {
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
                delta: "answer".into(),
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

/// One JSON-RPC client over a real TCP connection; keeps every
/// notification it receives.
struct Connection {
    writer: OwnedWriteHalf,
    frames: mpsc::UnboundedReceiver<Value>,
    reader_task: JoinHandle<()>,
    next_id: u64,
    notifications: Vec<Value>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

impl Connection {
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.writer
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        self.writer.flush().await.unwrap();
        let response = tokio::time::timeout(LIMIT, async {
            loop {
                let frame = self.frames.recv().await.expect("connection open");
                if frame["id"] == json!(id) {
                    return frame;
                }
                self.notifications.push(frame);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("bounded wait for the {method} response"));
        assert!(response["error"].is_null(), "{method} failed: {response}");
        response["result"].clone()
    }

    /// Keeps the frames that arrive until the connection is quiet.
    async fn settle(&mut self) {
        while let Ok(Some(frame)) = tokio::time::timeout(QUIET, self.frames.recv()).await {
            self.notifications.push(frame);
        }
    }

    /// The `session/event` notifications received for `session_id`.
    fn session_events(&self, session_id: &str) -> usize {
        self.notifications
            .iter()
            .filter(|frame| {
                frame["method"] == "session/event" && frame["params"]["session_id"] == session_id
            })
            .count()
    }

    /// The methods of the notifications received so far, for failure output.
    fn methods(&self) -> Vec<&str> {
        self.notifications
            .iter()
            .filter_map(|frame| frame["method"].as_str())
            .collect()
    }

    /// Creates a session with no first turn.
    async fn create_session(&mut self) -> String {
        self.request(
            "session/create",
            json!({"prompt":"bootstrap","initial_turn":"deferred"}),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Runs one turn on `session_id` to completion and lets its trailing
    /// notifications arrive.
    async fn turn(&mut self, session_id: &str, prompt: &str) {
        self.request(
            "turn/start",
            json!({"session_id":session_id,"prompt":prompt}),
        )
        .await;
        self.settle().await;
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    runtime: Arc<SessionRuntime>,
    config: Arc<dyn ConfigStore>,
    servers: Vec<Option<JoinHandle<Result<(), meerkat_rpc::server::ServerError>>>>,
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
        runtime.set_default_llm_client(Some(Arc::new(TextProvider)));
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

    /// A new client connection on the shared runtime, initialized; its
    /// index closes it with [`Self::close`].
    async fn connect(&mut self) -> (usize, Connection) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        let index = self.servers.len();
        self.servers.push(Some(tokio::spawn(serve_tcp_connection(
            accepted,
            self.runtime.clone(),
            self.config.clone(),
            None,
        ))));
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
            notifications: Vec::new(),
        };
        connection.request("initialize", json!({})).await;
        (index, connection)
    }

    /// Closes `connection` and waits until its server has ended.
    async fn close(&mut self, index: usize, connection: Connection) {
        drop(connection);
        let server = self.servers[index].take().unwrap();
        let _ = tokio::time::timeout(LIMIT, server)
            .await
            .expect("the closed connection's server ends");
    }

    async fn clean(self) {
        for server in self.servers.into_iter().flatten() {
            server.abort();
            let _ = server.await;
        }
        tokio::time::timeout(LIMIT, self.runtime.try_shutdown())
            .await
            .expect("bounded runtime cleanup")
            .unwrap();
    }
}

/// Intended behaviour: a turn's `session/event` notifications go to the
/// connection that started it, whichever connections opened since. A's turn
/// is watched on A while B is open, and still after B closes; B never sees
/// A's session.
///
/// Ignored until the RPC host routes session events per connection; its
/// `today_` sibling below pins the current behaviour and is deleted with
/// the fix.
#[tokio::test]
#[ignore = "session/event goes to the last connected router's sink, not the turn's connection; see the PR"]
async fn session_events_go_to_the_connection_that_started_the_turn() {
    let mut h = Harness::new();
    let (_, mut a) = h.connect().await;
    let session = a.create_session().await;
    a.turn(&session, "alone").await;
    // Setup took effect: with one connection, A sees its own turn.
    assert!(a.session_events(&session) > 0, "{:?}", a.methods());
    a.notifications.clear();

    let (b_index, mut b) = h.connect().await;
    a.turn(&session, "with b open").await;
    b.settle().await;
    assert!(
        a.session_events(&session) > 0,
        "A received no session/event for its own turn while B is open \
         (A got {:?}; B, which never touched the session, got {} of them)",
        a.methods(),
        b.session_events(&session)
    );
    assert_eq!(
        b.session_events(&session),
        0,
        "A's session events reached B: {:?}",
        b.methods()
    );
    a.notifications.clear();

    h.close(b_index, b).await;
    a.turn(&session, "after b closed").await;
    assert!(
        a.session_events(&session) > 0,
        "A received no session/event for its own turn after B closed: {:?}",
        a.methods()
    );
    h.clean().await;
}

/// Current behaviour, to be deleted with the fix: once B connects, the
/// `session/event` notifications of A's turn arrive on B and none on A; once
/// B closes, they arrive nowhere, though A is still connected and its turn
/// completes.
#[tokio::test]
async fn today_session_events_follow_the_last_connection() {
    let mut h = Harness::new();
    let (_, mut a) = h.connect().await;
    let session = a.create_session().await;
    a.turn(&session, "alone").await;
    assert!(
        a.session_events(&session) > 0,
        "with one connection, A sees its own turn: {:?}",
        a.methods()
    );
    a.notifications.clear();

    let (b_index, mut b) = h.connect().await;
    a.turn(&session, "with b open").await;
    b.settle().await;
    assert_eq!(
        a.session_events(&session),
        0,
        "A's own turn: {:?}",
        a.methods()
    );
    assert!(
        b.session_events(&session) > 0,
        "A's turn is delivered to B, which never touched the session: {:?}",
        b.methods()
    );

    h.close(b_index, b).await;
    a.turn(&session, "after b closed").await;
    assert_eq!(
        a.session_events(&session),
        0,
        "after B closed, A's turn events go to B's dead sink: {:?}",
        a.methods()
    );
    h.clean().await;
}

/// The workaround: a member stream is per connection. A opens a
/// `mob/stream_open` stream on its member, B connects afterwards, and A's
/// member turn still streams to A and not to B.
#[cfg(feature = "mob")]
#[tokio::test]
async fn mob_stream_open_stays_with_its_connection_when_another_connects() {
    let mut h = Harness::new();
    let (_, mut a) = h.connect().await;
    a.request(
        "mob/create",
        json!({"definition":{"id":"sink_mob","profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
    )
    .await;
    a.request(
        "mob/spawn",
        json!({"mob_id":"sink_mob","profile":"worker","agent_identity":"w1","runtime_mode":"turn_driven"}),
    )
    .await;
    let stream_id = a
        .request(
            "mob/stream_open",
            json!({"mob_id":"sink_mob","agent_identity":"w1"}),
        )
        .await["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (_, mut b) = h.connect().await;
    a.request(
        "mob/turn_start",
        json!({"mob_id":"sink_mob","agent_identity":"w1","prompt":"member turn"}),
    )
    .await;
    a.settle().await;
    b.settle().await;
    let on = |connection: &Connection| {
        connection
            .notifications
            .iter()
            .filter(|frame| {
                frame["method"] == "mob/stream_event" && frame["params"]["stream_id"] == stream_id
            })
            .count()
    };
    assert!(
        a.notifications.iter().any(|frame| {
            frame["method"] == "mob/stream_event"
                && frame["params"]["stream_id"] == stream_id
                && frame["params"]["event"]["payload"]["type"] == "run_completed"
        }),
        "A's member stream carries the turn to its end: {:?}",
        a.methods()
    );
    assert_eq!(on(&b), 0, "{:?}", b.methods());
    h.clean().await;
}
