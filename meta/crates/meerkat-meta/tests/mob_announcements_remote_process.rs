//! Mob announcements between two gateway processes: a coordinator gateway
//! delivers a burst to a specialist hosted by another gateway, through a
//! proxy that loses connections and answers, and across crashes of both.
//!
//! Gateway A owns the subscription; gateway B hosts the specialist and
//! serves the keyed inject control verbs to A's key only. A reaches B
//! through a proxy run by this test, which forwards whole control frames and
//! can drop an answer after B produced it, send one request to B twice, and
//! replay a captured request later. It never alters a byte: tampering is
//! covered by the unit suite.
//!
//! The story, each step asserted:
//!
//! 1. B is down while the burst arrives: A seals one batch and holds it
//!    unsettled, with no evidence either way.
//! 2. B returns, built to abort right after its continuation owner commits
//!    a delivery and before it answers. A is killed too. After both
//!    restart, A settles the batch from B's signed status alone ("admitted,
//!    acknowledgment lost"), and B holds exactly one input for it.
//! 3. A second batch: the proxy sends A's request to B twice and drops the
//!    answer. Both of B's answers carry the same receipt, and A settles the
//!    batch from a signed status read.
//! 4. A captured request replayed after B's freshness window is refused as
//!    stale and admits nothing.
//!
//! B's specialist was in the middle of its original work at the first
//! crash; it finishes that work once.
//!
//! B does not enable mob announcements itself: an identity-first gateway
//! serves keyed injects from its control listener either way.

#![cfg(feature = "announcement-test-faults")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use meerkat_meta::GatewayPeerKeys;
use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const MOB_A: &str = "announcements-remote-a";
const MOB_B: &str = "announcements-remote-b";
const MODEL: &str = "announcement-test";
const IDENTITY: &str = "agent:specialist";
const TOPIC: &str = "payments.ios";
const BURST: usize = 200;
const HOLD_MARKER: &str = "ORIGINAL-WORK";
const BATCH_MARKER: &str = "Mob announcements batch-";
const FINAL_MARKER: &str = "FINAL-CHECK";
/// B's freshness window for incoming keyed inject requests.
const REQUEST_WINDOW_MS: u64 = 5_000;

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SETTLE_TIMEOUT: Duration = Duration::from_mins(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(3);
const EXIT_TIMEOUT: Duration = Duration::from_mins(1);

fn mob_config(mob_id: &str) -> String {
    format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.specialist]
model = "{MODEL}"
provider = "self_hosted"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.specialist.tools]
comms = true
"#
    )
}

fn realm_binding(realm: &str) -> String {
    format!(
        r#"
[realm."{realm}"]
default_binding = "loop"

[realm."{realm}".backend.loop]
provider = "self_hosted"
backend_kind = "self_hosted"

[realm."{realm}".auth.loop]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{realm}".binding.loop]
backend_profile = "loop"
auth_profile = "loop"
default_model = "{MODEL}"
provider_default = true
"#
    )
}

/// The host config naming the loopback server for the specialist's model.
fn host_config(base_url: &str, mob_id: &str) -> String {
    let mut config = format!(
        r#"
[self_hosted]
default_model = "{MODEL}"

[self_hosted.servers.loop]
base_url = "{base_url}"

[self_hosted.models."{MODEL}"]
server = "loop"
remote_model = "{MODEL}"
display_name = "Announcement test model"
family = "{MODEL}"
"#
    );
    config.push_str(&realm_binding("global"));
    config.push_str(&realm_binding(&format!("mob.{mob_id}")));
    config
}

fn roster_spec() -> Value {
    json!({
        "identity": IDENTITY,
        "profile": "specialist",
        "addressability": "addressable",
        "display_name": null,
        "labels": {},
        "context": null,
        "additional_instructions": []
    })
}

/// The stderr line a fault point prints right before it aborts the process.
fn fault_line(point: &str) -> String {
    format!("announcement fault point {point}: aborting the process")
}

fn probe_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe port")
        .local_addr()
        .expect("probe addr")
        .port()
}

fn wait_until(what: &str, deadline: Duration, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Loopback model
// ---------------------------------------------------------------------------

/// What the loopback model saw: every request body, in arrival order.
#[derive(Default)]
struct ModelLog {
    requests: Vec<Value>,
    held: bool,
}

fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The text of the last message in a chat-completions request: the new
/// input a turn answers.
fn last_message_text(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|all| all.last())
        .map(message_text)
        .unwrap_or_default()
}

