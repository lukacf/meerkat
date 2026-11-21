//! Idle WorkGraph pursuit in a real persistent gateway process: two rows.
//!
//! Each boot is the `rpc_gateway --persistent` binary target, spawned as a
//! child process over stdin and built with the
//! `workgraph-pursuit-test-vehicle` feature. It runs the same `main` and the
//! same launch function as the shipped binary with the feature's gated
//! statements compiled in; it is not the shipped binary's bytes. A boot
//! composes pursuit only when this test sets
//! `META_TEST_VEHICLE_WORKGRAPH_PURSUIT=all` on the child it spawns.
//!
//! The model is a loopback scripted endpoint run by this test; no provider.
//! Everything else is the product: the gateway process, its stores, the
//! WorkGraph service, the pursuit and the runtime. The host writes only
//! through the gateway's own `meta/workgraph/*` methods, and no input is
//! injected to stand for a wake: a turn the test did not prompt can only
//! come from a committed write.
//!
//! - Row 1, the wake: a seated member that was never prompted takes one turn
//!   from the attention notice after the host creates a goal bound to it. Its
//!   control is the same binary on a second state directory without the
//!   environment value: the same goal starts no turn, and a later prompt's
//!   request is the first the model sees.
//! - Row 2, across a kill: a continuation that waits behind a running turn
//!   when the gateway process is killed takes one turn after a second boot
//!   on the same state directory, under the item as last revised, with no
//!   write and no prompt after the kill. That turn waits at the model while
//!   the claim, the binding and the session are read as the restart left
//!   them, and is then answered with two scripted calls of the member's own
//!   tools, one after the other: `workgraph_add_evidence`, read back with
//!   the claim it landed under before anything else is written, and then
//!   `workgraph_release`, the one write of its turn that the item admits only
//!   while a claim is present. One more revision then starts one more turn.
//!   The kill also leaves a run in flight, which is recovery work
//!   by itself, so this row does not show that a waiting input alone brings
//!   its member up.
//!
//! Each row prints every reading before it asserts on any, so a failed run
//! still shows what the processes, the stores and the model held.

#![cfg(feature = "workgraph-pursuit-test-vehicle")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const MOB_ID: &str = "workgraph-pursuit-process";
const MODEL: &str = "workgraph-pursuit-test-model";
/// The identity's name when the test host names none: the conventional form
/// of an identity-first name, which Core encodes into another roster id.
const CONVENTIONAL_IDENTITY: &str = "agent:pursuer";
/// The test host's choice of the identity's name for one run of the rows.
/// It names the scene: the gateway binary, the stores, the runtime
/// configuration and the model are the same for every name.
const IDENTITY_ENV: &str = "WORKGRAPH_PURSUIT_ROWS_IDENTITY";

static IDENTITY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    std::env::var(IDENTITY_ENV).unwrap_or_else(|_| CONVENTIONAL_IDENTITY.to_string())
});

/// The name of the one identity of the rows' roster.
fn identity() -> &'static str {
    IDENTITY.as_str()
}

/// Print which scene this run is: the identity's name and the roster id
/// Core encodes it into.
fn say_scene(row: &str) {
    let roster_id = meerkat_mob::ids::member_id_for_name(identity());
    let same = roster_id == identity();
    say(
        row,
        format!(
            "scene: {}; identity name {:?}, roster member id by Core's encoding {:?}, spellings \
             identical: {same}",
            if same {
                "a name that encodes to itself"
            } else {
                "a conventional name, which Core encodes into another roster id"
            },
            identity(),
            roster_id,
        ),
    );
}

/// The environment value that makes a boot compose pursuit.
const ENABLE_ENV: &str = "META_TEST_VEHICLE_WORKGRAPH_PURSUIT";
/// What the gateway writes to stderr when it composes or attaches pursuit.
const VEHICLE_LINE: &str = "workgraph pursuit test vehicle:";
/// The sentence an attention continuation's notice starts with.
const NOTICE_PHRASE: &str = "Continue from the WorkGraph attention projection.";
/// The log filter every boot of both arms runs under: the gateway's own
/// default, with the pursuit's module at debug so that a write it weighs and
/// starts no turn for says why on stderr, and the resume gate's module at
/// debug so that a run it lets resume says so. It changes what is logged
/// only.
const GATEWAY_LOG_FILTER: &str = "warn,meerkat_meta=info,rpc_gateway=info,\
     meerkat_mob::runtime::workgraph_pursuit=debug,meerkat_meta::workgraph_resume_gate=debug";
/// What every line of the resume gate's verdicts holds.
const RESUME_GATE_LINE: &str = "durable resume gate:";

const ORIGINAL: &str = "ORIGINAL-DESCRIPTION-7f3a91";
const REVISED: &str = "REVISED-DESCRIPTION-c41d08";
const LATER: &str = "LATER-DESCRIPTION-e95b22";
const HELD_PROMPT: &str = "HELD-PROMPT-2b6f40 answer when released";
const PROBE_PROMPT: &str = "PROBE-PROMPT-9d1735 say ok";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(2);
/// How long a row waits for a turn a committed write should start.
const WAKE_WAIT: Duration = Duration::from_mins(1);
/// How long a row waits for the turn of a continuation after a second boot.
const RECOVERY_WAIT: Duration = Duration::from_mins(2);
/// How long a row keeps reading after a turn, to count a second one.
const SETTLE: Duration = Duration::from_secs(8);
/// How long the control waits for a turn that must not start.
const CONTROL_QUIET: Duration = Duration::from_secs(15);

fn say(row: &str, text: impl AsRef<str>) {
    println!("[{row}] {}", text.as_ref());
}

fn verdict(pass: bool) -> &'static str {
    if pass { "pass" } else { "fail" }
}

fn head(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

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
workgraph = true
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
display_name = "WorkGraph pursuit test model"
family = "{MODEL}"
"#
    );
    config.push_str(&realm_binding("global"));
    config.push_str(&realm_binding(&format!("mob.{MOB_ID}")));
    config
}

fn roster_spec() -> Value {
    json!({
        "identity": identity(),
        "profile": "specialist",
        "addressability": "addressable",
        "display_name": null,
        "labels": {},
        "context": null,
        "additional_instructions": []
    })
}

// ---------------------------------------------------------------------------
// Loopback model: records every request, answers "ok", holds the requests
// that carry a marker until the test releases them, and answers one request
// a row scripts with one tool call
// ---------------------------------------------------------------------------

