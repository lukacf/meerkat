//! `meta/compact_member` on a fresh persistent gateway keeps the identity
//! healthy.
//!
//! The shipped `rpc_gateway --persistent` binary runs over a new state
//! directory with a roster provider and a loopback chat-completions model
//! that reports a provider input cost above the verb's floor on every
//! request. One identity takes three turns; the operator verb then forces one
//! compaction. The identity must stay active on its session: the verb reports
//! the compaction applied, the next turn completes and reaches the model with
//! the compacted transcript, and after a restart the identity resumes that
//! session, still compacted.

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

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const MOB_ID: &str = "compact-member-health";
const MODEL: &str = "compact-member-health-model";
const IDENTITY: &str = "agent:compacted";
const FIRST_TURN_TEXT: &str = "FIRST-TURN describe the release checklist";
const AFTER_VERB_TEXT: &str = "AFTER-VERB what is left to do";
const AFTER_RESTART_TEXT: &str = "AFTER-RESTART summarize the open items";
const COMPACTED_MARKER: &str = "[Context compacted]";

/// The provider input cost the loopback model reports for every request:
/// above the verb's floor, far below the auto-compaction threshold.
const REPORTED_PROMPT_TOKENS: u64 = 5_000;
const COMPACTION_FLOOR_TOKENS: u64 = 256;
const AUTO_COMPACT_THRESHOLD: u64 = 1_000_000;

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const VERB_TIMEOUT: Duration = Duration::from_mins(3);
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
display_name = "Compact member health test model"
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

// ---------------------------------------------------------------------------
// Loopback model
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ModelLog {
    requests: Vec<Value>,
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

/// The messages of one request that carry `marker`.
fn messages_with(request: &Value, marker: &str) -> usize {
    request["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message_text(message).contains(marker))
                .count()
        })
        .unwrap_or_default()
}

async fn chat_completions(
    axum::extract::State(log): axum::extract::State<Arc<Mutex<ModelLog>>>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    log.lock().unwrap().requests.push(request);
    let payload = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"ok\"}}}}]}}\n\n\
         data: {{\"choices\":[{{\"finish_reason\":\"stop\"}}],\
         \"usage\":{{\"prompt_tokens\":{REPORTED_PROMPT_TOKENS},\"completion_tokens\":1}}}}\n\n\
         data: [DONE]\n\n"
    );
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

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

/// The first request after `from` that carries `text`: the one that answered
/// the turn which sent it.
fn request_carrying(log: &Mutex<ModelLog>, from: usize, text: &str) -> Value {
    let log = log.lock().unwrap();
    log.requests[from..]
        .iter()
        .find(|request| messages_with(request, text) > 0)
        .cloned()
        .unwrap_or_else(|| panic!("no model request after #{from} carried {text:?}"))
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
}

impl Gateway {
    fn start(label: &str) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
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
                if sink.len() < 4_000 {
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

    fn status(&mut self) -> Value {
        self.call("meta/status_identity", json!({ "identity": IDENTITY }))
    }

    /// One tracked turn, waited until it completes.
    fn turn(&mut self, text: &str) {
        let sent = self.call(
            "meta/send",
            json!({ "identity": IDENTITY, "content": text, "track_turn": true }),
        );
        let ticket = sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("[{}] untracked turn: {sent}", self.label))
            .to_string();
        let waited = self.request(
            "meta/wait_for_turn",
            json!({ "identity": IDENTITY, "ticket": ticket, "timeout_ms": 90_000 }),
            Duration::from_mins(2),
        );
        assert_eq!(
            waited["result"]["state"],
            json!("completed"),
            "[{}] the turn {text:?} must complete: {waited}\n{}",
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

fn boot(label: &str, state: &Path, host_config: &Path) -> Gateway {
    let mut gateway = Gateway::start(label);
    let response = gateway.request(
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": mob_config(),
            "has_roster_provider": true,
            "runtime_options": {
                "meerkat_config_path": host_config,
                "identity_bootstrap_mode": { "mode": "eager_materialize" },
                "compaction": {
                    "auto_compact_threshold": AUTO_COMPACT_THRESHOLD,
                    "recent_turn_budget": 1
                }
            }
        }),
        INIT_TIMEOUT,
    );
    assert!(
        response.get("error").is_none(),
        "[{label}] init failed: {response}\n{}",
        gateway.stderr_tail()
    );
    gateway
}

/// The identity is active on `session`.
fn assert_active_on(gateway: &mut Gateway, session: &str, when: &str) {
    let status = gateway.status();
    assert_eq!(
        status["state"],
        json!("active"),
        "[{}] {when}: the identity must stay active: {status}\n{}",
        gateway.label,
        gateway.stderr_tail()
    );
    assert_eq!(
        status["session_id"],
        json!(session),
        "[{}] {when}: the identity must keep its session: {status}",
        gateway.label
    );
}

/// The request that answered the turn which sent `text` carries the
/// compaction summary and no longer carries the first turn's prompt.
fn assert_turn_saw_the_compacted_transcript(log: &Mutex<ModelLog>, from: usize, text: &str) {
    let request = request_carrying(log, from, text);
    assert!(
        messages_with(&request, COMPACTED_MARKER) > 0,
        "the turn {text:?} must reach the model with the compaction summary: {request}"
    );
    assert_eq!(
        messages_with(&request, FIRST_TURN_TEXT),
        0,
        "the turn {text:?} must not carry the compacted-away first turn: {request}"
    );
}

#[test]
fn compact_member_on_a_fresh_gateway_keeps_the_identity_healthy() {
    let log = Arc::new(Mutex::new(ModelLog::default()));
    let base_url = start_model(Arc::clone(&log));
    let scratch = tempfile::tempdir().expect("scratch dir");
    let state = scratch.path().join("state");
    let host_config = scratch.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");

    let mut gateway = boot("boot-1", &state, &host_config);
    gateway.turn(FIRST_TURN_TEXT);
    gateway.turn("SECOND-TURN list the blockers");
    gateway.turn("THIRD-TURN name the owners");
    let session = gateway.status()["session_id"]
        .as_str()
        .expect("the identity has a session after three turns")
        .to_string();

    let response = gateway.request(
        "meta/compact_member",
        json!({
            "identity": IDENTITY,
            "floor_tokens": COMPACTION_FLOOR_TOKENS,
            "timeout_ms": 60_000
        }),
        VERB_TIMEOUT,
    );
    assert!(
        response.get("error").is_none(),
        "compact_member must succeed on a fresh gateway: {response}\n{}",
        gateway.stderr_tail()
    );
    let result = &response["result"];
    assert_eq!(result["compaction_applied"], json!(true), "{result}");
    let before = result["messages_before"].as_u64().expect("messages_before");
    let after = result["messages_after"].as_u64().expect("messages_after");
    assert!(
        after < before,
        "the compaction must shrink the transcript: {result}"
    );
    assert_active_on(&mut gateway, &session, "after compact_member");

    let from = log.lock().unwrap().requests.len();
    gateway.turn(AFTER_VERB_TEXT);
    assert_turn_saw_the_compacted_transcript(&log, from, AFTER_VERB_TEXT);
    assert_active_on(&mut gateway, &session, "after the next turn");
    gateway.shutdown_and_reap();

    let mut gateway = boot("boot-2", &state, &host_config);
    assert_active_on(&mut gateway, &session, "after a restart");
    let from = log.lock().unwrap().requests.len();
    gateway.turn(AFTER_RESTART_TEXT);
    assert_turn_saw_the_compacted_transcript(&log, from, AFTER_RESTART_TEXT);
    gateway.shutdown_and_reap();
}
