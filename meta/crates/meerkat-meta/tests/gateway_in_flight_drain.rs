//! A graceful shutdown with `shutdown_in_flight_drain_ms` lets an in-flight
//! turn finish.
//!
//! The shipped `rpc_gateway --persistent` binary runs over a fresh state
//! directory with one roster identity and a loopback chat-completions model
//! that holds its answer until the test releases it. The identity's tracked
//! turn reaches the model and is held there. The gateway is then asked to
//! stop, by the SDK shutdown handshake or by SIGTERM, and the model is
//! released only once the drain has stopped the mob and waits for its runs.
//! The turn completes, its waiter gets the result before the gateway exits,
//! and the handshake's answer reports the drain. Without a window the init
//! advertises the plain horizon and the answer carries no drain report.
//!
//! Under SIGTERM a drained turn can still call a tool its host provides: the
//! host's answer comes back over the callback bridge, which stays open
//! through the drain. A second gateway shows that the drain closes the
//! cross-mob control listener first: its inject while the drain waits is
//! refused at connect, so nothing enters the stopped mob to be abandoned.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use meerkat_meta::GatewayPeerKeys;
use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const MOB_ID: &str = "in-flight-drain";
const MODEL: &str = "in-flight-drain-model";
const IDENTITY: &str = "agent:drained";
const TURN_TEXT: &str = "IN-FLIGHT finish the report";
const WINDOW_MS: u64 = 60_000;

/// The tool the host's agent customizer adds to the identity.
const HOST_TOOL: &str = "host_report_status";
/// The host's answer to a call of [`HOST_TOOL`].
const HOST_ANSWER: &str = "HOST-ANSWERED the report status";

/// The peer gateway's mob: one member on the deterministic demo model.
const PEER_MOB_ID: &str = "in-flight-drain-peer";
/// The peer's member, which injects into the drained gateway's member.
const PEER_MEMBER: &str = "peer-sender";
/// The drained gateway's member that the peer's injects address.
const INJECTED_MEMBER: &str = "inject-receiver";
/// The peer's first inject: it starts the turn the drain waits for.
const INJECT_TEXT: &str = "IN-FLIGHT inject from the peer";
/// The peer's inject while the drain waits.
const LATE_INJECT_TEXT: &str = "LATE inject while the drain waits";

/// The stderr line the drain logs once the mob is stopped with its runs left
/// running, right before it waits for them.
const DRAIN_WAITING: &str = "in-flight drain: the mob is stopped";
/// The stderr line the gateway logs with the drain's report.
const DRAIN_FINISHED: &str = "in-flight drain finished";
/// The stderr line the gateway logs when it keeps a shutdown handshake that
/// arrived after the dispatch loop ended.
const LATE_HANDSHAKE_KEPT: &str = "shutdown handshake arrived after the dispatch loop ended";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(3);
const EXIT_TIMEOUT: Duration = Duration::from_mins(1);