fn message_texts(request: &Value) -> Vec<(String, String)> {
    request["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    (
                        message["role"].as_str().unwrap_or_default().to_string(),
                        message_text(message),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn chat_completions(
    axum::extract::State(log): axum::extract::State<Arc<Mutex<ModelLog>>>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let last = last_message_text(&request);
    let hold = {
        let mut log = log.lock().unwrap();
        log.requests.push(request);
        let hold = last.contains(HOLD_MARKER) && !log.held;
        if hold {
            log.held = true;
        }
        hold
    };
    if hold {
        // The first attempt at the original work never finishes: the
        // gateway is killed while it waits.
        std::future::pending::<()>().await;
    }
    let reply = if last.contains(HOLD_MARKER) {
        "original work complete"
    } else if last.contains(BATCH_MARKER) {
        "noted the payment failures"
    } else {
        "ok"
    };
    let payload = format!(
        concat!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            "data: {{\"choices\":[{{\"finish_reason\":\"stop\"}}],",
            "\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n\n",
            "data: [DONE]\n\n"
        ),
        reply
    );
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

/// Start the loopback model on its own runtime; returns its base URL.
fn start_model(log: Arc<Mutex<ModelLog>>) -> String {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("model runtime");
        runtime.block_on(async move {
            let app = axum::Router::new()
                .route(
                    "/v1/chat/completions",
                    axum::routing::post(chat_completions),
                )
                .route("/chat/completions", axum::routing::post(chat_completions))
                .route("/v1/models", axum::routing::get(models))
                .route("/models", axum::routing::get(models))
                .with_state(log);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback model");
            tx.send(listener.local_addr().expect("model address"))
                .expect("report model address");
            axum::serve(listener, app)
                .await
                .expect("serve loopback model");
        });
    });
    let address = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("loopback model address");
    format!("http://{address}")
}

// ---------------------------------------------------------------------------
// Control-frame proxy
// ---------------------------------------------------------------------------

/// What the proxy does with the next frame of a verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// Forward the request and the answer.
    Pass,
    /// Forward the request twice, keep both answers, and answer the
    /// caller nothing.
    DuplicateAndDropReply,
}

/// One frame the proxy handled.
#[derive(Debug, Clone)]
struct FrameRecord {
    op: String,
    /// The request's delivery key: the batch id.
    delivery_key: String,
    /// `held`, `upstream_down`, `no_answer`, `forwarded` or `reply_dropped`.
    outcome: String,
    answers: Vec<Value>,
}

struct ProxyState {
    deliver_rule: Rule,
    /// While set, every request is dropped unforwarded and unanswered.
    hold: bool,
    log: Vec<FrameRecord>,
    /// Every `inject_keyed` request, as it crossed the proxy.
    captured: Vec<(String, Vec<u8>)>,
}

#[derive(Clone)]
struct Proxy {
    state: Arc<Mutex<ProxyState>>,
}

impl Proxy {
    fn set_deliver_rule(&self, rule: Rule) {
        self.state.lock().unwrap().deliver_rule = rule;
    }

    fn set_hold(&self, hold: bool) {
        self.state.lock().unwrap().hold = hold;
    }

    fn records(&self, op: &str, delivery_key: &str) -> Vec<FrameRecord> {
        self.state
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|record| record.op == op && record.delivery_key == delivery_key)
            .cloned()
            .collect()
    }

    fn any_record(&self, op: &str, outcome: &str) -> Option<FrameRecord> {
        self.state
            .lock()
            .unwrap()
            .log
            .iter()
            .find(|record| record.op == op && record.outcome == outcome)
            .cloned()
    }

    fn captured(&self, delivery_key: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .captured
            .iter()
            .find(|(key, _)| key == delivery_key)
            .map(|(_, frame)| frame.clone())
    }
}

