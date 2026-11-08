#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! A turn's `session/event` notifications go to the connection that started
//! it, not to whichever connection opened last.
//!
//! `SessionRuntime` held one process-wide `NotificationSink`, which every
//! `MethodRouter::new` (one per accepted connection on a shared runtime, as
//! `serve_tcp_with_options_until` and `serve_tcp_connection` build them)
//! replaced, and a session's runtime executor read the current one when it
//! applied a turn. With two connections open, the `session/event`
//! notifications of a turn connection A started went to connection B; once B
//! closed, they went nowhere, while A was still connected.
//!
//! A turn now writes them to the connections of the requests that admitted
//! its inputs. A turn no request waits on writes them to the connection that
//! holds the session: the one that created it or, once that one is gone, the
//! next one that starts a turn on it. No other connection receives them, and
//! a connection that is gone receives nothing.
//!
//! The provider is scripted; the RPC routers, TCP connections, session
//! runtime and executors are real.

use async_trait::async_trait;
use futures::StreamExt as _;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{BlobStore, Config, ConfigRuntime, ConfigStore, MemoryConfigStore, StopReason};
use meerkat_rpc::router::{MethodRouter, NotificationSink};
use meerkat_rpc::server::serve_tcp_connection;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const LIMIT: Duration = Duration::from_secs(10);
/// How long a connection is watched for trailing notifications.
const QUIET: Duration = Duration::from_millis(500);

/// Answers every request with a text delta and an end of turn. With a gate,
/// a turn waits after its delta until the gate is open.
#[derive(Default)]
struct TextProvider {
    gate: Option<watch::Receiver<bool>>,
}

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
        let usage = meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::Anthropic,
            &request.model,
            meerkat_core::Usage::default(),
        );
        let gate = self.gate.clone();
        Box::pin(
            futures::stream::iter(vec![Ok(LlmEvent::TextDelta {
                delta: "answer".into(),
                meta: None,
            })])
            .chain(futures::stream::once(async move {
                if let Some(mut gate) = gate {
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(LlmEvent::UsageUpdate { usage })
            }))
            .chain(futures::stream::iter(vec![Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            })])),
        )
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Whether `frame` is a `session/event` notification for `session_id`.
fn is_session_event(frame: &Value, session_id: &str) -> bool {
    frame["method"] == "session/event" && frame["params"]["session_id"] == session_id
}