const REPLY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
     data: {\"choices\":[{\"finish_reason\":\"stop\"}],\
     \"usage\":{\"prompt_tokens\":50,\"completion_tokens\":1}}\n\n\
     data: [DONE]\n\n";

/// The answer that makes one call of `tool` with `arguments` and ends there.
/// `number` tells the calls of one row apart.
fn tool_call_reply(number: usize, tool: &str, arguments: &Value) -> String {
    let call = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": format!("call-scripted-{number}"),
                    "type": "function",
                    "function": { "name": tool, "arguments": arguments.to_string() }
                }]
            }
        }]
    });
    let done = json!({
        "choices": [{ "finish_reason": "tool_calls" }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 1 }
    });
    format!("data: {call}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

/// The tool calls a row scripts for one turn. The turn's first request
/// starts from a notice that carries `marker`; each later request of it
/// carries a tool's result. Each of them takes the next call while one is
/// left, and waits at the model until the row names it.
struct Script {
    marker: String,
    calls: usize,
    taken: usize,
}

struct Model {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    hold: Arc<Mutex<Option<String>>>,
    release: tokio::sync::watch::Sender<bool>,
    script: Arc<Mutex<Option<Script>>>,
    scripted_replies: tokio::sync::watch::Sender<Vec<String>>,
}

#[derive(Clone)]
struct ModelState {
    requests: Arc<Mutex<Vec<Value>>>,
    hold: Arc<Mutex<Option<String>>>,
    released: tokio::sync::watch::Receiver<bool>,
    script: Arc<Mutex<Option<Script>>>,
    scripted_replies: tokio::sync::watch::Receiver<Vec<String>>,
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

fn messages(request: &Value) -> &[Value] {
    request["messages"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Every message's text of one request, in order.
fn transcript(request: &Value) -> String {
    messages(request)
        .iter()
        .map(message_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The newest message of one request: what its turn starts from.
fn last_message(request: &Value) -> String {
    messages(request)
        .last()
        .map(message_text)
        .unwrap_or_default()
}

/// Whether the request's newest message is an attention notice: the turn
/// was started by a continuation.
fn starts_from_notice(request: &Value) -> bool {
    last_message(request).contains(NOTICE_PHRASE)
}

fn tool_names(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the request's newest message is a tool's result: the request a
/// turn makes after a tool call.
fn ends_with_tool_result(request: &Value) -> bool {
    messages(request)
        .last()
        .is_some_and(|last| last["role"] == "tool")
}

/// The first request from `from` on whose newest message is a tool's result,
/// with that result's text.
fn tool_result(requests: &[Value], from: usize) -> Option<(usize, String)> {
    requests
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, request)| ends_with_tool_result(request))
        .map(|(index, request)| (index, last_message(request)))
}

/// One line for a request: what it carries and what it starts from.
fn digest(index: usize, request: &Value) -> String {
    let text = transcript(request);
    let list = messages(request);
    let tail = list
        .iter()
        .rev()
        .take(4)
        .rev()
        .map(|message| {
            format!(
                "{}:{:?}",
                message["role"].as_str().unwrap_or("?"),
                head(&message_text(message), 48)
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let tools = tool_names(request);
    let workgraph_tools = tools
        .iter()
        .filter(|tool| tool.starts_with("workgraph_"))
        .map(|tool| tool.trim_start_matches("workgraph_"))
        .collect::<Vec<_>>();
    format!(
        "request #{index}: messages={} notices={} original={} revised={} later={} held_prompt={} \
         probe_prompt={} starts_from_notice={} tools={} workgraph_tools={} ({}) tail=[{tail}]",
        list.len(),
        text.matches(NOTICE_PHRASE).count(),
        text.matches(ORIGINAL).count(),
        text.matches(REVISED).count(),
        text.matches(LATER).count(),
        text.matches(HELD_PROMPT).count(),
        text.matches(PROBE_PROMPT).count(),
        starts_from_notice(request),
        tools.len(),
        workgraph_tools.len(),
        workgraph_tools.join(" "),
    )
}

async fn chat_completions(
    axum::extract::State(state): axum::extract::State<ModelState>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let held = state
        .hold
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|marker| transcript(&request).contains(marker.as_str()));
    // A scripted turn: this request takes the next of the row's calls when
    // it is that turn's next request and a call is left.
    let scripted = {
        let mut script = state.script.lock().unwrap();
        script.as_mut().and_then(|script| {
            let next = if script.taken == 0 {
                starts_from_notice(&request)
                    && last_message(&request).contains(script.marker.as_str())
            } else {
                ends_with_tool_result(&request)
            };
            (next && script.taken < script.calls).then(|| {
                script.taken += 1;
                script.taken - 1
            })
        })
    };
    state.requests.lock().unwrap().push(request);
    if held {
        let mut released = state.released.clone();
        let _ = released.wait_for(|released| *released).await;
    }
    let body = match scripted {
        Some(call) => {
            let mut replies = state.scripted_replies.clone();
            replies
                .wait_for(|replies| replies.len() > call)
                .await
                .ok()
                .and_then(|replies| replies.get(call).cloned())
                .unwrap_or_else(|| REPLY.to_string())
        }
        None => REPLY.to_string(),
    };
    ([("content-type", "text/event-stream")], body)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

impl Model {
    fn start() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let hold = Arc::new(Mutex::new(None));
        let (release, released) = tokio::sync::watch::channel(false);
        let script = Arc::new(Mutex::new(None));
        let (scripted_replies, scripted_answers) = tokio::sync::watch::channel(Vec::new());
        let state = ModelState {
            requests: Arc::clone(&requests),
            hold: Arc::clone(&hold),
            released,
            script: Arc::clone(&script),
            scripted_replies: scripted_answers,
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
            hold,
            release,
            script,
            scripted_replies,
        }
    }

    /// Every request the model has seen, in arrival order.
    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    fn seen(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// From now on a request that carries `marker` is answered only once
    /// the test releases the model.
    fn hold_requests_carrying(&self, marker: &str) {
        *self.hold.lock().unwrap() = Some(marker.to_string());
    }

    fn release(&self) {
        self.release.send(true).expect("release the model");
    }

    /// From now on the turn whose first request's newest message is a
    /// notice that carries `marker` makes `calls` tool calls, one per
    /// request, and each of those requests waits at the model until the test
    /// names its call. Every other request is answered as before.
    fn script_calls_on_notice(&self, marker: &str, calls: usize) {
        *self.script.lock().unwrap() = Some(Script {
            marker: marker.to_string(),
            calls,
            taken: 0,
        });
    }

    /// Name the next scripted call: the request that waits for it is
    /// answered with one call of `tool`.
    fn answer_scripted_call(&self, tool: &str, arguments: &Value) {
        self.scripted_replies.send_modify(|replies| {
            let reply = tool_call_reply(replies.len() + 1, tool, arguments);
            replies.push(reply);
        });
    }
}

// ---------------------------------------------------------------------------
// Child gateway process
// ---------------------------------------------------------------------------

struct Gateway {
    row: String,
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    stderr_printed: usize,
    next_id: u64,
}

impl Gateway {
    /// Spawn the gateway binary; with `pursue` the child carries the
    /// environment value that makes it compose pursuit, without it the child
    /// carries no such value whatever this test's own environment holds.
    fn start(row: &str, label: &str, pursue: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .env("RUST_LOG", GATEWAY_LOG_FILTER)
            .env_remove(IDENTITY_ENV)
            .env_remove(ENABLE_ENV)
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
        if pursue {
            command.env(ENABLE_ENV, "all");
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
                if sink.len() < 8_000 {
                    sink.push(line);
                }
            }
        });
        say(
            row,
            format!(
                "[{label}] gateway process id {} ({}={})",
                child.id(),
                ENABLE_ENV,
                if pursue { "all" } else { "not set" }
            ),
        );
        Self {
            row: row.to_string(),
            label: label.to_string(),
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
            stderr_printed: 0,
            next_id: 0,
        }
    }

    /// Print every stderr line the gateway wrote since the last call.
    fn print_stderr(&mut self, when: &str) {
        let lines = self
            .stderr
            .lock()
            .map(|sink| sink[self.stderr_printed.min(sink.len())..].to_vec())
            .unwrap_or_default();
        self.stderr_printed += lines.len();
        self.say(format!(
            "{when}: gateway stderr lines since the last print: {}",
            lines.len()
        ));
        for line in &lines {
            self.say(format!("{when}: stderr: {}", head(line, 600)));
        }
    }

    /// Print every line of this boot's stderr in which the resume gate
    /// states a verdict.
    fn say_resume_gate_lines(&self, when: &str) {
        let lines = self
            .stderr
            .lock()
            .map(|sink| {
                sink.iter()
                    .filter(|line| line.contains(RESUME_GATE_LINE))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.say(format!(
            "{when}: resume gate verdict lines on this boot's stderr: {}",
            lines.len()
        ));
        for line in &lines {
            self.say(format!("{when}: resume gate: {}", head(line, 600)));
        }
    }

    fn say(&self, text: impl AsRef<str>) {
        say(&self.row, format!("[{}] {}", self.label, text.as_ref()));
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

    /// The stderr lines the gateway wrote about the pursuit it composed.
    fn vehicle_lines(&self) -> Vec<String> {
        self.stderr
            .lock()
            .map(|sink| {
                sink.iter()
                    .filter(|line| line.contains(VEHICLE_LINE))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One request; `None` when no response came within `deadline`.
    fn try_request(&mut self, method: &str, params: Value, deadline: Duration) -> Option<Value> {
        self.next_id += 1;
        let id = format!("{}-{}", self.label, self.next_id);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let start = Instant::now();
        loop {
            let remaining = deadline.checked_sub(start.elapsed())?;
            let line = self.lines.recv_timeout(remaining).ok()?;
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_str) == Some(id.as_str())
            {
                return Some(message);
            }
            self.answer_callback(&message);
        }
    }

    fn request(&mut self, method: &str, params: Value, deadline: Duration) -> Value {
        self.try_request(method, params, deadline)
            .unwrap_or_else(|| {
                panic!(
                    "[{}] no response to {method} within {deadline:?}\n{}",
                    self.label,
                    self.stderr_tail()
                )
            })
    }

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
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": [roster_spec()] }));
            }
            other if other.starts_with("callback/") => panic!(
                "[{}] unexpected host callback {other}: this harness provides only a roster",
                self.label
            ),
            _ => {}
        }
    }

    fn pump(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                self.answer_callback(&message);
            }
        }
    }

    /// Poll `condition` while answering the gateway's callbacks; `false`
    /// when `deadline` passed first.
    fn eventually(&mut self, deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            self.pump();
            if condition() {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Keep answering the gateway's callbacks for `window`.
    fn settle(&mut self, window: Duration) {
        let _ = self.eventually(window, || false);
    }

    /// Kill the process without a shutdown request and reap it.
    fn kill(&mut self) -> ExitStatus {
        self.child.kill().expect("kill the gateway process");
        self.child.wait().expect("reap the killed gateway process")
    }

    /// Ask for an orderly shutdown and print what came back. Not a reading
    /// of either row: nothing is asserted on it.
    fn shutdown_best_effort(&mut self) {
        let answer = self.try_request("meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        let summary = answer.map_or_else(
            || "no answer".to_string(),
            |answer| head(&answer.to_string(), 200),
        );
        self.say(format!(
            "end of row: orderly shutdown asked, not asserted on: {summary}"
        ));
        drop(self.stdin.take());
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            self.pump();
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// The identity's status at the gateway, printed.
    fn read_status(&mut self, when: &str) -> Value {
        let status = self.call("meta/status_identity", json!({ "identity": identity() }));
        self.say(format!(
            "{when}: identity state={} session_id={} generation={} agent_runtime_id={} \
             member_id={} wake_refusal={}",
            status["state"],
            status["session_id"],
            status["generation"],
            status["agent_runtime_id"],
            status["member_id"],
            status.get("wake_refusal").map_or("absent", |_| "PRESENT"),
        ));
        status
    }

    /// The item as the WorkGraph store holds it, printed with its claim.
    fn read_item(&mut self, item_id: &str, when: &str) -> Value {
        let item = self.call("meta/workgraph/get", json!({ "id": item_id }))["item"].clone();
        let claim = &item["claim"];
        self.say(format!(
            "{when}: item id={} revision={} status={} description={} updated_at={}",
            item["id"],
            item["revision"],
            item["status"],
            head(item["description"].as_str().unwrap_or(""), 40),
            item["updated_at"],
        ));
        if claim.is_null() {
            self.say(format!("{when}: item claim: none"));
        } else {
            self.say(format!(
                "{when}: item claim owner={} claimed_at={} lease_expires_at={} \
                 expiry_observed_at={}",
                claim["owner"]["key"],
                claim["claimed_at"],
                claim["lease_expires_at"],
                claim["expiry_observed_at"],
            ));
        }
        item
    }

    /// Print how many claims `item` holds and whose, under `label`.
    fn say_claim(&self, label: &str, item: &Value) {
        let claim = &item["claim"];
        if claim.is_null() {
            self.say(format!("{label}: claims the item holds: 0"));
        } else {
            self.say(format!(
                "{label}: claims the item holds: 1; owner={} claimed_at={} lease_expires_at={} \
                 expiry_observed_at={}",
                claim["owner"]["key"],
                claim["claimed_at"],
                claim["lease_expires_at"],
                claim["expiry_observed_at"],
            ));
        }
    }

    /// Print the claim and release events of the WorkGraph ledger; the
    /// ledger's event kinds in order, or `None` when it was not read.
    fn say_claim_events(&mut self) -> Option<Vec<String>> {
        let Some(answer) = self.try_request("meta/workgraph/events", json!({}), RPC_TIMEOUT) else {
            self.say("WorkGraph ledger: NOT READ (no answer)");
            return None;
        };
        let events = answer["result"]["events"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let kinds = events
            .iter()
            .map(|event| event["kind"].as_str().unwrap_or("?").to_string())
            .collect::<Vec<_>>();
        self.say(format!(
            "WorkGraph ledger: {} events, of which claimed {} and released {} (kinds in order: \
             {}; truncated: {})",
            events.len(),
            kinds.iter().filter(|kind| *kind == "claimed").count(),
            kinds.iter().filter(|kind| *kind == "released").count(),
            kinds.join(" "),
            answer["result"]["truncated"],
        ));
        Some(kinds)
    }

    /// Every attention binding the WorkGraph store lists, printed.
    fn read_bindings(&mut self, when: &str) -> Vec<Value> {
        let listed = self.call("meta/workgraph/attention/list", json!({}));
        let bindings = listed["attention"].as_array().cloned().unwrap_or_default();
        self.say(format!(
            "{when}: attention bindings listed: {}",
            bindings.len()
        ));
        for binding in &bindings {
            self.say(format!(
                "{when}: binding id={} status={} revision={} item={} target={} updated_at={}",
                binding["binding_id"],
                binding["status"],
                binding["machine_state"]["revision"],
                binding["work_ref"]["item_id"],
                binding["target"],
                binding["updated_at"],
            ));
        }
        bindings
    }

    /// One tracked turn; returns the state its wait ended in.
    fn turn(&mut self, text: &str) -> Value {
        let ticket = self.send_tracked(text);
        let waited = self.request(
            "meta/wait_for_turn",
            json!({ "identity": identity(), "ticket": ticket, "timeout_ms": 90_000 }),
            Duration::from_mins(2),
        );
        self.say(format!(
            "tracked turn {:?}: wait ended with {}",
            head(text, 24),
            head(&waited.to_string(), 200)
        ));
        waited["result"]["state"].clone()
    }

    /// Send one tracked prompt and return its ticket without waiting.
    fn send_tracked(&mut self, text: &str) -> String {
        let sent = self.call(
            "meta/send",
            json!({ "identity": identity(), "content": text, "track_turn": true }),
        );
        sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("[{}] untracked turn: {sent}", self.label))
            .to_string()
    }

    /// Create a goal bound to the identity through the gateway's own method.
    fn create_goal(&mut self, description: &str) -> Goal {
        let result = self.call(
            "meta/workgraph/goal/create",
            json!({
                "title": "watch the queue",
                "description": description,
                "target": { "kind": "identity", "identity": identity() },
            }),
        );
        let goal = Goal {
            item_id: result["item"]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("goal/create returned no item id: {result}"))
                .to_string(),
            binding_id: result["attention"]["binding_id"]
                .as_str()
                .unwrap_or_else(|| panic!("goal/create returned no binding id: {result}"))
                .to_string(),
            owner_key: result["attention"]["target"]["owner_key"].clone(),
        };
        self.say(format!(
            "host write meta/workgraph/goal/create: item={} revision={} binding={} target={}",
            goal.item_id,
            result["item"]["revision"],
            goal.binding_id,
            result["attention"]["target"],
        ));
        // The lowering of the name: the member id the roster seats it under
        // and the owner key the goal's binding was stored with. A turn is
        // started for the binding's member only where the key names that id.
        let status = self.call("meta/status_identity", json!({ "identity": identity() }));
        let member_id = status["member_id"].as_str().unwrap_or_default();
        let key_of_member = format!("mob/{MOB_ID}/agent/{member_id}");
        self.say(format!(
            "lowering of the name {:?}: the member id it is seated under (the gateway's status): \
             {:?}; the stored owner key of the goal's binding: {}; the key is that member's own \
             ({key_of_member}): {} (printed, not asserted)",
            identity(),
            member_id,
            goal.owner_key,
            !member_id.is_empty() && goal.owner_key["id"] == key_of_member.as_str()
        ));
        goal
    }

    /// Revise the item's description through the gateway's own method.
    fn revise(&mut self, item_id: &str, description: &str) {
        let revision =
            self.call("meta/workgraph/get", json!({ "id": item_id }))["item"]["revision"].clone();
        let result = self.call(
            "meta/workgraph/update",
            json!({ "id": item_id, "expected_revision": revision, "description": description }),
        );
        self.say(format!(
            "host write meta/workgraph/update: item={item_id} revision {revision} -> {} \
             description={}",
            result["item"]["revision"],
            head(description, 40),
        ));
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Goal {
    item_id: String,
    binding_id: String,
    owner_key: Value,
}

/// One state directory with its host configuration and its own model.
struct Scene {
    row: &'static str,
    model: Model,
    _scratch: tempfile::TempDir,
    state: PathBuf,
    host_config: PathBuf,
}

impl Scene {
    fn new(row: &'static str, label: &str) -> Self {
        let model = Model::start();
        let scratch = tempfile::tempdir().expect("scratch dir");
        let state = scratch.path().join("state");
        let host_config = scratch.path().join("config.toml");
        std::fs::write(&host_config, host_config_text(&model.base_url)).expect("write host config");
        say(
            row,
            format!(
                "[{label}] model: a loopback scripted endpoint run by the test; no provider \
                 ({})",
                model.base_url
            ),
        );
        say(
            row,
            format!("[{label}] state directory: {}", state.display()),
        );
        Self {
            row,
            model,
            _scratch: scratch,
            state,
            host_config,
        }
    }

    fn boot(&self, label: &str, pursue: bool) -> Gateway {
        let mut gateway = Gateway::start(self.row, label, pursue);
        let response = gateway.request(
            "meta/init",
            json!({
                "persistent_state": self.state,
                "mob_config": mob_config(),
                "has_roster_provider": true,
                "runtime_options": {
                    "meerkat_config_path": self.host_config,
                    "identity_bootstrap_mode": { "mode": "eager_materialize" }
                }
            }),
            INIT_TIMEOUT,
        );
        assert!(
            response.get("error").is_none(),
            "[{label}] init failed: {response}\n{}",
            gateway.stderr_tail()
        );
        // The pursuit attaches inside init; its stderr line is read by
        // another thread, so wait for it where one is expected.
        let stderr = Arc::clone(&gateway.stderr);
        let _ = gateway.eventually(Duration::from_secs(5), || {
            !pursue
                || stderr
                    .lock()
                    .is_ok_and(|sink| sink.iter().any(|line| line.contains("attached to mob")))
        });
        if !pursue {
            gateway.settle(Duration::from_secs(2));
        }
        let lines = gateway.vehicle_lines();
        gateway.say(format!(
            "init answered; pursuit lines on the gateway's stderr: {}",
            lines.len()
        ));
        for line in &lines {
            gateway.say(format!("stderr: {}", head(line, 160)));
        }
        gateway
    }

    /// One line: how many requests the model has seen at this stage, how
    /// many of them start from a notice and how many from the held prompt.
    fn say_turns(&self, label: &str, stage: &str) {
        let requests = self.model.requests();
        say(
            self.row,
            format!(
                "[{label}] TURNS {stage}: model requests seen {}, starting from a notice {}, \
                 starting from the held prompt {}",
                requests.len(),
                notice_starts(&requests, 0).len(),
                requests
                    .iter()
                    .filter(|request| last_message(request).contains(HELD_PROMPT))
                    .count()
            ),
        );
    }

    /// Print every request the model has seen from `from` on.
    fn print_requests(&self, label: &str, from: usize) -> Vec<Value> {
        let requests = self.model.requests();
        say(
            self.row,
            format!(
                "[{label}] model requests seen so far: {} (printing from #{from})",
                requests.len()
            ),
        );
        for (index, request) in requests.iter().enumerate().skip(from) {
            say(self.row, format!("[{label}] {}", digest(index, request)));
        }
        requests
    }
}

// ---------------------------------------------------------------------------
// The runtime store, read from the test process without writing
// ---------------------------------------------------------------------------

/// One stored input of the runtime store.
struct StoredInput {
    input_id: String,
    current_state: String,
    carries_notice: bool,
    carries_original: bool,
    carries_revised: bool,
    carries_later: bool,
    carries_held_prompt: bool,
    bytes: usize,
}

impl StoredInput {
    /// Whether the input still waits or runs: not consumed, superseded,
    /// coalesced or abandoned.
    fn is_open(&self) -> bool {
        !matches!(
            self.current_state.as_str(),
            "consumed" | "superseded" | "coalesced" | "abandoned"
        )
    }
}

fn find_file(root: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        } else if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
    }
    if depth == 0 {
        return None;
    }
    directories
        .into_iter()
        .find_map(|directory| find_file(&directory, name, depth - 1))
}

/// The runtime store's input rows and its count of runtime state rows, or
/// why they could not be read.
fn stored_inputs(state: &Path) -> Result<(Vec<StoredInput>, i64), String> {
    let path = find_file(state, "runtime.sqlite", 3)
        .ok_or_else(|| "no runtime.sqlite under the state directory".to_string())?;
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("busy timeout: {error}"))?;
    let mut statement = connection
        .prepare("SELECT input_id, state_json FROM runtime_input_states ORDER BY rowid")
        .map_err(|error| format!("prepare the input read: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            let input_id: String = row.get(0)?;
            let bytes = row.get_ref(1)?.as_bytes().map(<[u8]>::to_vec);
            Ok((input_id, bytes.unwrap_or_default()))
        })
        .map_err(|error| format!("read the inputs: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read an input row: {error}"))?;
    let inputs = rows
        .into_iter()
        .map(|(input_id, bytes)| {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let current_state = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| value["current_state"].as_str().map(str::to_string))
                .unwrap_or_else(|| "UNREAD".to_string());
            StoredInput {
                input_id,
                current_state,
                carries_notice: text.contains(NOTICE_PHRASE),
                carries_original: text.contains(ORIGINAL),
                carries_revised: text.contains(REVISED),
                carries_later: text.contains(LATER),
                carries_held_prompt: text.contains(HELD_PROMPT),
                bytes: bytes.len(),
            }
        })
        .collect();
    let runtime_states = connection
        .query_row("SELECT COUNT(*) FROM runtime_states", [], |row| row.get(0))
        .map_err(|error| format!("count the runtime states: {error}"))?;
    Ok((inputs, runtime_states))
}

/// Print what the runtime store holds and return its input rows when the
/// store could be read.
fn read_store(row: &str, label: &str, state: &Path, when: &str) -> Option<Vec<StoredInput>> {
    match stored_inputs(state) {
        Ok((inputs, runtime_states)) => {
            say(
                row,
                format!(
                    "[{label}] {when}: runtime store holds {} input rows ({} open), {} runtime \
                     state rows",
                    inputs.len(),
                    inputs
                        .iter()
                        .filter(|input| StoredInput::is_open(input))
                        .count(),
                    runtime_states
                ),
            );
            for input in &inputs {
                say(
                    row,
                    format!(
                        "[{label}] {when}: input {} state={} bytes={} notice={} original={} \
                         revised={} later={} held_prompt={}",
                        input.input_id,
                        input.current_state,
                        input.bytes,
                        input.carries_notice,
                        input.carries_original,
                        input.carries_revised,
                        input.carries_later,
                        input.carries_held_prompt
                    ),
                );
            }
            Some(inputs)
        }
        Err(error) => {
            say(
                row,
                format!("[{label}] {when}: runtime store NOT READ: {error}"),
            );
            None
        }
    }
}

/// Whether the store holds an open input that carries the attention notice
/// under `token`: a continuation for that revision that waits or runs.
fn open_continuation_under(state: &Path, token: &str) -> bool {
    stored_inputs(state).is_ok_and(|(inputs, _)| {
        inputs.iter().any(|input| {
            input.is_open()
                && input.carries_notice
                && match token {
                    REVISED => input.carries_revised,
                    LATER => input.carries_later,
                    _ => input.carries_original,
                }
        })
    })
}

/// The ids of the store's open inputs; none when the store cannot be read.
fn open_input_ids(state: &Path) -> Vec<String> {
    stored_inputs(state)
        .map(|(inputs, _)| {
            inputs
                .into_iter()
                .filter(StoredInput::is_open)
                .map(|input| input.input_id)
                .collect()
        })
        .unwrap_or_default()
}

fn notice_starts(requests: &[Value], from: usize) -> Vec<usize> {
    requests
        .iter()
        .enumerate()
        .skip(from)
        .filter(|(_, request)| starts_from_notice(request))
        .map(|(index, _)| index)
        .collect()
}

// ---------------------------------------------------------------------------
// Row 1
// ---------------------------------------------------------------------------

/// Row 1. With pursuit composed, a goal the host creates for a seated member
/// that was never prompted starts exactly one turn, from the attention
/// notice, under WorkGraph tools. The same binary without the environment
/// value starts none for the same goal.
#[test]
fn a_host_goal_wakes_a_never_prompted_member_and_starts_no_turn_without_the_value() {
    let row = "row1";
    say(
        row,
        format!("gateway binary: {}", env!("CARGO_BIN_EXE_rpc_gateway")),
    );
    say_scene(row);

    // The wake: pursuit composed.
    let on = Scene::new(row, "on");
    let mut gateway = on.boot("on", true);
    let on_vehicle_lines = gateway.vehicle_lines();
    let on_status_before = gateway.read_status("before the host's write");
    let on_bindings_before = gateway.read_bindings("before the host's write");
    let on_requests_before = on.model.seen();
    gateway.say(format!(
        "before the host's write: model requests seen: {on_requests_before}"
    ));
    let on_store_before = read_store(row, "on", &on.state, "before the host's write");
    let goal = gateway.create_goal(ORIGINAL);
    let written = Instant::now();
    let on_woke = gateway.eventually(WAKE_WAIT, || on.model.seen() > 0);
    gateway.say(format!(
        "after the host's write: a model request arrived within the wait: {on_woke} (waited \
         {:?} of {WAKE_WAIT:?})",
        written.elapsed()
    ));
    gateway.settle(SETTLE);
    let on_requests = on.print_requests("on", 0);
    let on_status_after = gateway.read_status("after the wait");
    gateway.read_item(&goal.item_id, "after the wait");
    let on_bindings_after = gateway.read_bindings("after the wait");
    read_store(row, "on", &on.state, "after the wait");
    gateway.print_stderr("after the wait");
    gateway.say(format!(
        "session before={} after={}; binding={}; the test sent no prompt in this arm",
        on_status_before["session_id"], on_status_after["session_id"], goal.binding_id
    ));
    gateway.shutdown_best_effort();
    drop(gateway);

    // The control: the same binary, a second state directory, no value.
    let off = Scene::new(row, "off");
    let mut gateway = off.boot("off", false);
    let off_vehicle_lines = gateway.vehicle_lines();
    gateway.read_status("before the host's write");
    let off_requests_before = off.model.seen();
    gateway.say(format!(
        "before the host's write: model requests seen: {off_requests_before}"
    ));
    read_store(row, "off", &off.state, "before the host's write");
    let off_goal = gateway.create_goal(ORIGINAL);
    gateway.settle(CONTROL_QUIET);
    let off_requests_after_goal = off.model.seen();
    gateway.say(format!(
        "after the host's write and {CONTROL_QUIET:?}: model requests seen: \
         {off_requests_after_goal}"
    ));
    gateway.read_item(&off_goal.item_id, "after the quiet wait");
    gateway.read_bindings("after the quiet wait");
    read_store(row, "off", &off.state, "after the quiet wait");
    gateway.print_stderr("after the quiet wait");
    let off_turn = gateway.turn(PROBE_PROMPT);
    let off_requests = off.print_requests("off", 0);
    gateway.read_status("after the prompt");
    gateway.shutdown_best_effort();
    drop(gateway);

    // The row's two readings, each by itself, printed before any assertion.
    let never_prompted = on_status_before["state"] == json!("active")
        && on_bindings_before.is_empty()
        && on_requests_before == 0
        && on_store_before.as_ref().is_none_or(Vec::is_empty);
    let idle_wake = never_prompted
        && on_requests.len() == 1
        && on_requests.first().is_some_and(|wake| {
            starts_from_notice(wake)
                && last_message(wake).contains(ORIGINAL)
                && tool_names(wake)
                    .iter()
                    .any(|tool| tool.starts_with("workgraph_"))
        })
        && on_bindings_after.len() == 1;
    let off_control = off_requests_before == 0
        && off_requests_after_goal == 0
        && off_turn == json!("completed")
        && off_requests
            .first()
            .is_some_and(|request| last_message(request).contains(PROBE_PROMPT))
        && notice_starts(&off_requests, 0).is_empty();
    say(
        row,
        format!(
            "READING (a) idle wake (a seated, never prompted member takes exactly one turn from \
             the notice after the host's committed goal): {}",
            verdict(idle_wake)
        ),
    );
    say(
        row,
        format!(
            "READING (b) the off control (the same binary without the value starts no turn for \
             the same goal; a later prompt's request is the model's first): {}",
            verdict(off_control)
        ),
    );

    // Harness preconditions: the arms differ by the pursuit and nothing else.
    assert!(
        on_vehicle_lines
            .iter()
            .any(|line| line.contains("attached to mob")),
        "harness: the gateway started with {ENABLE_ENV}=all did not attach pursuit; its lines: \
         {on_vehicle_lines:?}"
    );
    assert!(
        off_vehicle_lines.is_empty(),
        "harness: the control gateway composed pursuit without the value: {off_vehicle_lines:?}"
    );

    // The member was seated and never prompted before the write.
    assert_eq!(
        on_status_before["state"],
        json!("active"),
        "the identity must be seated before the write: {on_status_before}"
    );
    assert!(
        on_bindings_before.is_empty(),
        "no binding exists before the write: {on_bindings_before:?}"
    );
    assert_eq!(
        on_requests_before, 0,
        "the model must have seen no request before the host's write"
    );
    if let Some(inputs) = on_store_before {
        assert!(
            inputs.is_empty(),
            "the runtime store must hold no input before the host's write"
        );
    }

    // The wake.
    assert_eq!(
        on_requests.len(),
        1,
        "the committed goal must start exactly one turn of the member that was never prompted \
         (the test sent no prompt); the model saw {} requests",
        on_requests.len()
    );
    let wake = &on_requests[0];
    assert!(
        starts_from_notice(wake),
        "the one turn must start from the attention notice: {}",
        digest(0, wake)
    );
    assert!(
        last_message(wake).contains(ORIGINAL),
        "the notice must carry the goal as created: {}",
        digest(0, wake)
    );
    assert!(
        tool_names(wake)
            .iter()
            .any(|tool| tool.starts_with("workgraph_")),
        "the wake turn must be offered WorkGraph tools: {:?}",
        tool_names(wake)
    );
    assert_eq!(
        on_bindings_after.len(),
        1,
        "the goal's binding is the store's only one: {on_bindings_after:?}"
    );

    // The control.
    assert_eq!(
        off_requests_before, 0,
        "control: no request before the write"
    );
    assert_eq!(
        off_requests_after_goal, 0,
        "control: without the value the same committed goal must start no turn"
    );
    assert_eq!(
        off_turn,
        json!("completed"),
        "control: the later prompt's turn must complete"
    );
    assert!(
        off_requests
            .first()
            .is_some_and(|request| last_message(request).contains(PROBE_PROMPT)),
        "control: the later prompt's request must be the first the model sees"
    );
    assert!(
        notice_starts(&off_requests, 0).is_empty(),
        "control: no turn may start from an attention notice"
    );
}

// ---------------------------------------------------------------------------
// Row 2
// ---------------------------------------------------------------------------

/// Row 2. A continuation that waits behind a running turn when the gateway
/// process is killed takes exactly one turn after a second boot on the same
/// state directory, under the item as last revised, with no write and no
/// prompt after the kill. That turn then makes two writes of its own, one
/// under the claim the restart kept and then that claim's release, which
/// start no further turn; one more revision then starts exactly one more.
#[test]
fn a_continuation_that_waits_at_a_kill_takes_one_turn_after_the_second_boot() {
    let row = "row2";
    say(
        row,
        format!("gateway binary: {}", env!("CARGO_BIN_EXE_rpc_gateway")),
    );
    say_scene(row);
    let scene = Scene::new(row, "boot-1");
    let mut gateway = scene.boot("boot-1", true);
    let first_vehicle_lines = gateway.vehicle_lines();
    gateway.read_status("before any write");

    // Setup: the goal (its wake turn), then a claim in the member's own name.
    let goal = gateway.create_goal(ORIGINAL);
    let goal_woke = gateway.eventually(WAKE_WAIT, || {
        !notice_starts(&scene.model.requests(), 0).is_empty()
    });
    gateway.say(format!(
        "setup: the goal's wake turn reached the model: {goal_woke}"
    ));
    gateway.settle(Duration::from_secs(3));
    scene.say_turns("boot-1", "after the goal's own wake turn");
    let before_claim = scene.model.seen();
    let item = gateway.read_item(&goal.item_id, "setup, before the claim");
    let claimed = gateway.call(
        "meta/workgraph/claim",
        json!({
            "id": goal.item_id,
            "expected_revision": item["revision"],
            "owner": goal.owner_key,
            "lease_seconds": 3600
        }),
    );
    gateway.say(format!(
        "host write meta/workgraph/claim in the member's own owner key {} (written by the host \
         through the gateway's method, not made by the member): item revision -> {}",
        goal.owner_key, claimed["item"]["revision"]
    ));
    let claim_woke = gateway.eventually(Duration::from_secs(30), || {
        !notice_starts(&scene.model.requests(), before_claim).is_empty()
    });
    gateway.say(format!(
        "setup: the claim's own wake turn reached the model within 30 s: {claim_woke} (printed, \
         not asserted)"
    ));
    gateway.settle(Duration::from_secs(3));
    scene.say_turns("boot-1", "after the claim's own wake turn");

    // The scene: a prompt turn held at the model, one revision while it runs.
    scene.model.hold_requests_carrying(HELD_PROMPT);
    let before_prompt = scene.model.seen();
    let ticket = gateway.send_tracked(HELD_PROMPT);
    let prompt_held = gateway.eventually(WAKE_WAIT, || {
        scene.model.requests()[before_prompt..]
            .iter()
            .any(|request| last_message(request).contains(HELD_PROMPT))
    });
    gateway.say(format!(
        "the prompt turn (ticket {ticket}) reached the model and is held there: {prompt_held}"
    ));
    scene.say_turns("boot-1", "with the prompt turn held");
    let open_before_revision = open_input_ids(&scene.state);
    gateway.revise(&goal.item_id, REVISED);
    let continuation_waits = gateway.eventually(Duration::from_secs(45), || {
        open_input_ids(&scene.state)
            .iter()
            .any(|id| !open_before_revision.contains(id))
    });
    let continuation_under_revision = gateway.eventually(Duration::from_secs(15), || {
        open_continuation_under(&scene.state, REVISED)
    });
    gateway.say(format!(
        "an input admitted after the revision is open in the store while the prompt turn runs: \
         {continuation_waits}; an open input carries the notice and the revision: \
         {continuation_under_revision}"
    ));

    // Readings before the kill.
    let status_before = gateway.read_status("before the kill");
    let item_before = gateway.read_item(&goal.item_id, "before the kill");
    let bindings_before = gateway.read_bindings("before the kill");
    let store_before = read_store(row, "boot-1", &scene.state, "before the kill");
    let requests_at_kill = scene.print_requests("boot-1", 0).len();
    let notice_turn_at_kill = !notice_starts(&scene.model.requests(), before_prompt).is_empty();
    gateway.say(format!(
        "before the kill: a turn started from a notice since the prompt was sent: \
         {notice_turn_at_kill}"
    ));
    gateway.print_stderr("before the kill");
    scene.say_turns("boot-1", "before the kill");
    gateway.say_claim("ORIGINAL claim (before the kill)", &item_before);

    // The scene must be the one the row is about before the process is
    // killed: what the second boot does with a continuation that never
    // waited says nothing about one that does.
    let scene_faults = [
        (
            !goal_woke,
            "the goal's own wake turn never reached the model",
        ),
        (
            !prompt_held,
            "the prompt turn never reached the model, so nothing ran at the kill",
        ),
        (
            !continuation_waits,
            "the store held no open input admitted after the revision while the prompt turn ran",
        ),
        (
            !continuation_under_revision,
            "no open stored input held the notice's sentence and the revision's token while the \
             prompt turn ran",
        ),
        (
            notice_turn_at_kill,
            "a turn started from a notice while the prompt turn was held",
        ),
    ]
    .into_iter()
    .filter(|(fault, _)| *fault)
    .map(|(_, text)| text)
    .collect::<Vec<_>>();
    if !scene_faults.is_empty() {
        say(
            row,
            "READING (c) the same claim, binding and session after the restart with exactly one \
             continuation turn: not reached (the scene did not come about before the kill)",
        );
    }
    assert!(
        scene_faults.is_empty(),
        "scene: the row's scene did not come about before the kill, so the process was not \
         killed and nothing after it was read: {scene_faults:?}\n{}",
        gateway.stderr_tail()
    );

    // The kill: no shutdown request, the process is killed and reaped.
    let exit = gateway.kill();
    gateway.say(format!(
        "stop mode: kill of the gateway process (Child::kill, SIGKILL on this platform), no \
         shutdown request sent; exit: {exit}"
    ));
    drop(gateway);
    scene.model.release();
    // The continuation's turn after the second boot waits at the model, so
    // that the claim is read as the restart left it before that turn writes.
    scene.model.script_calls_on_notice(REVISED, 2);
    read_store(row, "between", &scene.state, "after the kill, no process");
    scene.say_turns("between", "after the kill, no process");

    // The second boot on the same state directory. From here the test makes
    // no write and sends no prompt until the continuation's turn is read.
    let mut gateway = scene.boot("boot-2", true);
    let second_vehicle_lines = gateway.vehicle_lines();
    let recovered = gateway.eventually(RECOVERY_WAIT, || {
        !notice_starts(&scene.model.requests(), requests_at_kill).is_empty()
    });
    gateway.say(format!(
        "after the second boot, with no write and no prompt: a turn started from a notice \
         within {RECOVERY_WAIT:?}: {recovered}"
    ));

    if !recovered {
        // Read on, so the failure says which fact it is.
        scene.print_requests("boot-2", requests_at_kill);
        let status = gateway.read_status("no continuation turn after the second boot");
        read_store(row, "boot-2", &scene.state, "no continuation turn");
        let before_probe = scene.model.seen();
        let probe = gateway.try_request(
            "meta/send",
            json!({ "identity": identity(), "content": PROBE_PROMPT, "track_turn": true }),
            RPC_TIMEOUT,
        );
        gateway.say(format!(
            "read on: one prompt sent; the gateway answered: {}",
            probe.map_or_else(
                || "no answer".to_string(),
                |answer| head(&answer.to_string(), 300)
            )
        ));
        let after_prompt = gateway.eventually(WAKE_WAIT, || {
            !notice_starts(&scene.model.requests(), before_probe).is_empty()
        });
        scene.print_requests("boot-2", requests_at_kill);
        gateway.read_status("after the read-on prompt");
        read_store(row, "boot-2", &scene.state, "after the read-on prompt");
        gateway.print_stderr("after the read-on prompt");
        scene.say_turns("boot-2", "after the read-on prompt");
        let item_after = gateway.read_item(&goal.item_id, "after the read-on prompt");
        gateway.say_claim("RECOVERED claim (after the second boot)", &item_after);
        gateway.read_bindings("after the read-on prompt");
        gateway.say_claim_events();
        say(
            row,
            "READING (c) the same claim, binding and session after the restart with exactly one \
             continuation turn: fail (no continuation turn after the second boot)",
        );
        let fact = if after_prompt {
            "the waiting input was kept and its member was not brought up at the start: the \
             continuation's turn ran only after a later prompt"
        } else {
            "the waiting input was not taken up: no continuation turn ran after the second \
             boot, nor after a later prompt"
        };
        panic!(
            "{fact}\nidentity status at the gateway: {status}\n{}",
            gateway.stderr_tail()
        );
    }

    // Readings after the second boot. The continuation's turn waits at the
    // model for its scripted answer, so they are taken before it writes.
    gateway.settle(SETTLE);
    gateway.say(
        "the readings after the second boot are taken while the continuation's turn waits at \
         the model for its answer: before that turn writes anything",
    );
    scene.say_turns("boot-2", "after the second boot settled");
    let after_restart = scene.print_requests("boot-2", requests_at_kill);
    let recovered_starts = notice_starts(&after_restart, requests_at_kill);
    let prompt_requests_after_restart = after_restart[requests_at_kill..]
        .iter()
        .filter(|request| last_message(request).contains(HELD_PROMPT))
        .count();
    let first_recovered = &after_restart[recovered_starts[0]];
    gateway.say(format!(
        "(a) requests after the second boot that start from a notice: {} (indexes \
         {recovered_starts:?})",
        recovered_starts.len()
    ));
    gateway.say(format!(
        "(b) the first of them carries the revision in its notice: {}; carries the earlier \
         description in its notice: {}",
        last_message(first_recovered).contains(REVISED),
        last_message(first_recovered).contains(ORIGINAL)
    ));
    gateway.say(format!(
        "(c) pinned as found, not asserted: requests after the second boot that start from the \
         held prompt: {prompt_requests_after_restart}; the notice's request also carries the \
         held prompt in its transcript: {}",
        transcript(first_recovered).contains(HELD_PROMPT)
    ));
    let status_after = gateway.read_status("after the second boot");
    let item_after = gateway.read_item(&goal.item_id, "after the second boot");
    let bindings_after = gateway.read_bindings("after the second boot");
    read_store(row, "boot-2", &scene.state, "after the second boot");
    gateway.print_stderr("after the second boot");
    gateway.say_resume_gate_lines("after the second boot");
    gateway.say(format!(
        "session before the kill={} after the second boot={} unchanged={}",
        status_before["session_id"],
        status_after["session_id"],
        status_before["session_id"] == status_after["session_id"]
    ));
    gateway.say(format!(
        "binding ids before the kill={:?} after the second boot={:?}",
        bindings_before
            .iter()
            .map(|binding| binding["binding_id"].to_string())
            .collect::<Vec<_>>(),
        bindings_after
            .iter()
            .map(|binding| binding["binding_id"].to_string())
            .collect::<Vec<_>>()
    ));
    gateway.say(format!(
        "claim unchanged across the kill and the second boot (printed, not asserted): {}; item \
         revision before={} after={}",
        item_before["claim"] == item_after["claim"],
        item_before["revision"],
        item_after["revision"]
    ));
    gateway.say_claim("ORIGINAL claim (before the kill)", &item_before);
    gateway.say_claim("RECOVERED claim (after the second boot)", &item_after);
    gateway.say_claim_events();
    let claim_same = !item_before["claim"].is_null() && item_before["claim"] == item_after["claim"];
    let claim_valid = item_after["claim"]["expiry_observed_at"].is_null()
        && item_after["claim"]["lease_expires_at"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .is_some_and(|at| at > chrono::Utc::now());
    let binding_same = !bindings_before.is_empty()
        && bindings_before.len() == bindings_after.len()
        && bindings_before
            .iter()
            .zip(&bindings_after)
            .all(|(before, after)| {
                ["binding_id", "status", "target", "work_ref"]
                    .iter()
                    .all(|member| before[member] == after[member])
            });
    let session_same = !status_before["session_id"].is_null()
        && status_before["session_id"] == status_after["session_id"];
    let one_continuation = recovered_starts.len() == 1
        && last_message(first_recovered).contains(REVISED)
        && !last_message(first_recovered).contains(ORIGINAL);
    gateway.say(format!(
        "ORIGINAL against RECOVERED: claim the same (owner, claimed_at, lease times): \
         {claim_same}; the recovered claim's lease is live and no expiry was observed (read at \
         {}): {claim_valid}; bindings the same (id, status, target, item): {binding_same}; \
         session the same: {session_same}; exactly one turn from a notice after the restart, \
         under the revision: {one_continuation}; replays of the held prompt after the restart \
         (not counted as a continuation): {prompt_requests_after_restart}",
        chrono::Utc::now().to_rfc3339()
    ));
    say(
        row,
        format!(
            "READING (c) the same claim, binding and session after the restart with exactly one \
             continuation turn: {}",
            verdict(claim_same && claim_valid && binding_same && session_same && one_continuation)
        ),
    );

    // The continuation's first write. Its turn has waited at the model since
    // the second boot; it is now answered with one call of the member's own
    // `workgraph_add_evidence` on the claimed item. The turn's next request,
    // which carries the tool's answer, waits at the model in its turn, so
    // the item, its claim and the ledger are read after this one write and
    // before any other: the write lands on the item under the claim the
    // restart kept, with no release and no second claim before it.
    let before_first = scene.model.seen();
    let evidence = json!({
        "id": goal.item_id,
        "expected_revision": item_after["revision"],
        "evidence": {
            "kind": "finding",
            "id": "continuation-after-restart",
            "summary": "written by the continuation's turn after the second boot"
        }
    });
    scene
        .model
        .answer_scripted_call("workgraph_add_evidence", &evidence);
    gateway.say(format!(
        "the continuation's turn, which waited at the model since the second boot, is answered \
         with its first tool call: workgraph_add_evidence {evidence}"
    ));
    let first_answered = gateway.eventually(WAKE_WAIT, || {
        tool_result(&scene.model.requests(), before_first).is_some()
    });
    let after_first = scene.print_requests("boot-2", requests_at_kill);
    let first_result = tool_result(&after_first, before_first);
    gateway.say(format!(
        "the turn's next request carries the tool's answer as its newest message within \
         {WAKE_WAIT:?}: {first_answered}; {}",
        first_result.as_ref().map_or_else(
            || "no such request".to_string(),
            |(index, text)| format!("request #{index}, the answer: {}", head(text, 600))
        )
    ));
    let answered_item = first_result
        .as_ref()
        .and_then(|(_, text)| serde_json::from_str::<Value>(text).ok())
        .map_or(Value::Null, |answer| answer["item"].clone());
    let item_tied = gateway.read_item(&goal.item_id, "after the continuation's first write");
    gateway.say_claim("claim after the continuation's first write", &item_tied);
    let kinds_tied = gateway.say_claim_events().unwrap_or_default();
    let count = |kinds: &[String], kind: &str| kinds.iter().filter(|k| *k == kind).count();
    let answer_tied = !item_before["claim"].is_null()
        && answered_item["id"] == goal.item_id.as_str()
        && answered_item["claim"] == item_before["claim"]
        && answered_item["revision"].as_u64()
            == item_after["revision"].as_u64().map(|revision| revision + 1);
    let store_tied = !item_before["claim"].is_null()
        && item_tied["claim"] == item_before["claim"]
        && item_tied["revision"] == answered_item["revision"];
    let ledger_tied = count(&kinds_tied, "claimed") == 1
        && count(&kinds_tied, "released") == 0
        && count(&kinds_tied, "evidence_added") == 1;
    gateway.say(format!(
        "THE TIE, the tool's own answer: item {} (the goal's item: {}); claim owner={} \
         claimed_at={} lease_expires_at={} expiry_observed_at={}; the claim equals the one read \
         before the kill: {}; item revision before the call={} in the answer={}; status={}; \
         evidence the answer holds: {}",
        answered_item["id"],
        answered_item["id"] == goal.item_id.as_str(),
        answered_item["claim"]["owner"]["key"],
        answered_item["claim"]["claimed_at"],
        answered_item["claim"]["lease_expires_at"],
        answered_item["claim"]["expiry_observed_at"],
        !item_before["claim"].is_null() && answered_item["claim"] == item_before["claim"],
        item_after["revision"],
        answered_item["revision"],
        answered_item["status"],
        head(&answered_item["evidence"].to_string(), 300)
    ));
    gateway.say(format!(
        "THE TIE, the store after that write and before any other: the claim equals the one \
         read before the kill: {}; item revision={}; ledger events claimed={} released={} \
         evidence_added={}",
        !item_before["claim"].is_null() && item_tied["claim"] == item_before["claim"],
        item_tied["revision"],
        count(&kinds_tied, "claimed"),
        count(&kinds_tied, "released"),
        count(&kinds_tied, "evidence_added")
    ));
    say(
        row,
        format!(
            "READING (d) the continuation's first write landed on the original item under the \
             claim the restart kept (same owner, claimed_at and lease), with no release and no \
             second claim in the ledger: {}",
            verdict(first_answered && answer_tied && store_tied && ledger_tied)
        ),
    );

    // The continuation's second write, in the same turn: the member's own
    // `workgraph_release`. The item's lifecycle admits a release only while
    // a claim is present. The call names the item and its revision only:
    // whose claim it is, is no part of it.
    let before_second = scene.model.seen();
    let release = json!({ "id": goal.item_id, "expected_revision": item_tied["revision"] });
    scene
        .model
        .answer_scripted_call("workgraph_release", &release);
    gateway.say(format!(
        "the same turn is answered with its second tool call: workgraph_release {release}"
    ));
    let second_answered = gateway.eventually(WAKE_WAIT, || {
        tool_result(&scene.model.requests(), before_second).is_some()
    });
    gateway.settle(SETTLE);
    let after_calls = scene.print_requests("boot-2", requests_at_kill);
    let second_result = tool_result(&after_calls, before_second);
    gateway.say(format!(
        "the turn's next request carries the tool's answer as its newest message within \
         {WAKE_WAIT:?}: {second_answered}; {}",
        second_result.as_ref().map_or_else(
            || "no such request".to_string(),
            |(index, text)| format!("request #{index}, the answer: {}", head(text, 600))
        )
    ));
    let item_written = gateway.read_item(&goal.item_id, "after the continuation's writes");
    gateway.say_claim("claim after the continuation's writes", &item_written);
    let kinds_written = gateway.say_claim_events().unwrap_or_default();
    gateway.print_stderr("after the continuation's writes");
    scene.say_turns("boot-2", "after the continuation's writes");
    let starts_after_calls = notice_starts(&after_calls, requests_at_kill);
    let claim_released = !item_tied["claim"].is_null()
        && item_written["claim"].is_null()
        && item_written["revision"].as_u64()
            == item_tied["revision"].as_u64().map(|revision| revision + 1)
        && count(&kinds_written, "claimed") == 1
        && count(&kinds_written, "released") == 1;
    gateway.say(format!(
        "the continuation's second write: a claim was held before it: {}; none is held after \
         it: {}; item revision before={} after={}; ledger events claimed={} released={}; \
         requests after the second boot that start from a notice, counted after both writes: \
         {} (indexes {starts_after_calls:?})",
        !item_tied["claim"].is_null(),
        item_written["claim"].is_null(),
        item_tied["revision"],
        item_written["revision"],
        count(&kinds_written, "claimed"),
        count(&kinds_written, "released"),
        starts_after_calls.len()
    ));
    say(
        row,
        format!(
            "READING (e) the same turn then released that claim by its own tool call, and \
             neither write started a further turn: {}",
            verdict(second_answered && claim_released && starts_after_calls.len() == 1)
        ),
    );

    // The tail: one more revision starts one more turn.
    let before_tail = scene.model.seen();
    gateway.revise(&goal.item_id, LATER);
    let tail_woke = gateway.eventually(WAKE_WAIT, || {
        !notice_starts(&scene.model.requests(), before_tail).is_empty()
    });
    gateway.say(format!(
        "tail: a turn started from a notice after one more revision: {tail_woke}"
    ));
    gateway.settle(SETTLE);
    let after_tail = scene.print_requests("boot-2", before_tail);
    let tail_starts = notice_starts(&after_tail, before_tail);
    gateway.say(format!(
        "tail: requests after the later revision that start from a notice: {} (indexes \
         {tail_starts:?})",
        tail_starts.len()
    ));
    gateway.read_item(&goal.item_id, "after the tail");
    read_store(row, "boot-2", &scene.state, "after the tail");
    gateway.print_stderr("after the tail");
    scene.say_turns("boot-2", "after the tail");
    gateway.shutdown_best_effort();
    drop(gateway);

    // Harness preconditions.
    for (boot, lines) in [
        ("first", &first_vehicle_lines),
        ("second", &second_vehicle_lines),
    ] {
        assert!(
            lines.iter().any(|line| line.contains("attached to mob")),
            "harness: the {boot} boot did not attach pursuit; its lines: {lines:?}"
        );
    }
    assert!(
        goal_woke,
        "scene: the goal's own wake turn never reached the model"
    );
    assert!(
        prompt_held,
        "scene: the prompt turn never reached the model, so nothing ran at the kill"
    );
    assert!(
        continuation_waits,
        "scene: the store held no open input admitted after the revision before the kill; the \
         printed store rows say whether it could be read: {}",
        store_before.map_or_else(
            || "store not read".to_string(),
            |inputs| format!("{} rows", inputs.len())
        )
    );
    assert!(
        continuation_under_revision,
        "scene: no open stored input held the notice's sentence and the revision's token before \
         the kill"
    );
    assert!(
        !notice_turn_at_kill,
        "scene: the continuation must wait at the kill, not have run before it"
    );

    // The row's assertions.
    assert_eq!(
        recovered_starts.len(),
        1,
        "(a) after the second boot, with no write and no prompt, exactly one turn must start \
         from the notice; the restart alone must start no other"
    );
    assert!(
        last_message(first_recovered).contains(REVISED)
            && !last_message(first_recovered).contains(ORIGINAL),
        "(b) that turn must run under the item as revised: {}",
        digest(recovered_starts[0], first_recovered)
    );
    assert!(
        first_answered && second_answered,
        "(d) (e) the continuation's turn must make its two tool calls and read each answer: \
         first {first_answered}, second {second_answered}"
    );
    assert!(
        answer_tied && store_tied && ledger_tied,
        "(d) the continuation's first write must land on the original item under the claim the \
         restart kept, with no release and no second claim: the tool's answer {answer_tied}, \
         the store {store_tied}, the ledger {ledger_tied} ({kinds_tied:?}); claim before the \
         kill {}, in the answer {}, in the store {}",
        item_before["claim"],
        answered_item["claim"],
        item_tied["claim"]
    );
    assert!(
        claim_released,
        "(e) the continuation's turn must then release that claim: claim before the call {}, \
         after it {}; item revision before {} and after {}; ledger {kinds_written:?}",
        item_tied["claim"], item_written["claim"], item_tied["revision"], item_written["revision"]
    );
    assert_eq!(
        starts_after_calls.len(),
        1,
        "(a) through the end of the continuation's turn and its own writes, still exactly one \
         turn must have started from the notice after the second boot"
    );
    assert_eq!(
        tail_starts.len(),
        1,
        "tail: one more host revision must start exactly one more turn"
    );
    assert!(
        last_message(&after_tail[tail_starts[0]]).contains(LATER),
        "tail: that turn must run under the later revision: {}",
        digest(tail_starts[0], &after_tail[tail_starts[0]])
    );
}