async fn read_frame(stream: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await?;
    let mut payload = vec![0_u8; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

async fn write_frame(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    stream
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .await?;
    stream.write_all(payload).await?;
    stream.flush().await
}

/// `Err` when the upstream cannot be reached; `Ok(None)` when it closed
/// without answering.
async fn exchange(upstream: &str, payload: &[u8]) -> Result<Option<Vec<u8>>, ()> {
    let mut stream = tokio::net::TcpStream::connect(upstream)
        .await
        .map_err(|_| ())?;
    write_frame(&mut stream, payload).await.map_err(|_| ())?;
    Ok(read_frame(&mut stream).await.ok())
}

async fn proxy_one(mut client: tokio::net::TcpStream, upstream: String, proxy: Proxy) {
    let Ok(payload) = read_frame(&mut client).await else {
        return;
    };
    let request: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    let op = request["op"].as_str().unwrap_or_default().to_string();
    let delivery_key = request["delivery_key"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (rule, hold) = {
        let mut state = proxy.state.lock().unwrap();
        let hold = state.hold;
        let rule = if op == "inject_keyed" {
            state.captured.push((delivery_key.clone(), payload.clone()));
            state.deliver_rule
        } else {
            Rule::Pass
        };
        (rule, hold)
    };
    let record = |outcome: &str, answers: Vec<Value>| {
        proxy.state.lock().unwrap().log.push(FrameRecord {
            op: op.clone(),
            delivery_key: delivery_key.clone(),
            outcome: outcome.to_string(),
            answers,
        });
    };
    if hold {
        // Nothing reaches the peer, and the caller's connection closes with
        // nothing on it.
        return record("held", Vec::new());
    }
    let first = match exchange(&upstream, &payload).await {
        Err(()) => return record("upstream_down", Vec::new()),
        Ok(None) => return record("no_answer", Vec::new()),
        Ok(Some(answer)) => answer,
    };
    let decoded = |bytes: &[u8]| serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Null);
    if rule == Rule::DuplicateAndDropReply {
        let mut answers = vec![decoded(&first)];
        if let Ok(Some(second)) = exchange(&upstream, &payload).await {
            answers.push(decoded(&second));
        }
        // The caller's connection closes with nothing on it.
        return record("reply_dropped", answers);
    }
    record("forwarded", vec![decoded(&first)]);
    let _ = write_frame(&mut client, &first).await;
}

/// Start the proxy in front of `upstream`; returns it and its address.
fn start_proxy(upstream: String) -> (Proxy, String) {
    let proxy = Proxy {
        state: Arc::new(Mutex::new(ProxyState {
            deliver_rule: Rule::Pass,
            hold: false,
            log: Vec::new(),
            captured: Vec::new(),
        })),
    };
    let serving = proxy.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("proxy runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind proxy");
            tx.send(listener.local_addr().expect("proxy address"))
                .expect("report proxy address");
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(proxy_one(client, upstream.clone(), serving.clone()));
            }
        });
    });
    let address = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("proxy address");
    (proxy, format!("tcp://{address}"))
}

/// Send one frame straight to `address` and read its answer.
fn send_frame_blocking(address: &str, payload: &[u8]) -> Value {
    let mut stream = std::net::TcpStream::connect(address).expect("connect to the peer");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    stream
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .expect("write header");
    stream.write_all(payload).expect("write frame");
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).expect("read header");
    let mut answer = vec![0_u8; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut answer).expect("read answer");
    serde_json::from_slice(&answer).expect("decode answer")
}

// ---------------------------------------------------------------------------
// Child gateway process
// ---------------------------------------------------------------------------

struct Gateway {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
    roster: Vec<Value>,
}