/// Whether `frame` is a `session/event` notification of `kind` for
/// `session_id`.
fn is_session_event_of(frame: &Value, session_id: &str, kind: &str) -> bool {
    is_session_event(frame, session_id) && frame["params"]["event"]["payload"]["type"] == kind
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

    /// The number of frames kept so far: a mark for [`Self::wait_for`].
    fn mark(&self) -> usize {
        self.notifications.len()
    }

    /// Keeps the frames that arrive until one kept since `mark` satisfies
    /// `found`.
    async fn wait_for(&mut self, mark: usize, what: &str, found: impl Fn(&Value) -> bool) {
        if self.notifications[mark..].iter().any(&found) {
            return;
        }
        tokio::time::timeout(LIMIT, async {
            loop {
                let frame = self.frames.recv().await.expect("connection open");
                let hit = found(&frame);
                self.notifications.push(frame);
                if hit {
                    return;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("bounded wait for {what}"));
    }

    /// The `session/event` notifications received for `session_id`.
    fn session_events(&self, session_id: &str) -> usize {
        self.notifications
            .iter()
            .filter(|frame| is_session_event(frame, session_id))
            .count()
    }

    /// The payload types of the `session/event` notifications received for
    /// `session_id`, in order, for failure output.
    fn session_event_types(&self, session_id: &str) -> Vec<&str> {
        self.notifications
            .iter()
            .filter(|frame| is_session_event(frame, session_id))
            .filter_map(|frame| frame["params"]["event"]["payload"]["type"].as_str())
            .collect()
    }

    /// The runs whose `run_completed` reached this connection for
    /// `session_id`.
    fn runs_completed(&self, session_id: &str) -> usize {
        self.notifications
            .iter()
            .filter(|frame| is_session_event_of(frame, session_id, "run_completed"))
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

    /// The assistant messages in `session_id`'s history.
    async fn assistant_messages(&mut self, session_id: &str) -> usize {
        self.request("session/history", json!({"session_id":session_id}))
            .await["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "block_assistant")
            .count()
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
        Self::with_provider(TextProvider::default())
    }

    /// A harness whose turns, while the returned gate is set to `false`,
    /// wait after their first delta until it is set to `true` again. It
    /// starts open.
    fn gated() -> (Self, watch::Sender<bool>) {
        let (gate, open) = watch::channel(true);
        (Self::with_provider(TextProvider { gate: Some(open) }), gate)
    }

    fn with_provider(provider: TextProvider) -> Self {
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
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        runtime.set_default_llm_client(Some(Arc::new(provider)));
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

    /// Creates a session on the runtime itself, which no connection holds.
    async fn session_no_connection_holds(&self) -> String {
        self.runtime
            .create_session(
                meerkat::AgentBuildConfig::new("claude-sonnet-4-6"),
                None,
                None,
                Vec::new(),
            )
            .await
            .expect("create a session on the runtime")
            .to_string()
    }

    /// Starts one turn on `session_id` through the runtime itself, as a host
    /// does: no connection's request starts it.
    fn spawn_runtime_turn(
        &self,
        session_id: &str,
    ) -> JoinHandle<Result<meerkat_core::RunResult, meerkat_rpc::protocol::RpcError>> {
        let runtime = Arc::clone(&self.runtime);
        let session_id = meerkat_core::SessionId::parse(session_id).unwrap();
        tokio::spawn(async move {
            let (events, _events_rx) = mpsc::channel(64);
            runtime
                .start_turn_via_runtime(
                    &session_id,
                    "no connection starts this turn".into(),
                    Vec::new(),
                    events,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        })
    }

    /// Runs one turn on `session_id` through the runtime itself to
    /// completion; see [`Self::spawn_runtime_turn`].
    async fn runtime_turn(&self, session_id: &str) {
        tokio::time::timeout(LIMIT, self.spawn_runtime_turn(session_id))
            .await
            .expect("bounded runtime turn")
            .expect("the runtime turn's task")
            .expect("run a turn through the runtime");
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

/// A turn's `session/event` notifications go to the connection that started
/// it, whichever connections opened since. A's turn is watched on A while B
/// is open, and still after B closes; B never sees A's session.
#[tokio::test]
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

/// Interleaved turns of two connections, each on its own session and on a
/// session they share: every turn's `session/event` notifications reach the
/// connection that started it and no other.
#[tokio::test]
async fn interleaved_turns_reach_only_the_connection_that_started_each() {
    let mut h = Harness::new();
    let (_, mut a) = h.connect().await;
    let (_, mut b) = h.connect().await;
    let own_a = a.create_session().await;
    let own_b = b.create_session().await;
    let shared = a.create_session().await;

    // Turns on each connection's own session, run at the same time.
    tokio::join!(a.turn(&own_a, "a first"), b.turn(&own_b, "b first"));
    tokio::join!(b.turn(&own_b, "b second"), a.turn(&own_a, "a second"));
    assert_eq!(
        a.runs_completed(&own_a),
        2,
        "A's own turns: {:?}",
        a.session_event_types(&own_a)
    );
    assert_eq!(
        b.runs_completed(&own_b),
        2,
        "B's own turns: {:?}",
        b.session_event_types(&own_b)
    );
    assert_eq!(a.session_events(&own_b), 0, "B's session reached A");
    assert_eq!(b.session_events(&own_a), 0, "A's session reached B");

    // Turns the two connections take in turn on the session they share, A
    // having created it: each goes to the connection that started it.
    for (step, starter) in ["a", "b", "a", "b"].into_iter().enumerate() {
        a.notifications.clear();
        b.notifications.clear();
        let (started, other) = if starter == "a" {
            (&mut a, &mut b)
        } else {
            (&mut b, &mut a)
        };
        started.turn(&shared, &format!("shared {step}")).await;
        other.settle().await;
        assert_eq!(
            started.runs_completed(&shared),
            1,
            "step {step}: the turn {starter} started did not reach it: {:?}",
            started.session_event_types(&shared)
        );
        assert_eq!(
            other.session_events(&shared),
            0,
            "step {step}: the turn {starter} started reached the other connection: {:?}",
            other.session_event_types(&shared)
        );
    }
    h.clean().await;
}

/// The connection holding a session closes in the middle of a turn no
/// connection's request started (one the host starts on the runtime): the
/// rest of that turn's `session/event` notifications go nowhere, never to the
/// other connection, and the turn still completes: its result and the
/// session's history are intact.
#[tokio::test]
async fn a_holder_that_closes_mid_turn_leaves_the_turn_to_no_other_connection() {
    let (mut h, gate) = Harness::gated();
    let (a_index, mut a) = h.connect().await;
    let session = a.create_session().await;
    a.turn(&session, "materialize").await;
    let answered = a.assistant_messages(&session).await;
    let (_, mut b) = h.connect().await;

    gate.send_replace(false);
    let mark = a.mark();
    let held = h.spawn_runtime_turn(&session);
    // The host's turn writes to A, which holds the session, and then waits
    // after its first delta.
    a.wait_for(mark, "the held turn on A", |frame| {
        is_session_event_of(frame, &session, "run_started")
    })
    .await;
    b.settle().await;
    assert_eq!(
        b.session_events(&session),
        0,
        "the held turn reached B: {:?}",
        b.session_event_types(&session)
    );

    h.close(a_index, a).await;
    gate.send_replace(true);
    let result = tokio::time::timeout(LIMIT, held)
        .await
        .expect("bounded held turn")
        .expect("the held turn's task")
        .expect("the held turn completes");
    assert_eq!(result.text, "answer", "the held turn's result is intact");
    b.settle().await;
    assert_eq!(
        b.session_events(&session),
        0,
        "the rest of the held turn reached B: {:?}",
        b.session_event_types(&session)
    );
    assert_eq!(
        b.assistant_messages(&session).await,
        answered + 1,
        "the held turn completed into the session's history"
    );
    h.clean().await;
}

/// Once the connection holding a session is gone, a turn no connection's
/// request started reaches no connection until another connection starts a
/// turn on the session. That connection then holds it; a third connection,
/// open all along, never receives the session's turns.
#[tokio::test]
async fn the_next_connection_that_starts_a_turn_holds_the_session() {
    let mut h = Harness::new();
    let (a_index, mut a) = h.connect().await;
    let session = a.create_session().await;
    a.turn(&session, "materialize").await;
    let (_, mut b) = h.connect().await;
    let (_, mut c) = h.connect().await;
    h.close(a_index, a).await;

    // The holder is gone: the host's turn reaches no one.
    h.runtime_turn(&session).await;
    b.settle().await;
    c.settle().await;
    assert_eq!(
        b.session_events(&session),
        0,
        "{:?}",
        b.session_event_types(&session)
    );
    assert_eq!(
        c.session_events(&session),
        0,
        "{:?}",
        c.session_event_types(&session)
    );

    // B starts a turn: it receives it, and holds the session from now on.
    b.turn(&session, "b takes the session").await;
    assert_eq!(
        b.runs_completed(&session),
        1,
        "{:?}",
        b.session_event_types(&session)
    );
    let mark = b.mark();
    h.runtime_turn(&session).await;
    b.wait_for(mark, "the host's turn on B", |frame| {
        is_session_event_of(frame, &session, "run_completed")
    })
    .await;
    c.settle().await;
    assert_eq!(
        b.runs_completed(&session),
        2,
        "{:?}",
        b.session_event_types(&session)
    );
    assert_eq!(
        c.session_events(&session),
        0,
        "the session's turns reached C: {:?}",
        c.session_event_types(&session)
    );
    h.clean().await;
}

/// A single-client server keeps the process-default route: a turn no
/// connection's request started, of a session no connection holds, reaches
/// its one connection.
#[tokio::test]
async fn a_single_client_router_receives_the_turns_of_sessions_no_connection_holds() {
    let h = Harness::new();
    let (tx, mut rx) = mpsc::channel(256);
    let _router = MethodRouter::new(
        h.runtime.clone(),
        h.config.clone(),
        NotificationSink::new(tx),
    )
    .expect("construct the router");
    let session = h.session_no_connection_holds().await;
    h.runtime_turn(&session).await;
    let reached = tokio::time::timeout(LIMIT, async {
        loop {
            let notification = rx.recv().await.expect("the client's channel stays open");
            let params: Value = serde_json::from_str(notification.params.get()).unwrap();
            let frame = json!({"method": notification.method, "params": params});
            if is_session_event_of(&frame, &session, "run_completed") {
                return;
            }
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "the runtime's turn did not reach the single client"
    );
    h.clean().await;
}

/// A connection of a server that serves several on one runtime never
/// becomes the process-default route: a turn no connection's request
/// started, of a session no connection holds, reaches none of them.
#[tokio::test]
async fn tcp_connections_do_not_receive_the_turns_of_sessions_no_connection_holds() {
    let mut h = Harness::new();
    let (_, mut a) = h.connect().await;
    let session = h.session_no_connection_holds().await;
    h.runtime_turn(&session).await;
    a.settle().await;
    assert_eq!(
        a.session_events(&session),
        0,
        "a session no connection holds reached a TCP connection: {:?}",
        a.session_event_types(&session)
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