fn mob_config() -> String {
    format!(
        r#"
[mob]
id = "{MOB_ID}"

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

fn peer_mob_config() -> String {
    format!(
        r#"
[mob]
id = "{PEER_MOB_ID}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
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

fn host_config_text(base_url: &str) -> String {
    let mut config = format!(
        r#"
[self_hosted]
default_model = "{MODEL}"

[self_hosted.servers.loop]
base_url = "{base_url}"

[self_hosted.models."{MODEL}"]
server = "loop"
remote_model = "{MODEL}"
display_name = "In-flight drain test model"
family = "{MODEL}"
"#
    );
    config.push_str(&realm_binding("global"));
    config.push_str(&realm_binding(&format!("mob.{MOB_ID}")));
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

/// Probe a free loopback port. The port is released before the gateway
/// binds it, so another process could race onto it: the caveat
/// `cross_mob_two_process.rs` accepts, since both contact directories must
/// be written before either gateway boots.
fn probe_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe port")
        .local_addr()
        .expect("probe addr")
        .port()
}

// ---------------------------------------------------------------------------
// Loopback model that answers only once released
// ---------------------------------------------------------------------------

/// A streamed answer that ends the turn.
const DONE_REPLY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n\
                          data: {\"choices\":[{\"finish_reason\":\"stop\"}],\
                          \"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n\n\
                          data: [DONE]\n\n";

/// A streamed answer that is one call of `tool`, with no arguments.
fn tool_call_reply(tool: &str) -> String {
    let call = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call-host-tool-1",
                    "type": "function",
                    "function": { "name": tool, "arguments": "{}" }
                }]
            }
        }]
    });
    let done = json!({
        "choices": [{ "finish_reason": "tool_calls" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 1 }
    });
    format!("data: {call}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

/// The role and the text of a chat-completions request's last message.
fn last_message(request: &Value) -> (String, String) {
    let Some(message) = request["messages"].as_array().and_then(|all| all.last()) else {
        return (String::new(), String::new());
    };
    let text = match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    (
        message["role"].as_str().unwrap_or_default().to_string(),
        text,
    )
}

/// The names of the tools a chat-completions request offers the model.
fn offered_tools(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

struct Model {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    release: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone)]
struct ModelState {
    requests: Arc<Mutex<Vec<Value>>>,
    released: tokio::sync::watch::Receiver<bool>,
    /// A tool the model calls before it ends a turn.
    calls: Option<&'static str>,
}

async fn chat_completions(
    axum::extract::State(state): axum::extract::State<ModelState>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let answers_a_tool = last_message(&request).0 == "tool";
    state.requests.lock().unwrap().push(request);
    let mut released = state.released.clone();
    let _ = released.wait_for(|released| *released).await;
    let payload = match state.calls {
        Some(tool) if !answers_a_tool => tool_call_reply(tool),
        _ => DONE_REPLY.to_string(),
    };
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

impl Model {
    fn start() -> Self {
        Self::serve(None)
    }

    /// A model whose turns call `tool` once released, and end once its
    /// answer comes back.
    fn calling(tool: &'static str) -> Self {
        Self::serve(Some(tool))
    }

    fn serve(calls: Option<&'static str>) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (release, released) = tokio::sync::watch::channel(false);
        let state = ModelState {
            requests: Arc::clone(&requests),
            released,
            calls,
        };
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
                    .with_state(state);
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
        Self {
            base_url: format!("http://{address}"),
            requests,
            release,
        }
    }

    /// Wait until the model holds one request: the turn is in flight.
    fn wait_for_request(&self) {
        let start = Instant::now();
        while self.requests.lock().unwrap().is_empty() {
            assert!(
                start.elapsed() < RPC_TIMEOUT,
                "the turn never reached the model"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Every request the model has seen, in arrival order.
    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    fn release(&self) {
        self.release.send(true).expect("release the model");
    }
}

// ---------------------------------------------------------------------------
// Child gateway process
// ---------------------------------------------------------------------------

struct Gateway {
    label: &'static str,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    init: Value,
    /// Whether the host adds [`HOST_TOOL`] through an agent customizer.
    host_tool: bool,
    /// One entry per call of [`HOST_TOOL`] the gateway made: whether the
    /// drain was already waiting for the turn when the call came.
    host_tool_calls: Vec<bool>,
}

impl Gateway {
    /// Spawn `rpc_gateway --persistent` with `args` after it.
    fn spawn(label: &'static str, args: &[&str]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .args(args)
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
                if sink.len() < 20_000 {
                    sink.push(line);
                }
            }
        });
        Self {
            label,
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
            init: Value::Null,
            host_tool: false,
            host_tool_calls: Vec::new(),
        }
    }

    /// `meta/init` with `params`, which must succeed.
    fn init(&mut self, params: Value) {
        self.send_request("init", "meta/init", params);
        let response = self.response("init", INIT_TIMEOUT);
        assert!(
            response.get("error").is_none(),
            "[{}] init failed: {response}\n{}",
            self.label,
            self.stderr_tail()
        );
        self.init = response["result"].clone();
    }

    fn boot(state: &Path, host_config: &Path, window_ms: Option<u64>) -> Self {
        Self::boot_with(state, host_config, window_ms, false)
    }

    /// [`Self::boot`], with an agent customizer that adds [`HOST_TOOL`] to
    /// the identity when `host_tool` is set.
    fn boot_with(
        state: &Path,
        host_config: &Path,
        window_ms: Option<u64>,
        host_tool: bool,
    ) -> Self {
        let mut gateway = Self::spawn("gateway", &[]);
        gateway.host_tool = host_tool;
        let mut params = json!({
            "persistent_state": state,
            "mob_config": mob_config(),
            "has_roster_provider": true,
            "runtime_options": {
                "meerkat_config_path": host_config,
                "identity_bootstrap_mode": { "mode": "eager_materialize" }
            }
        });
        if host_tool {
            params["has_agent_customizer"] = json!(true);
        }
        if let Some(window_ms) = window_ms {
            params["shutdown_in_flight_drain_ms"] = json!(window_ms);
        }
        gateway.init(params);
        gateway
    }

    fn send_request(&mut self, id: &str, method: &str, params: Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(stdin, "{request}").expect("write request");
        stdin.flush().expect("flush request");
    }

    /// Send one request and return its response, error or not.
    fn request(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.send_request(id, method, params);
        self.response(id, RPC_TIMEOUT)
    }

    /// [`Self::request`], which must succeed.
    fn call(&mut self, id: &str, method: &str, params: Value) -> Value {
        let response = self.request(id, method, params);
        assert!(
            response.get("error").is_none(),
            "[{}] {method} failed: {response}\n{}",
            self.label,
            self.stderr_tail()
        );
        response
    }

    fn stderr_tail(&self) -> String {
        let Ok(sink) = self.stderr.lock() else {
            return "[stderr unavailable]".to_string();
        };
        let start = sink.len().saturating_sub(80);
        format!(
            "--- [{}] gateway stderr ---\n{}",
            self.label,
            sink[start..].join("\n")
        )
    }

    fn stderr_has(&self, needle: &str) -> bool {
        self.stderr
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    }

    /// The next response to any of `ids`, answering host callbacks on the
    /// way.
    fn next_response(&mut self, ids: &[&str], deadline: Duration) -> (String, Value) {
        let start = Instant::now();
        loop {
            let remaining = deadline.checked_sub(start.elapsed()).unwrap_or_else(|| {
                panic!(
                    "no response to {ids:?} within {deadline:?}\n{}",
                    self.stderr_tail()
                )
            });
            let line = self.lines.recv_timeout(remaining).unwrap_or_else(|_| {
                panic!(
                    "no response to {ids:?} within {deadline:?}\n{}",
                    self.stderr_tail()
                )
            });
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && let Some(id) = message.get("id").and_then(Value::as_str)
                && ids.contains(&id)
            {
                return (id.to_string(), message);
            }
            self.answer_callback(&message);
        }
    }

    fn response(&mut self, id: &str, deadline: Duration) -> Value {
        self.next_response(&[id], deadline).1
    }

    fn answer(&mut self, id: Value, result: Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        let answer = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        writeln!(stdin, "{answer}").expect("write callback answer");
        stdin.flush().expect("flush callback answer");
    }

    fn answer_callback(&mut self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        match method {
            "callback/roster_provider/roster" => self.answer(id, json!([roster_spec()])),
            "callback/agent_customizer/customize_build" if self.host_tool => {
                let mut draft = message["params"]["draft"].clone();
                draft["external_tools"] = json!([{
                    "name": HOST_TOOL,
                    "description": "Report the status to the host.",
                    "input_schema": { "type": "object", "properties": {} }
                }]);
                self.answer(id, draft);
            }
            "callback/agent_customizer/after_create" if self.host_tool => {
                self.answer(id, Value::Null);
            }
            "callback/call_tool" if self.host_tool => {
                assert_eq!(message["params"]["tool"], json!(HOST_TOOL), "{message}");
                let drain_waiting = self.stderr_has(DRAIN_WAITING);
                self.host_tool_calls.push(drain_waiting);
                self.answer(id, json!({ "content": HOST_ANSWER }));
            }
            other if other.starts_with("callback/") => panic!(
                "[{}] unexpected host callback {other}: this harness provides only a roster \
                 and, where a row asks for it, one host tool",
                self.label
            ),
            _ => {}
        }
    }

    /// Send the identity's tracked turn and return its ticket.
    fn send_turn(&mut self) -> String {
        self.send_request(
            "send",
            "meta/send",
            json!({ "identity": IDENTITY, "content": TURN_TEXT, "track_turn": true }),
        );
        let sent = self.response("send", RPC_TIMEOUT);
        assert!(sent.get("error").is_none(), "meta/send failed: {sent}");
        sent["result"]["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("untracked turn: {sent}"))
            .to_string()
    }

    fn wait_for_turn(&mut self, ticket: &str) {
        self.send_request(
            "wait",
            "meta/wait_for_turn",
            json!({ "identity": IDENTITY, "ticket": ticket, "timeout_ms": 120_000 }),
        );
    }

    /// [`Self::wait_for_turn`], then a cheap request, answered. The dispatch
    /// loop admits requests in order, so the answer proves the waiter was
    /// dispatched before whatever the row does next: a request the loop has
    /// not admitted when a signal ends it is refused, never served.
    fn wait_for_turn_dispatched(&mut self, ticket: &str) {
        self.wait_for_turn(ticket);
        self.call("dispatched", "meta/status", json!({}));
    }

    fn spawn_member(&mut self, profile: &str, member: &str) {
        let spawned = self.call(
            "spawn",
            "meta/spawn_member",
            json!({ "profile": profile, "meerkat_id": member }),
        );
        assert_eq!(
            spawned["result"]["accepted"],
            json!(true),
            "[{}] {member} is spawned: {spawned}",
            self.label
        );
    }

    fn wait_for_stderr(&self, needle: &str, deadline: Duration) -> String {
        let start = Instant::now();
        loop {
            if let Some(line) = self
                .stderr
                .lock()
                .unwrap()
                .iter()
                .find(|line| line.contains(needle))
            {
                return line.clone();
            }
            assert!(
                start.elapsed() < deadline,
                "no stderr line with {needle:?} within {deadline:?}\n{}",
                self.stderr_tail()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .args([format!("-{name}"), self.child.id().to_string()])
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -{name} failed: {status}");
    }

    fn wait_for_exit(&mut self, deadline: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                return status;
            }
            assert!(
                start.elapsed() < deadline,
                "gateway did not exit within {deadline:?}\n{}",
                self.stderr_tail()
            );
            while let Ok(line) = self.lines.try_recv() {
                if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                    self.answer_callback(&message);
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// The handshake shutdown, which must complete its cleanup, then the
    /// exit.
    fn shutdown_and_reap(&mut self) {
        self.send_request("shutdown", "meta/shutdown", json!({}));
        let shutdown = self.response("shutdown", SHUTDOWN_TIMEOUT);
        gateway_shutdown::assert_persistent_shutdown_completed(
            &shutdown,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        drop(self.stdin.take());
        let status = self.wait_for_exit(EXIT_TIMEOUT);
        assert!(
            status.success(),
            "[{}] exit status {status}\n{}",
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

struct Scratch {
    _dir: tempfile::TempDir,
    state: std::path::PathBuf,
    host_config: std::path::PathBuf,
}

fn scratch(model: &Model) -> Scratch {
    let dir = tempfile::tempdir().expect("scratch dir");
    let state = dir.path().join("state");
    let host_config = dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&model.base_url)).expect("write host config");
    Scratch {
        _dir: dir,
        state,
        host_config,
    }
}

fn assert_turn_completed(wait: &Value, gateway: &Gateway) {
    assert_eq!(
        wait["result"]["state"],
        json!("completed"),
        "the drained turn completes: {wait}\n{}",
        gateway.stderr_tail()
    );
}

/// The handshake with a window: the stop waits for the held turn, which
/// completes once released. Its waiter is answered before the shutdown, and
/// the shutdown's answer reports the member's run ended, completed.
#[test]
fn a_handshake_shutdown_lets_an_in_flight_turn_complete() {
    let model = Model::start();
    let scratch = scratch(&model);
    let mut gateway = Gateway::boot(&scratch.state, &scratch.host_config, Some(WINDOW_MS));
    assert_eq!(
        gateway.init["stdio_shutdown_horizon_ms"],
        json!(432_000 + 10_000 + WINDOW_MS),
        "the horizon counts the drain phase: {}",
        gateway.init
    );

    let ticket = gateway.send_turn();
    model.wait_for_request();
    gateway.wait_for_turn(&ticket);
    gateway.send_request("shutdown", "meta/shutdown", json!({}));
    gateway.wait_for_stderr(DRAIN_WAITING, SHUTDOWN_TIMEOUT);
    model.release();

    let (first, wait) = gateway.next_response(&["wait", "shutdown"], SHUTDOWN_TIMEOUT);
    assert_eq!(first, "wait", "the waiter is answered first: {wait}");
    assert_turn_completed(&wait, &gateway);
    let shutdown = gateway.response("shutdown", SHUTDOWN_TIMEOUT);
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        &format!("shutdown\n{}", gateway.stderr_tail()),
    );
    let drain = &shutdown["result"]["in_flight_drain"];
    assert_eq!(drain["outcome"], json!("drained"), "{shutdown}");
    assert_eq!(
        drain["window_ms"].as_u64().map(|ms| ms <= WINDOW_MS),
        Some(true)
    );
    assert!(
        drain["waited_ms"].as_u64().expect("waited_ms") < WINDOW_MS,
        "the drain ends with the turn, not at its bound: {drain}"
    );
    let ended = drain["ended"].as_array().expect("ended");
    assert_eq!(ended.len(), 1, "{drain}");
    assert_eq!(ended[0]["terminal"], json!("completed"), "{drain}");
    assert_eq!(drain["still_running_at_bound"], json!([]), "{drain}");

    drop(gateway.stdin.take());
    let status = gateway.wait_for_exit(EXIT_TIMEOUT);
    assert!(
        status.success(),
        "exit status {status}\n{}",
        gateway.stderr_tail()
    );
}

/// SIGTERM with a window drains the same way. Its turn completes and its
/// waiter is answered before the gateway exits, and the drain's report is
/// logged.
#[cfg(unix)]
#[test]
fn a_sigterm_shutdown_lets_an_in_flight_turn_complete() {
    let model = Model::start();
    let scratch = scratch(&model);
    let mut gateway = Gateway::boot(&scratch.state, &scratch.host_config, Some(WINDOW_MS));

    let ticket = gateway.send_turn();
    model.wait_for_request();
    gateway.wait_for_turn_dispatched(&ticket);
    gateway.signal("TERM");
    gateway.wait_for_stderr(DRAIN_WAITING, SHUTDOWN_TIMEOUT);
    model.release();

    let wait = gateway.response("wait", SHUTDOWN_TIMEOUT);
    assert_turn_completed(&wait, &gateway);
    let status = gateway.wait_for_exit(SHUTDOWN_TIMEOUT);
    assert!(
        status.success(),
        "exit status {status}\n{}",
        gateway.stderr_tail()
    );
    let finished = gateway.wait_for_stderr(DRAIN_FINISHED, EXIT_TIMEOUT);
    assert!(
        finished.contains("\"outcome\":\"drained\"") && finished.contains("\"completed\""),
        "the drain's report: {finished}"
    );
}

/// Once SIGTERM has ended the dispatch loop nothing more is dispatched, and
/// nothing sent is left unanswered. An ordinary request is refused at once
/// with the typed shutdown code. A shutdown handshake becomes this stop's: it
/// keeps stdin and the callback bridge open, and it is answered once cleanup
/// completes, with the drain's report.
#[cfg(unix)]
#[test]
fn after_a_sigterm_a_request_is_refused_and_a_handshake_answered() {
    let model = Model::start();
    let scratch = scratch(&model);
    let mut gateway = Gateway::boot(&scratch.state, &scratch.host_config, Some(WINDOW_MS));

    gateway.send_turn();
    model.wait_for_request();
    gateway.signal("TERM");
    gateway.wait_for_stderr(DRAIN_WAITING, SHUTDOWN_TIMEOUT);

    let late = gateway.request("late", "meta/status", json!({}));
    assert_eq!(
        late["error"]["code"],
        json!(-32098),
        "a request after the loop ended is refused: {late}\n{}",
        gateway.stderr_tail()
    );

    gateway.send_request("shutdown", "meta/shutdown", json!({}));
    gateway.wait_for_stderr(LATE_HANDSHAKE_KEPT, SHUTDOWN_TIMEOUT);
    model.release();
    let shutdown = gateway.response("shutdown", SHUTDOWN_TIMEOUT);
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        &format!("shutdown\n{}", gateway.stderr_tail()),
    );
    let drain = &shutdown["result"]["in_flight_drain"];
    assert_eq!(drain["outcome"], json!("drained"), "{shutdown}");
    assert_eq!(drain["ended"][0]["terminal"], json!("completed"), "{drain}");

    drop(gateway.stdin.take());
    let status = gateway.wait_for_exit(EXIT_TIMEOUT);
    assert!(
        status.success(),
        "exit status {status}\n{}",
        gateway.stderr_tail()
    );
}

/// SIGTERM with a window, and a drained turn that calls a tool its host
/// provides. The model is released only once the drain waits for the turn,
/// so the gateway asks the host during the drain. The callback bridge is
/// still open: the host answers, its answer reaches the model, and the turn
/// completes.
#[cfg(unix)]
#[test]
fn a_sigterm_drain_answers_a_drained_turns_host_callback() {
    let model = Model::calling(HOST_TOOL);
    let scratch = scratch(&model);
    let mut gateway =
        Gateway::boot_with(&scratch.state, &scratch.host_config, Some(WINDOW_MS), true);

    let ticket = gateway.send_turn();
    model.wait_for_request();
    gateway.wait_for_turn_dispatched(&ticket);
    gateway.signal("TERM");
    gateway.wait_for_stderr(DRAIN_WAITING, SHUTDOWN_TIMEOUT);
    model.release();

    let wait = gateway.response("wait", SHUTDOWN_TIMEOUT);
    assert_turn_completed(&wait, &gateway);
    assert_eq!(
        gateway.host_tool_calls,
        [true],
        "the host is asked once, while the drain waits for the turn\n{}",
        gateway.stderr_tail()
    );
    let requests = model.requests();
    assert!(
        offered_tools(&requests[0])
            .iter()
            .any(|tool| tool == HOST_TOOL),
        "the turn is offered the host's tool: {:?}",
        offered_tools(&requests[0])
    );
    let answers: Vec<String> = requests
        .iter()
        .map(last_message)
        .filter(|(role, _)| role == "tool")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(answers.len(), 1, "one tool answer: {answers:?}");
    assert!(
        answers[0].contains(HOST_ANSWER),
        "the host's answer reaches the model: {answers:?}"
    );
    let status = gateway.wait_for_exit(SHUTDOWN_TIMEOUT);
    assert!(
        status.success(),
        "exit status {status}\n{}",
        gateway.stderr_tail()
    );
    let finished = gateway.wait_for_stderr(DRAIN_FINISHED, EXIT_TIMEOUT);
    assert!(
        finished.contains("\"outcome\":\"drained\"") && finished.contains("\"completed\""),
        "the drain's report: {finished}"
    );
}

/// The drain closes the cross-mob control listener before it stops the mob.
/// A peer gateway's first inject starts the member's turn, which the model
/// holds. Under SIGTERM the drain waits for that turn, and the peer's next
/// inject is refused at connect: it never reaches the stopped mob, where it
/// would be held and then abandoned. The first turn still completes.
#[cfg(unix)]
#[test]
fn a_cross_mob_inject_while_the_drain_waits_is_refused_at_the_peer() {
    let model = Model::start();
    let scratch = scratch(&model);
    let peer_state = tempfile::tempdir().expect("peer state dir");
    let control = format!("tcp://127.0.0.1:{}", probe_port());
    let peer_control = format!("tcp://127.0.0.1:{}", probe_port());
    let keys = GatewayPeerKeys::load_or_create(&scratch.state).expect("drained gateway keys");
    let peer_keys = GatewayPeerKeys::load_or_create(peer_state.path()).expect("peer gateway keys");
    let contacts = |mob: &str, control: &str, keys: &GatewayPeerKeys| {
        format!(
            "[mobs]\n\"{mob}\" = {{ transport = \"{control}\", pubkey = \"{}\" }}\n",
            keys.pubkey_b64()
        )
    };
    let grants = |grant: &str, keys: &GatewayPeerKeys, member: &str| {
        format!(
            "[control_grants.{grant}]\npubkey = \"{}\"\n\
             verbs = [\"lookup_member\", \"wire\", \"unwire\", \"inject\"]\n\
             members = [\"{member}\"]\n",
            keys.pubkey_b64()
        )
    };

    let mut drained = Gateway::spawn("drained", &["--control-listen", &control]);
    drained.init(json!({
        "persistent_state": &scratch.state,
        "mob_config": mob_config(),
        "shutdown_in_flight_drain_ms": WINDOW_MS,
        "runtime_options": {
            "meerkat_config_path": &scratch.host_config,
            "member_comms_address": "127.0.0.1:0",
            "contacts_toml": contacts(PEER_MOB_ID, &peer_control, &peer_keys),
            "control_grants_toml": grants("peer", &peer_keys, INJECTED_MEMBER),
        }
    }));
    let mut peer = Gateway::spawn("peer", &["--control-listen", &peer_control]);
    peer.init(json!({
        "persistent_state": peer_state.path(),
        "mob_config": peer_mob_config(),
        "runtime_options": {
            "demo_llm": true,
            "member_comms_address": "127.0.0.1:0",
            "contacts_toml": contacts(MOB_ID, &control, &keys),
            "control_grants_toml": grants("drained", &keys, PEER_MEMBER),
        }
    }));
    drained.spawn_member("specialist", INJECTED_MEMBER);
    peer.spawn_member("worker", PEER_MEMBER);
    peer.call(
        "wire",
        "meta/cross_mob/wire",
        json!({
            "local_member_id": PEER_MEMBER,
            "remote_member_id": INJECTED_MEMBER,
            "remote_mob_id": MOB_ID,
        }),
    );
    let inject = |content: &str| {
        json!({
            "from_member_id": PEER_MEMBER,
            "remote_member_id": INJECTED_MEMBER,
            "remote_mob_id": MOB_ID,
            "content": content,
        })
    };
    let accepted = peer.call("inject", "meta/cross_mob/send", inject(INJECT_TEXT));
    assert!(
        accepted["result"]["session_id"].is_string(),
        "the first inject is accepted: {accepted}"
    );
    model.wait_for_request();

    drained.signal("TERM");
    drained.wait_for_stderr(DRAIN_WAITING, SHUTDOWN_TIMEOUT);
    let late = peer.request(
        "late-inject",
        "meta/cross_mob/send",
        inject(LATE_INJECT_TEXT),
    );
    assert!(late.get("result").is_none(), "{late}");
    let refusal = late["error"]["message"].as_str().unwrap_or_default();
    assert!(
        refusal.contains("control channel") && !refusal.contains("rejected request"),
        "the peer cannot connect; no stopped mob answered it: {late}"
    );

    model.release();
    let status = drained.wait_for_exit(SHUTDOWN_TIMEOUT);
    assert!(
        status.success(),
        "exit status {status}\n{}",
        drained.stderr_tail()
    );
    let finished = drained.wait_for_stderr(DRAIN_FINISHED, EXIT_TIMEOUT);
    assert!(
        finished.contains("\"outcome\":\"drained\"") && finished.contains("\"completed\""),
        "the first inject's turn completes in the drain: {finished}"
    );
    assert!(
        model
            .requests()
            .iter()
            .all(|request| !last_message(request).1.contains(LATE_INJECT_TEXT)),
        "the refused inject never reaches the model"
    );
    peer.shutdown_and_reap();
}

/// Without a window the init advertises the plain horizon, and the shutdown
/// answer carries no drain report.
#[test]
fn without_a_window_the_shutdown_is_unchanged() {
    let model = Model::start();
    let scratch = scratch(&model);
    let mut gateway = Gateway::boot(&scratch.state, &scratch.host_config, None);
    assert_eq!(
        gateway.init["stdio_shutdown_horizon_ms"],
        json!(432_000),
        "{}",
        gateway.init
    );
    gateway.send_request("shutdown", "meta/shutdown", json!({}));
    let shutdown = gateway.response("shutdown", SHUTDOWN_TIMEOUT);
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        &format!("shutdown\n{}", gateway.stderr_tail()),
    );
    assert!(
        shutdown["result"].get("in_flight_drain").is_none(),
        "{shutdown}"
    );
    drop(gateway.stdin.take());
    let status = gateway.wait_for_exit(EXIT_TIMEOUT);
    assert!(
        status.success(),
        "exit status {status}\n{}",
        gateway.stderr_tail()
    );
    assert!(
        !gateway.stderr_has("in-flight drain"),
        "no drain phase ran\n{}",
        gateway.stderr_tail()
    );
}