impl Gateway {
    fn start(label: &str, control_listen: Option<&str>, fault: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .env_remove("RUST_LOG")
            .env_remove("META_ANNOUNCEMENT_FAULT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(address) = control_listen {
            command.arg("--control-listen").arg(address);
        }
        for name in [
            "RKAT_ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY",
            "RKAT_GEMINI_API_KEY",
            "GEMINI_API_KEY",
            "RKAT_SELF_HOSTED_API_KEY",
        ] {
            command.env_remove(name);
        }
        if let Some(point) = fault {
            command.env("META_ANNOUNCEMENT_FAULT", point);
        }
        let mut child = command.spawn().expect("spawn rpc_gateway --persistent");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let child_stderr = child.stderr.take().expect("gateway stderr");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr);
        thread::spawn(move || {
            for line in BufReader::new(child_stderr).lines() {
                let Ok(line) = line else { break };
                let Ok(mut sink) = sink.lock() else { break };
                if sink.len() < 2_000 {
                    sink.push(line);
                }
            }
        });
        Self {
            label: label.to_string(),
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
            next_id: 0,
            roster: Vec::new(),
        }
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        writeln!(stdin, "{}", serde_json::to_string(value).expect("encode"))
            .expect("write request");
        stdin.flush().expect("flush request");
    }

    fn stderr_tail(&self) -> String {
        let Ok(sink) = self.stderr.lock() else {
            return "[stderr unavailable]".to_string();
        };
        let start = sink.len().saturating_sub(60);
        format!(
            "--- [{}] gateway stderr (last {} lines) ---\n{}",
            self.label,
            sink.len() - start,
            sink[start..].join("\n")
        )
    }

    /// Send one request; answer host callbacks while waiting for its
    /// response. Returns the whole response, error or not.
    fn request(&mut self, method: &str, params: Value, deadline: Duration) -> Value {
        self.next_id += 1;
        let id = format!("{}-{}", self.label, self.next_id);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let start = Instant::now();
        loop {
            let Some(remaining) = deadline.checked_sub(start.elapsed()) else {
                panic!(
                    "[{}] no response to {method} within {deadline:?}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            };
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                panic!(
                    "[{}] no response to {method} within {deadline:?}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_str) == Some(id.as_str())
            {
                return message;
            }
            self.answer_callback(&message);
        }
    }

    /// `request`, asserting success; returns `result`.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let response = self.request(method, params, RPC_TIMEOUT);
        assert!(
            response.get("error").is_none(),
            "[{}] {method} failed: {response}\n{}",
            self.label,
            self.stderr_tail()
        );
        response["result"].clone()
    }

    fn answer_callback(&mut self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        match method {
            "callback/roster_provider/roster" => {
                let roster = Value::Array(self.roster.clone());
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": roster }));
            }
            other if other.starts_with("callback/") => panic!(
                "[{}] unexpected host callback {other}: this harness provides only a roster",
                self.label
            ),
            _ => {}
        }
    }

    /// Drain stdout without blocking, answering callbacks.
    fn pump(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                self.answer_callback(&message);
            }
        }
    }

    fn wait_for_exit(&mut self, deadline: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                return status;
            }
            if start.elapsed() >= deadline {
                let tail = self.stderr_tail();
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "[{}] gateway did not exit within {deadline:?}\n{tail}",
                    self.label
                );
            }
            self.pump();
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// SIGKILL: no shutdown path runs.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn shutdown_and_reap(&mut self) {
        let response = self.request("meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        gateway_shutdown::assert_persistent_shutdown_completed(
            &response,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        drop(self.stdin.take());
        let status = self.wait_for_exit(EXIT_TIMEOUT);
        assert!(
            status.success(),
            "[{}] gateway exited with {status}\n{}",
            self.label,
            self.stderr_tail()
        );
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Setup {
    state_a: tempfile::TempDir,
    state_b: tempfile::TempDir,
    config_a: std::path::PathBuf,
    config_b: std::path::PathBuf,
    control_b: String,
    contacts_a: String,
    grants_b: String,
}

fn boot_a(setup: &Setup) -> Gateway {
    let mut gateway = Gateway::start("A", None, None);
    let response = gateway.request(
        "meta/init",
        json!({
            "persistent_state": setup.state_a.path(),
            "mob_config": mob_config(MOB_A),
            "has_roster_provider": true,
            "runtime_options": {
                "meerkat_config_path": setup.config_a,
                "identity_bootstrap_mode": { "mode": "eager_materialize" },
                "contacts_toml": setup.contacts_a,
                "mob_announcements": {
                    "enabled": true,
                    "batch": {
                        "max_count": 256,
                        "max_bytes": 1_048_576,
                        "max_delay_ms": 1_000,
                        "min_wake_interval_ms": 0
                    }
                }
            }
        }),
        INIT_TIMEOUT,
    );
    assert!(
        response.get("error").is_none(),
        "[A] init failed: {response}\n{}",
        gateway.stderr_tail()
    );
    gateway
}

/// B serves keyed injects from its control listener under its grants and
/// its freshness window. It does not enable mob announcements itself.
fn b_init(setup: &Setup) -> Value {
    json!({
        "persistent_state": setup.state_b.path(),
        "mob_config": mob_config(MOB_B),
        "has_roster_provider": true,
        "runtime_options": {
            "meerkat_config_path": setup.config_b,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
            "control_grants_toml": setup.grants_b,
            "control_request_window_ms": REQUEST_WINDOW_MS
        }
    })
}

fn boot_b(setup: &Setup) -> Gateway {
    let mut gateway = Gateway::start("B", Some(&setup.control_b), None);
    gateway.roster = vec![roster_spec()];
    let response = gateway.request("meta/init", b_init(setup), INIT_TIMEOUT);
    assert!(
        response.get("error").is_none(),
        "[B] init failed: {response}\n{}",
        gateway.stderr_tail()
    );
    assert_eq!(
        response["result"]["control_listen_address"],
        json!(setup.control_b),
        "[B] init must report the bound control listener: {response}"
    );
    gateway
}

/// The specialist as A names it: a member of the mob A's contact directory
/// files B under.
fn member() -> Value {
    json!({ "mob_id": MOB_B, "identity": IDENTITY })
}

fn deliveries(gateway: &mut Gateway) -> Vec<Value> {
    gateway.call(
        "meta/mob_announcements/deliveries/list",
        json!({ "member": member() }),
    )["deliveries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The batch `index` (in seal order) once it reached `state`.
fn wait_for_batch(gateway: &mut Gateway, index: usize, state: &str) -> Value {
    let start = Instant::now();
    loop {
        let all = deliveries(gateway);
        if let Some(batch) = all.get(index)
            && batch["state"]["state"] == json!(state)
        {
            return batch.clone();
        }
        assert!(
            start.elapsed() < SETTLE_TIMEOUT,
            "[{}] batch {index} never reached {state}: {all:?}\n{}",
            gateway.label,
            gateway.stderr_tail()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// Announce into A's own mob, the default, which the subscription takes.
fn announce(gateway: &mut Gateway, key: &str, body: &str) {
    let announced = gateway.call(
        "meta/mob_announcements/announce",
        json!({ "source_key": key, "topics": [TOPIC], "body": body }),
    );
    assert_eq!(announced["captured"], json!(1));
}

fn receipt_of(answer: &Value) -> Value {
    assert_eq!(answer["result"], json!("inject_receipt"), "{answer}");
    assert!(
        answer["sig_b64"].is_string(),
        "the receipt is signed: {answer}"
    );
    answer["receipt"].clone()
}

#[test]
fn a_remote_specialist_gets_each_batch_once_through_lost_answers_and_crashes() {
    let log = Arc::new(Mutex::new(ModelLog::default()));
    let base_url = start_model(Arc::clone(&log));
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_a = config_dir.path().join("a.toml");
    let config_b = config_dir.path().join("b.toml");
    std::fs::write(&config_a, host_config(&base_url, MOB_A)).expect("write A config");
    std::fs::write(&config_b, host_config(&base_url, MOB_B)).expect("write B config");
    let state_a = tempfile::tempdir().expect("state dir a");
    let state_b = tempfile::tempdir().expect("state dir b");
    let keys_a = GatewayPeerKeys::load_or_create(state_a.path()).expect("gateway A keys");
    let keys_b = GatewayPeerKeys::load_or_create(state_b.path()).expect("gateway B keys");
    let port_b = probe_port();
    let control_b = format!("tcp://127.0.0.1:{port_b}");
    let (proxy, proxy_address) = start_proxy(format!("127.0.0.1:{port_b}"));
    let setup = Setup {
        contacts_a: format!(
            "[mobs]\n\"{MOB_B}\" = {{ transport = \"{proxy_address}\", pubkey = \"{}\" }}\n",
            keys_b.pubkey_b64()
        ),
        grants_b: format!(
            "[control_grants.coordinator]\npubkey = \"{}\"\n\
             verbs = [\"inject_keyed\", \"inject_status\"]\nmembers = [\"{IDENTITY}\"]\n",
            keys_a.pubkey_b64()
        ),
        state_a,
        state_b,
        config_a,
        config_b,
        control_b,
    };

    // B hosts the specialist, which starts its original work; the model
    // holds that first attempt open.
    let mut b = boot_b(&setup);
    b.call(
        "meta/send",
        json!({ "identity": IDENTITY, "content": format!("{HOLD_MARKER}: investigate the checkout regression") }),
    );
    wait_until(
        "the original work to reach the model",
        SETTLE_TIMEOUT,
        || log.lock().unwrap().held,
    );

    // A subscribes the remote specialist to its own mob and pauses the
    // subscription.
    let mut a = boot_a(&setup);
    let subscription = a.call(
        "meta/mob_announcements/subscriptions/put",
        json!({
            "operation_id": "subscribe-1",
            "subscription_id": "ios-payments",
            "member": member(),
            "topics": [TOPIC]
        }),
    )["subscription"]
        .clone();
    assert_eq!(subscription["member"], member());
    let paused = a.call(
        "meta/mob_announcements/subscriptions/pause",
        json!({
            "operation_id": "pause-1",
            "subscription_id": "ios-payments",
            "expected_revision": subscription["revision"]
        }),
    );

    // 1. B goes down in the middle of its original work; the burst arrives.
    b.kill();
    for index in 0..BURST {
        announce(
            &mut a,
            &format!("failure-{index}"),
            &format!("card declined during checkout for order {index}"),
        );
    }
    a.call(
        "meta/mob_announcements/subscriptions/resume",
        json!({
            "operation_id": "resume-1",
            "subscription_id": "ios-payments",
            "expected_revision": paused["subscription"]["revision"]
        }),
    );
    let first = wait_for_batch(&mut a, 0, "submitting");
    let first_id = first["id"].as_str().unwrap().to_string();
    assert_eq!(
        first["items"].as_array().map(Vec::len),
        Some(BURST),
        "the whole burst is one batch"
    );
    wait_until(
        "an attempt to reach the absent peer",
        SETTLE_TIMEOUT,
        || {
            proxy
                .records("inject_keyed", &first_id)
                .iter()
                .any(|record| record.outcome == "upstream_down")
        },
    );
    let observed = a.call(
        "meta/mob_announcements/deliveries/observe",
        json!({ "batch_id": first_id }),
    );
    assert_eq!(
        observed["native"]["status"],
        json!("unavailable"),
        "{observed}"
    );
    assert_eq!(
        deliveries(&mut a)[0]["state"]["state"],
        json!("submitting"),
        "no evidence either way while the peer is down"
    );

    // 2. B returns, aborts right after committing the batch, and A is
    // killed before it can learn anything else. B may abort before it
    // answers its own init, so that answer is not awaited.
    let mut b = Gateway::start("B", Some(&setup.control_b), Some("after_remote_commit"));
    b.roster = vec![roster_spec()];
    b.send(&json!({
        "jsonrpc": "2.0",
        "id": "init-fault",
        "method": "meta/init",
        "params": b_init(&setup)
    }));
    let status = b.wait_for_exit(SETTLE_TIMEOUT);
    assert!(!status.success(), "B must die at the fault point: {status}");
    let b_tail = b.stderr_tail();
    assert!(
        b_tail.contains(&fault_line("after_remote_commit")),
        "B died somewhere else\n{b_tail}"
    );
    drop(b);
    assert!(
        proxy
            .records("inject_keyed", &first_id)
            .iter()
            .any(|record| record.outcome == "no_answer"),
        "the committing attempt was never answered"
    );
    a.kill();
    drop(a);

    let mut b = boot_b(&setup);
    let mut a = boot_a(&setup);
    let first = wait_for_batch(&mut a, 0, "delivered");
    let statuses = proxy.records("inject_status", &first_id);
    let settled_by = statuses
        .iter()
        .rev()
        .find(|record| record.outcome == "forwarded")
        .expect("A read the status");
    let answer = &settled_by.answers[0];
    assert!(answer["sig_b64"].is_string(), "{answer}");
    assert!(
        matches!(
            answer["status"]["status"].as_str(),
            Some("pending" | "applied")
        ),
        "the lost acknowledgment was settled from B's signed status: {answer}"
    );
    assert_eq!(
        first["state"]["receipt"], answer["status"]["receipt"],
        "the recorded receipt is the one B issued"
    );
    wait_until("the first batch input to complete", SETTLE_TIMEOUT, || {
        let observed = a.call(
            "meta/mob_announcements/deliveries/observe",
            json!({ "batch_id": first_id }),
        );
        observed["input"]["completion"] == json!("completed")
    });

    // 3. A second batch: B gets the request twice and A gets no answer.
    proxy.set_deliver_rule(Rule::DuplicateAndDropReply);
    announce(&mut a, "failure-late", "card declined after the restart");
    let second = wait_for_batch(&mut a, 1, "delivered");
    let second_id = second["id"].as_str().unwrap().to_string();
    proxy.set_deliver_rule(Rule::Pass);
    let dropped = proxy
        .records("inject_keyed", &second_id)
        .into_iter()
        .find(|record| record.outcome == "reply_dropped")
        .expect("the second batch's answer was dropped");
    assert_eq!(dropped.answers.len(), 2, "B was asked twice: {dropped:?}");
    assert_eq!(
        receipt_of(&dropped.answers[0]),
        receipt_of(&dropped.answers[1]),
        "a replay is answered with the original receipt"
    );
    assert_eq!(
        second["state"]["receipt"],
        receipt_of(&dropped.answers[0]),
        "A settled from status the receipt it never received"
    );

    // 4. A captured request replayed after B's window is refused as stale.
    let frame = proxy.captured(&second_id).expect("captured request");
    thread::sleep(Duration::from_millis(REQUEST_WINDOW_MS + 1_500));
    let stale = send_frame_blocking(&format!("127.0.0.1:{port_b}"), &frame);
    assert_eq!(stale["result"], json!("inject_refused"), "{stale}");
    assert_eq!(stale["code"], json!("stale_request"), "{stale}");
    assert_eq!(
        stale["cause"],
        json!("outside_window"),
        "a stale refusal says why, in its own signed field: {stale}"
    );
    assert!(stale["sig_b64"].is_string(), "refusals are signed: {stale}");
    assert!(
        proxy.any_record("inject_keyed", "upstream_down").is_some(),
        "the proxy saw the absent peer"
    );

    // The specialist's own record: one more turn shows its whole history.
    wait_until("the second batch input to complete", SETTLE_TIMEOUT, || {
        let observed = a.call(
            "meta/mob_announcements/deliveries/observe",
            json!({ "batch_id": second_id }),
        );
        observed["input"]["completion"] == json!("completed")
    });
    b.call(
        "meta/send",
        json!({ "identity": IDENTITY, "content": format!("{FINAL_MARKER}: summarize") }),
    );
    wait_until("the final turn to reach the model", SETTLE_TIMEOUT, || {
        log.lock()
            .unwrap()
            .requests
            .iter()
            .any(|request| last_message_text(request).contains(FINAL_MARKER))
    });
    let log = log.lock().unwrap();
    let final_request = log
        .requests
        .iter()
        .rev()
        .find(|request| last_message_text(request).contains(FINAL_MARKER))
        .expect("final request");
    let transcript = message_texts(final_request);
    let count = |needle: &str| {
        transcript
            .iter()
            .filter(|(_, text)| text.contains(needle))
            .count()
    };
    let originals = transcript
        .iter()
        .filter(|(role, text)| role == "user" && text.contains(HOLD_MARKER))
        .count();
    assert_eq!(originals, 1, "one original input: {transcript:?}");
    assert_eq!(
        count(&first_id),
        1,
        "the first batch is one input: {transcript:?}"
    );
    assert_eq!(
        count(&second_id),
        1,
        "the second batch is one input: {transcript:?}"
    );
    assert!(
        transcript
            .iter()
            .any(|(role, text)| role == "assistant" && text.contains("original work complete")),
        "the original work completed after the crashes: {transcript:?}"
    );
    let batch_turns = log
        .requests
        .iter()
        .filter(|request| last_message_text(request).contains(BATCH_MARKER))
        .count();
    eprintln!(
        "mob announcement remote evidence: first={first_id} items={} receipt={} \
         second={second_id} duplicate_receipts_equal=true stale={} batch_turns={batch_turns} \
         model_requests={}",
        first["items"].as_array().map_or(0, Vec::len),
        first["state"]["receipt"]["delivery_id"],
        stale["code"],
        log.requests.len(),
    );
    drop(log);
    a.shutdown_and_reap();
    b.shutdown_and_reap();
}

/// A second gateway granted on the same member cannot take over another
/// caller's batch: it delivers under the other caller's batch id, and the
/// first caller still settles only its own delivery.
///
/// A's first attempt never reaches B, so A holds the batch unsettled. The
/// second caller, granted the keyed inject verbs on the same member, then
/// delivers its own body under A's batch id, straight to B. When A reaches
/// B again it must find nothing committed for its own delivery, submit its
/// own content, and settle on its own receipt; B's specialist receives each
/// body exactly once.
#[test]
fn a_second_granted_caller_cannot_settle_another_callers_batch() {
    const COORDINATOR_TEXT: &str = "COORDINATOR-ANNOUNCEMENT card declined on the new checkout";
    const INTRUDER_TEXT: &str = "INTRUDER-BODY unrelated content under a borrowed batch id";
    let log = Arc::new(Mutex::new(ModelLog::default()));
    let base_url = start_model(Arc::clone(&log));
    let config_dir = tempfile::tempdir().expect("config dir");
    let config_a = config_dir.path().join("a.toml");
    let config_b = config_dir.path().join("b.toml");
    std::fs::write(&config_a, host_config(&base_url, MOB_A)).expect("write A config");
    std::fs::write(&config_b, host_config(&base_url, MOB_B)).expect("write B config");
    let state_a = tempfile::tempdir().expect("state dir a");
    let state_b = tempfile::tempdir().expect("state dir b");
    let intruder_dir = tempfile::tempdir().expect("intruder key dir");
    let keys_a = GatewayPeerKeys::load_or_create(state_a.path()).expect("gateway A keys");
    let keys_b = GatewayPeerKeys::load_or_create(state_b.path()).expect("gateway B keys");
    let keys_c = GatewayPeerKeys::load_or_create(intruder_dir.path()).expect("second caller keys");
    let port_b = probe_port();
    let control_b = format!("tcp://127.0.0.1:{port_b}");
    let (proxy, proxy_address) = start_proxy(format!("127.0.0.1:{port_b}"));
    let keyed_inject_grant = |label: &str, keys: &GatewayPeerKeys| {
        format!(
            "[control_grants.{label}]\npubkey = \"{}\"\n\
             verbs = [\"inject_keyed\", \"inject_status\"]\nmembers = [\"{IDENTITY}\"]\n",
            keys.pubkey_b64()
        )
    };
    let setup = Setup {
        contacts_a: format!(
            "[mobs]\n\"{MOB_B}\" = {{ transport = \"{proxy_address}\", pubkey = \"{}\" }}\n",
            keys_b.pubkey_b64()
        ),
        grants_b: format!(
            "{}{}",
            keyed_inject_grant("coordinator", &keys_a),
            keyed_inject_grant("second-caller", &keys_c)
        ),
        state_a,
        state_b,
        config_a,
        config_b,
        control_b,
    };

    let mut b = boot_b(&setup);
    let mut a = boot_a(&setup);
    a.call(
        "meta/mob_announcements/subscriptions/put",
        json!({
            "operation_id": "subscribe-1",
            "subscription_id": "ios-payments",
            "member": member(),
            "topics": [TOPIC]
        }),
    );

    // A's first attempt never reaches B: the batch stays unsettled.
    proxy.set_hold(true);
    announce(&mut a, "failure-1", COORDINATOR_TEXT);
    let batch = wait_for_batch(&mut a, 0, "submitting");
    let batch_id = batch["id"].as_str().unwrap().to_string();
    wait_until("A's first attempt to be held", SETTLE_TIMEOUT, || {
        proxy
            .records("inject_keyed", &batch_id)
            .iter()
            .any(|record| record.outcome == "held")
    });

    // The second caller delivers its own body under A's batch id.
    let issued_at_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut request = meerkat_meta::runtime::cross_mob_control::ControlRequest::InjectKeyed {
        remote_member: IDENTITY.to_string(),
        delivery_key: batch_id.clone(),
        content_digest: meerkat_meta::mob_announcements::types::content_digest(INTRUDER_TEXT),
        summary: "1 announcement".to_string(),
        body: INTRUDER_TEXT.to_string(),
        issued_at_ms,
        nonce: "second-caller-nonce-1".to_string(),
        caller: None,
    };
    meerkat_meta::runtime::cross_mob_control::sign_control_request_as_caller(
        &keys_c,
        Some(MOB_B),
        &mut request,
    );
    let intruder_answer = send_frame_blocking(
        &format!("127.0.0.1:{port_b}"),
        &serde_json::to_vec(&request).unwrap(),
    );
    let intruder_receipt = receipt_of(&intruder_answer);

    // A reaches B again.
    proxy.set_hold(false);
    let start = Instant::now();
    let settled = loop {
        let all = deliveries(&mut a);
        let state = all[0]["state"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if matches!(state.as_str(), "delivered" | "blocked" | "refused") {
            break all[0].clone();
        }
        assert!(
            start.elapsed() < SETTLE_TIMEOUT,
            "A's batch never settled: {all:?}\n{}",
            a.stderr_tail()
        );
        thread::sleep(Duration::from_millis(100));
    };
    let own_receipt = settled["state"]["receipt"].clone();
    let settled_on_own_receipt = settled["state"]["state"] == json!("delivered")
        && own_receipt["delivery_id"] != intruder_receipt["delivery_id"];

    // One more turn shows the specialist's whole history.
    if settled_on_own_receipt {
        wait_until("A's batch input to complete", SETTLE_TIMEOUT, || {
            let observed = a.call(
                "meta/mob_announcements/deliveries/observe",
                json!({ "batch_id": batch_id }),
            );
            observed["input"]["completion"] == json!("completed")
        });
    }
    b.call(
        "meta/send",
        json!({ "identity": IDENTITY, "content": format!("{FINAL_MARKER}: summarize") }),
    );
    wait_until("the final turn to reach the model", SETTLE_TIMEOUT, || {
        log.lock()
            .unwrap()
            .requests
            .iter()
            .any(|request| last_message_text(request).contains(FINAL_MARKER))
    });
    let transcript = {
        let log = log.lock().unwrap();
        message_texts(
            log.requests
                .iter()
                .rev()
                .find(|request| last_message_text(request).contains(FINAL_MARKER))
                .expect("final request"),
        )
    };
    let count = |needle: &str| {
        transcript
            .iter()
            .filter(|(_, text)| text.contains(needle))
            .count()
    };
    let evidence = format!(
        "batch={batch_id} state={} own_receipt={} intruder_receipt={} \
         coordinator_inputs={} intruder_inputs={}",
        settled["state"]["state"],
        own_receipt["delivery_id"],
        intruder_receipt["delivery_id"],
        count("COORDINATOR-ANNOUNCEMENT"),
        count("INTRUDER-BODY"),
    );
    eprintln!("mob announcement substitution evidence: {evidence}");
    assert!(
        settled_on_own_receipt,
        "A settles only on its own delivery's receipt: {evidence}"
    );
    assert_eq!(
        count("COORDINATOR-ANNOUNCEMENT"),
        1,
        "A's content is admitted exactly once: {evidence}"
    );
    assert_eq!(
        count("INTRUDER-BODY"),
        1,
        "the second caller's content is its own delivery: {evidence}"
    );
    a.shutdown_and_reap();
    b.shutdown_and_reap();
}
