//! Console sends across real process crashes, from two authenticated
//! principals who use the same idempotency key.
//!
//! Each boot is the shipped `rpc_gateway --persistent` binary over one state
//! directory, with the console served over HTTP under JWT auth, built with
//! the `console-test-faults` feature so a boot can abort at an exact point:
//!
//! - boot A dies after user 1's send is durably accepted and before anything
//!   is submitted (`after_console_reserve`);
//! - boot B dies after user 2's send (same key as user 1's) was admitted
//!   natively and before its frame recorded the delivery
//!   (`after_console_admission`);
//! - boot C serves both users again, with a scheduler wake for the same
//!   identity in between.
//!
//! The model is a loopback chat-completions server run by this test. It
//! records every request, so what reached the model is counted exactly.

#![cfg(feature = "console-test-faults")]
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

const MOB_ID: &str = "console-send-recovery-process";
const MODEL: &str = "console-recovery-test";
const IDENTITY: &str = "agent:specialist";
const SECRET: &str = "console-recovery-signing-key-with-enough-bytes";
const ISSUER: &str = "http://127.0.0.1/meta-gateway";
const AUDIENCE: &str = "persistent-gateway";
const U1_EMAIL: &str = "user-one@example.com";
const U2_EMAIL: &str = "user-two@example.com";
const SHARED_KEY: &str = "shared-key-1";
const U1_TEXT: &str = "U1-ALPHA please check the deploy";
const U2_TEXT: &str = "U2-BETA please check the billing";
const SCHEDULED_TEXT: &str = "SCHEDULED-WAKE run the nightly check";
const U1_PRIVATE_TEXT: &str = "U1-PRIVATE draft the summary";
const U2_GAMMA_TEXT: &str = "U2-GAMMA review the alerts";
const SAME_BODY_TEXT: &str = "SAME-BODY rotate the on-call schedule";
const FINAL_TEXT: &str = "FINAL-CHECK list what you were asked";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SETTLE_TIMEOUT: Duration = Duration::from_mins(2);
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
display_name = "Console recovery test model"
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

fn token(subject: &str, email: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": ISSUER,
            "aud": AUDIENCE,
            "sub": subject,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("sign console token")
}

// ---------------------------------------------------------------------------
// Loopback model
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ModelLog {
    requests: Vec<Value>,
    /// Whether the one request that asks to be held ([`HOLD_ONCE`]) was.
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

fn last_message_text(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|all| all.last())
        .map(message_text)
        .unwrap_or_default()
}

/// The messages of one request that carry `marker`. The model only ever
/// answers "ok", so every carrier is an input the identity received.
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

/// A user message that holds this marker asks the model for one tool call:
/// `CALL-TOOL <tool name> <JSON arguments>`.
const CALL_TOOL: &str = "CALL-TOOL ";

/// The tool call the last message of `request` asks for, if it is a user
/// message that holds [`CALL_TOOL`]: the tool's name and its arguments.
fn scripted_call(request: &Value) -> Option<(String, Value)> {
    let last = request["messages"].as_array()?.last()?;
    if last["role"] != "user" {
        return None;
    }
    let text = message_text(last);
    let rest = &text[text.find(CALL_TOOL)? + CALL_TOOL.len()..];
    let (name, arguments) = rest.split_once(' ')?;
    let arguments = serde_json::Deserializer::from_str(arguments)
        .into_iter::<Value>()
        .next()?
        .ok()?;
    Some((name.to_string(), arguments))
}

/// The first request whose last message holds this marker is not answered
/// while its gateway lives: its run is in flight when the gateway dies.
/// Every later request with the marker is answered at once.
const HOLD_ONCE: &str = "HOLD-ONCE";

async fn chat_completions(
    axum::extract::State(log): axum::extract::State<Arc<Mutex<ModelLog>>>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let call = scripted_call(&request);
    let asks_hold = last_message_text(&request).contains(HOLD_ONCE);
    let (number, hold) = {
        let mut log = log.lock().unwrap();
        let hold = asks_hold && !log.held;
        log.held |= hold;
        log.requests.push(request);
        (log.requests.len(), hold)
    };
    if hold {
        tokio::time::sleep(Duration::from_mins(10)).await;
    }
    let payload = match call {
        Some((name, arguments)) => {
            let delta = json!({ "choices": [{ "delta": { "tool_calls": [{
                "index": 0,
                "id": format!("call_scripted_{number}"),
                "type": "function",
                "function": { "name": name, "arguments": arguments.to_string() },
            }] } }] });
            format!(
                "data: {delta}\n\n\
                 data: {{\"choices\":[{{\"finish_reason\":\"tool_calls\"}}],\
                 \"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n\n\
                 data: [DONE]\n\n"
            )
        }
        None => concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string(),
    };
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

/// Poll `condition` until it holds or `deadline` passes; `false` on timeout.
fn eventually(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if condition() {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
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
    http_base_url: String,
}

impl Gateway {
    fn start(label: &str, fault: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .env_remove("RUST_LOG")
            .env_remove("META_CONSOLE_FAULT")
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
        if let Some(point) = fault {
            command.env("META_CONSOLE_FAULT", point);
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
            http_base_url: String::new(),
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

    /// The identity's session, once its materialization has bound one.
    fn session_id(&mut self) -> String {
        let start = Instant::now();
        loop {
            let status = self.call("meta/status_identity", json!({ "identity": IDENTITY }));
            if let Some(session) = status["session_id"].as_str() {
                return session.to_string();
            }
            assert!(
                start.elapsed() < SETTLE_TIMEOUT,
                "[{}] the identity never bound a session: {status}\n{}",
                self.label,
                self.stderr_tail()
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_mins(1))
            .build()
            .expect("HTTP client")
    }

    /// `POST {base}{path}` as `bearer`: the HTTP status and JSON body, or
    /// `None` when the gateway went away before answering.
    fn http(&self, path: &str, bearer: &str, body: &Value) -> Option<(u16, Value)> {
        let response = Self::client()
            .post(format!("{}{}", self.http_base_url, path))
            .bearer_auth(bearer)
            .json(body)
            .send()
            .ok()?;
        let status = response.status().as_u16();
        Some((status, response.json().unwrap_or(Value::Null)))
    }

    /// One console JSON-RPC call over HTTP as `bearer`: the whole response.
    fn console_rpc(&self, bearer: &str, method: &str, params: Value) -> Option<Value> {
        self.http(
            "/console/rpc",
            bearer,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
        )
        .map(|(_, body)| body)
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn boot(label: &str, state: &Path, host_config: &Path, fault: Option<&str>) -> Gateway {
    boot_as(label, state, host_config, fault, false)
}

/// [`boot`], as a candidate launch when `candidate`
/// (`runtime_options.mob_composition.authority = "candidate"`).
fn boot_as(
    label: &str,
    state: &Path,
    host_config: &Path,
    fault: Option<&str>,
    candidate: bool,
) -> Gateway {
    let mut gateway = Gateway::start(label, fault);
    let mut runtime_options = json!({
        "meerkat_config_path": host_config,
        "identity_bootstrap_mode": { "mode": "eager_materialize" },
        "http_listen": "127.0.0.1:0",
        "auth_config": {
            "provider": "jwt",
            "shared_secret": SECRET,
            "email_allowlist": [U1_EMAIL, U2_EMAIL]
        }
    });
    if candidate {
        runtime_options["mob_composition"] = json!({ "authority": "candidate" });
    }
    let response = gateway.request(
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": mob_config(),
            "has_roster_provider": true,
            "runtime_options": runtime_options
        }),
        INIT_TIMEOUT,
    );
    assert!(
        response.get("error").is_none(),
        "[{label}] init failed: {response}\n{}",
        gateway.stderr_tail()
    );
    gateway.http_base_url = response["result"]["http_base_url"]
        .as_str()
        .expect("HTTP base URL")
        .trim_end_matches('/')
        .to_string();
    gateway
}

fn send_body(text: &str, key: &str) -> Value {
    json!({
        "identity": IDENTITY,
        "content": text,
        "origin": "console",
        "idempotency_key": key,
        "handling_mode": "queue"
    })
}

fn recover_params(key: &str) -> Value {
    json!({ "identity": IDENTITY, "origin": "console", "idempotency_key": key })
}

fn assert_died_at(gateway: &mut Gateway, point: &str) {
    let status = gateway.wait_for_exit(SETTLE_TIMEOUT);
    assert!(
        !status.success(),
        "[{}] must die at the fault point: {status}\n{}",
        gateway.label,
        gateway.stderr_tail()
    );
    let tail = gateway.stderr_tail();
    assert!(
        tail.contains(&format!("console fault point {point}")),
        "[{}] died somewhere else\n{tail}",
        gateway.label
    );
}

/// The statuses of the console sends the Console store under `state`
/// holds, in order, read once the gateway exited.
fn send_frame_statuses(state: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open_with_flags(
        state.join("meta_console.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the Console store read-only");
    let mut statement = connection
        .prepare(
            "SELECT status FROM console_frames \
             WHERE kind = 'user_input' AND source_kind = 'send' ORDER BY cursor_seq",
        )
        .expect("prepare the send query");
    statement
        .query_map([], |row| row.get(0))
        .expect("query the sends")
        .collect::<Result<Vec<String>, _>>()
        .expect("read the sends")
}

/// A candidate launch settles no console send at startup. User 2's send is
/// admitted natively, and the process dies before its frame records the
/// delivery. A candidate launch over the same state leaves the send
/// unsettled (its startup census counts it) and refuses the recovery that
/// would settle it. The authoritative launch that follows settles it
/// delivered at its startup.
#[test]
fn a_candidate_startup_leaves_an_interrupted_console_send_for_the_authoritative_launch() {
    let state = tempfile::tempdir().expect("state dir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let log = Arc::new(Mutex::new(ModelLog::default()));
    let base_url = start_model(Arc::clone(&log));
    let host_config = config_dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");
    let u2 = token("user-two-subject", U2_EMAIL);

    // Boot A: user 2's send is admitted natively, then the process dies
    // before its frame records the delivery.
    let mut boot_a = boot(
        "interrupted-a",
        state.path(),
        &host_config,
        Some("after_console_admission"),
    );
    let _ = boot_a.console_rpc(&u2, "meta/console/send", send_body(U2_TEXT, SHARED_KEY));
    assert_died_at(&mut boot_a, "after_console_admission");
    drop(boot_a);
    let unsettled = send_frame_statuses(state.path());
    assert!(
        unsettled.len() == 1 && unsettled[0] != "delivered",
        "the send stays unsettled: {unsettled:?}"
    );

    // Boot B, a candidate: its startup settles nothing, and its Console
    // refuses the recovery.
    let mut candidate = boot_as("interrupted-b", state.path(), &host_config, None, true);
    let census = eventually(SETTLE_TIMEOUT, || {
        candidate.stderr.lock().is_ok_and(|lines| {
            lines.iter().any(|line| {
                line.contains("candidate launch census") && line.contains("unsettled=1")
            })
        })
    });
    assert!(
        census,
        "the candidate's startup census counts the send\n{}",
        candidate.stderr_tail()
    );
    let refused = candidate
        .http("/console/send/recover", &u2, &recover_params(SHARED_KEY))
        .expect("the candidate answers");
    assert_eq!(
        (refused.0, refused.1["error"].clone()),
        (409, json!("candidate_launch")),
        "{refused:?}"
    );
    candidate.shutdown_and_reap();
    drop(candidate);
    assert_eq!(
        send_frame_statuses(state.path()),
        unsettled,
        "the candidate leaves the send unsettled"
    );

    // Boot C, authoritative: its startup settles the send delivered.
    let mut boot_c = boot("interrupted-c", state.path(), &host_config, None);
    let (status, recovered) = boot_c
        .http("/console/send/recover", &u2, &recover_params(SHARED_KEY))
        .expect("the recovery answers");
    assert_eq!(
        (
            status,
            recovered["status"].clone(),
            recovered["native"]["state"].clone()
        ),
        (200, json!("delivered"), json!("admitted")),
        "{recovered}"
    );
    boot_c.shutdown_and_reap();
}

#[test]
fn two_principals_one_key_and_two_crashes_deliver_each_send_exactly_once() {
    let state = tempfile::tempdir().expect("state dir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let log = Arc::new(Mutex::new(ModelLog::default()));
    let base_url = start_model(Arc::clone(&log));
    let host_config = config_dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");
    let u1 = token("user-one-subject", U1_EMAIL);
    let u2 = token("user-two-subject", U2_EMAIL);

    // Boot A: user 1's send is accepted, then the process dies before
    // anything is submitted.
    let mut boot_a = boot(
        "boot-a",
        state.path(),
        &host_config,
        Some("after_console_reserve"),
    );
    let session = boot_a.session_id();
    let lost = boot_a.http("/console/send", &u1, &send_body(U1_TEXT, SHARED_KEY));
    assert!(
        lost.is_none(),
        "boot A answered a send it should have died in: {lost:?}"
    );
    assert_died_at(&mut boot_a, "after_console_reserve");
    drop(boot_a);

    // Boot B: user 2 sends under the SAME key. It is a different send; it is
    // admitted natively, then the process dies before recording delivery.
    let mut boot_b = boot(
        "boot-b",
        state.path(),
        &host_config,
        Some("after_console_admission"),
    );
    assert_eq!(boot_b.session_id(), session, "one session lineage");
    let u2_first = boot_b.console_rpc(&u2, "meta/console/send", send_body(U2_TEXT, SHARED_KEY));
    if let Some(answer) = &u2_first {
        assert!(
            answer.get("error").is_none_or(Value::is_null),
            "user 2's send under the key user 1 used must be its own send: {answer}"
        );
    }
    assert_died_at(&mut boot_b, "after_console_admission");
    drop(boot_b);

    // Boot C: no fault.
    let mut boot_c = boot("boot-c", state.path(), &host_config, None);
    assert_eq!(boot_c.session_id(), session, "one session lineage");

    // A scheduler wake for the same identity, under the same key string.
    boot_c.call(
        "meta/dispatch",
        json!({
            "identity": IDENTITY,
            "dispatch_input": {
                "content": SCHEDULED_TEXT,
                "origin": "scheduler",
                "idempotency_key": SHARED_KEY,
                "correlation_id": "3b241101-e2bb-4255-8caf-4136c566a962"
            }
        }),
    );

    // User 2's admitted send settled at boot from native evidence (REST).
    let (status, u2_recovered) = boot_c
        .http("/console/send/recover", &u2, &recover_params(SHARED_KEY))
        .expect("u2 recovery answered");
    let u2_settled = status == 200
        && u2_recovered["status"] == "delivered"
        && u2_recovered["native"]["state"] == "admitted";

    // User 1's resend of the interrupted send dispatches it once.
    let u1_before = boot_c
        .console_rpc(&u1, "meta/console/recover_send", recover_params(SHARED_KEY))
        .expect("u1 recovery answered");
    let u1_absent_before = u1_before["result"]["native"]["state"] == "absent"
        && u1_before["result"]["status"] == "accepted";
    let (status, u1_resent) = boot_c
        .http("/console/send", &u1, &send_body(U1_TEXT, SHARED_KEY))
        .expect("u1 resend answered");
    assert_eq!(status, 200, "u1 resend: {u1_resent}");
    let u1_delivered = eventually(SETTLE_TIMEOUT, || {
        boot_c
            .console_rpc(&u1, "meta/console/recover_send", recover_params(SHARED_KEY))
            .is_some_and(|response| response["result"]["status"] == "delivered")
    });
    let u1_recovered = boot_c
        .console_rpc(&u1, "meta/console/recover_send", recover_params(SHARED_KEY))
        .expect("u1 recovery answered");

    // The two principals' sends under one key are two sends.
    assert_ne!(
        u1_recovered["result"]["input_frame_id"], u2_recovered["input_frame_id"],
        "{u1_recovered} {u2_recovered}"
    );
    assert_ne!(
        u1_recovered["result"]["interaction_id"], u2_recovered["interaction_id"],
        "{u1_recovered} {u2_recovered}"
    );
    assert_eq!(
        u1_resent["input_frame_id"], u1_recovered["result"]["input_frame_id"],
        "the resend replays user 1's own send"
    );
    if let Some(first) = &u2_first
        && first.get("result").is_some()
    {
        assert_eq!(
            first["result"]["input_frame_id"],
            u2_recovered["input_frame_id"]
        );
    }

    // User 2's resend replays its own settled send: nothing is dispatched.
    let u2_resend = boot_c
        .console_rpc(&u2, "meta/console/send", send_body(U2_TEXT, SHARED_KEY))
        .expect("u2 resend answered");
    assert_eq!(
        u2_resend["result"]["input_frame_id"], u2_recovered["input_frame_id"],
        "{u2_resend}"
    );

    // Same-principal replay and distinct-principal isolation on a key only
    // user 1 used.
    let private = boot_c
        .console_rpc(
            &u1,
            "meta/console/send",
            send_body(U1_PRIVATE_TEXT, "u1-only"),
        )
        .expect("u1 private send");
    let private_again = boot_c
        .console_rpc(
            &u1,
            "meta/console/send",
            send_body(U1_PRIVATE_TEXT, "u1-only"),
        )
        .expect("u1 private replay");
    assert_eq!(
        private["result"]["input_frame_id"], private_again["result"]["input_frame_id"],
        "{private} {private_again}"
    );
    assert_eq!(
        private["result"]["interaction_id"],
        private_again["result"]["interaction_id"]
    );

    // recover_send authorization: user 2 asking for user 1's key gets the
    // same not_found as a key nobody used, over RPC and over REST.
    let foreign_rpc = boot_c
        .console_rpc(&u2, "meta/console/recover_send", recover_params("u1-only"))
        .expect("foreign recovery answered");
    let missing_rpc = boot_c
        .console_rpc(
            &u2,
            "meta/console/recover_send",
            recover_params("never-used"),
        )
        .expect("missing recovery answered");
    assert_eq!(foreign_rpc["error"], missing_rpc["error"]);
    assert_eq!(foreign_rpc["error"]["data"]["kind"], "send_not_found");
    assert!(foreign_rpc.get("result").is_none_or(Value::is_null));
    let foreign_rest = boot_c
        .http("/console/send/recover", &u2, &recover_params("u1-only"))
        .expect("foreign REST recovery answered");
    let missing_rest = boot_c
        .http("/console/send/recover", &u2, &recover_params("never-used"))
        .expect("missing REST recovery answered");
    assert_eq!(foreign_rest, missing_rest);
    assert_eq!(foreign_rest.0, 404);
    assert_eq!(foreign_rest.1["error"], "send_not_found");
    // Neither the returned ids nor the key reach another principal's send.
    let by_ids = boot_c
        .console_rpc(
            &u2,
            "meta/console/recover_send",
            json!({
                "identity": IDENTITY,
                "origin": "console",
                "idempotency_key": "u1-only",
                "input_frame_id": private["result"]["input_frame_id"],
                "interaction_id": private["result"]["interaction_id"],
            }),
        )
        .expect("recovery by ids answered");
    assert_eq!(by_ids["error"]["code"], -32602, "{by_ids}");

    // User 2 under user 1's key, with its own body: its own new send.
    let gamma = boot_c
        .console_rpc(
            &u2,
            "meta/console/send",
            send_body(U2_GAMMA_TEXT, "u1-only"),
        )
        .expect("u2 send under u1's key");
    assert!(gamma.get("error").is_none_or(Value::is_null), "{gamma}");
    assert_ne!(
        gamma["result"]["input_frame_id"],
        private["result"]["input_frame_id"]
    );

    // The same body under the same key from both principals: two sends, so
    // neither receives the other's acceptance.
    let mine = boot_c
        .console_rpc(
            &u1,
            "meta/console/send",
            send_body(SAME_BODY_TEXT, "same-body"),
        )
        .expect("u1 same-body send");
    let theirs = boot_c
        .console_rpc(
            &u2,
            "meta/console/send",
            send_body(SAME_BODY_TEXT, "same-body"),
        )
        .expect("u2 same-body send");
    assert!(mine.get("error").is_none_or(Value::is_null), "{mine}");
    assert!(theirs.get("error").is_none_or(Value::is_null), "{theirs}");
    assert_ne!(
        mine["result"]["input_frame_id"], theirs["result"]["input_frame_id"],
        "{mine} {theirs}"
    );
    assert_ne!(
        mine["result"]["interaction_id"],
        theirs["result"]["interaction_id"]
    );

    // One more turn shows the whole transcript the identity holds.
    boot_c
        .console_rpc(&u1, "meta/console/send", send_body(FINAL_TEXT, "final"))
        .expect("final send");
    let final_seen = eventually(SETTLE_TIMEOUT, || {
        log.lock()
            .unwrap()
            .requests
            .iter()
            .any(|request| last_message_text(request).contains("FINAL-CHECK"))
    });
    assert!(final_seen, "the final turn never reached the model");
    let transcript = log
        .lock()
        .unwrap()
        .requests
        .iter()
        .rev()
        .find(|request| last_message_text(request).contains("FINAL-CHECK"))
        .cloned()
        .expect("final request");
    let count = |marker: &str| messages_with(&transcript, marker);
    let delivered_once = [
        ("U1-ALPHA", count("U1-ALPHA")),
        ("U2-BETA", count("U2-BETA")),
        ("SCHEDULED-WAKE", count("SCHEDULED-WAKE")),
        ("U1-PRIVATE", count("U1-PRIVATE")),
        ("U2-GAMMA", count("U2-GAMMA")),
    ];
    let same_body = count("SAME-BODY");
    let model_requests = log.lock().unwrap().requests.len();
    let session_after = boot_c.session_id();

    let evidence = format!(
        "u2_settled_at_boot={u2_settled} u1_absent_before_resend={u1_absent_before} \
         u1_delivered_after_resend={u1_delivered} transcript={delivered_once:?} \
         same_body_sends={same_body} model_requests={model_requests} session_unchanged={}",
        session_after == session
    );
    eprintln!("console send recovery evidence: {evidence}");
    assert!(
        u2_settled,
        "user 2's admitted send settles delivered: {evidence}"
    );
    assert!(
        u1_absent_before,
        "user 1's interrupted send was never admitted: {evidence}"
    );
    assert!(u1_delivered, "user 1's resend delivers it: {evidence}");
    assert!(
        delivered_once.iter().all(|(_, count)| *count == 1),
        "every send reaches the identity exactly once: {evidence}"
    );
    assert_eq!(
        same_body, 2,
        "one delivery per principal's send: {evidence}"
    );
    assert_eq!(session_after, session, "one session lineage: {evidence}");

    boot_c.shutdown_and_reap();
}

// ---------------------------------------------------------------------------
// A side chat that is built while its mob starts
// ---------------------------------------------------------------------------

const SIDE: &str = "side-kept";
const SIDE_NOTE: &str = "SIDE-START-NOTE";

fn mob_config_with_agent_memory() -> String {
    format!("{}memory = true\n", mob_config())
}

/// [`boot`] with agent memory on and no agent loaded at the start.
fn boot_with_agent_memory(
    label: &str,
    state: &Path,
    host_config: &Path,
    fault: Option<&str>,
    personal_accounts: bool,
) -> Gateway {
    let mut gateway = Gateway::start(label, fault);
    let mut auth_config = json!({
        "provider": "jwt",
        "shared_secret": SECRET,
        "email_allowlist": [U1_EMAIL, U2_EMAIL]
    });
    if personal_accounts {
        // Each token names one person: a side chat that person forks on
        // the Console is private to them.
        auth_config["accounts"] = json!("personal");
    }
    let response = gateway.request(
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": mob_config_with_agent_memory(),
            "has_roster_provider": true,
            "runtime_options": {
                "meerkat_config_path": host_config,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" },
                "agent_memory": true,
                "http_listen": "127.0.0.1:0",
                "auth_config": auth_config
            }
        }),
        INIT_TIMEOUT,
    );
    assert!(
        response.get("error").is_none(),
        "[{label}] init failed: {response}\n{}",
        gateway.stderr_tail()
    );
    gateway.http_base_url = response["result"]["http_base_url"]
        .as_str()
        .expect("HTTP base URL")
        .trim_end_matches('/')
        .to_string();
    gateway
}

fn send_to(member: &str, text: &str, key: &str) -> Value {
    json!({
        "identity": member,
        "content": text,
        "origin": "console",
        "idempotency_key": key,
        "handling_mode": "queue"
    })
}

/// Whether the model was sent a request that carries `marker`.
fn model_saw(log: &Arc<Mutex<ModelLog>>, marker: &str) -> bool {
    log.lock()
        .unwrap()
        .requests
        .iter()
        .any(|request| messages_with(request, marker) > 0)
}

/// One Console send of `text` to `member` as `bearer`, until the model was
/// sent it and answered: `marker` names the send in the model's log.
fn console_turn(
    gateway: &mut Gateway,
    log: &Arc<Mutex<ModelLog>>,
    bearer: &str,
    member: &str,
    text: &str,
    marker: &str,
) {
    let sent = gateway
        .console_rpc(bearer, "meta/console/send", send_to(member, text, marker))
        .unwrap_or_else(|| panic!("[{}] the send {marker} answered", gateway.label));
    assert!(
        sent.get("error").is_none_or(Value::is_null),
        "[{}] the send {marker} is accepted: {sent}\n{}",
        gateway.label,
        gateway.stderr_tail()
    );
    let delivered = eventually(SETTLE_TIMEOUT, || {
        gateway.pump();
        model_saw(log, marker)
            && gateway
                .console_rpc(
                    bearer,
                    "meta/console/recover_send",
                    json!({ "identity": member, "origin": "console", "idempotency_key": marker }),
                )
                .is_some_and(|response| response["result"]["status"] == "delivered")
    });
    assert!(
        delivered,
        "[{}] the send {marker} reaches the model and is delivered\n{}",
        gateway.label,
        gateway.stderr_tail()
    );
}

/// The scope keys of the agent memory records titled `title` that the
/// store under `state` holds, read once the gateway exited.
fn note_scope_keys(state: &Path, title: &str) -> Vec<String> {
    let connection = rusqlite::Connection::open_with_flags(
        state.join("agent-memory").join("default.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the agent memory store read-only");
    let mut statement = connection
        .prepare("SELECT scope_key FROM records WHERE title = ?1 ORDER BY created_at_ms")
        .expect("prepare the note query");
    statement
        .query_map([title], |row| row.get(0))
        .expect("query the notes")
        .collect::<Result<Vec<String>, _>>()
        .expect("read the notes")
}

/// A side chat that owes work when its gateway starts is built while the
/// mob starts, before its source agent. Once the agent has run, a note the
/// side chat stores with its `memory` tool is kept under the side chat's
/// own key.
///
/// - boot A: the agent runs, and a side chat of it is forked on the
///   Console;
/// - boot B dies after a send to the side chat was admitted natively, so
///   the side chat owes that work at the next start;
/// - boot C starts with the side chat's work owed; the agent runs after it;
///   then the side chat is asked to store a note.
#[test]
fn a_side_chat_built_while_its_mob_starts_keeps_its_notes_under_its_own_key() {
    side_chat_built_while_its_mob_starts(false, false);
}

/// The same, with the side chat's source agent owing work at that start
/// too (a run of the agent is in flight when boot B dies): both are built
/// while the mob starts, the agent before its side chat.
#[test]
fn a_side_chat_built_with_its_source_while_their_mob_starts_keeps_its_notes_under_its_own_key() {
    side_chat_built_while_its_mob_starts(true, false);
}

/// The same again, on a Console whose sign-in names one person per token:
/// the side chat is private to the person who forked it, and its note is
/// kept under that person and the side chat's name.
#[test]
fn a_side_chat_built_with_its_source_while_their_mob_starts_keeps_a_private_note_under_its_owner() {
    side_chat_built_while_its_mob_starts(true, true);
}

fn side_chat_built_while_its_mob_starts(agent_owes_work: bool, private: bool) {
    let state = tempfile::tempdir().expect("state dir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let log = Arc::new(Mutex::new(ModelLog::default()));
    let base_url = start_model(Arc::clone(&log));
    let host_config = config_dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");
    let u1 = token("user-one-subject", U1_EMAIL);

    // Boot A: the agent runs; a side chat of it is forked on the Console.
    let mut boot_a = boot_with_agent_memory("start-a", state.path(), &host_config, None, private);
    console_turn(
        &mut boot_a,
        &log,
        &u1,
        IDENTITY,
        "AGENT-FIRST say hello",
        "AGENT-FIRST",
    );
    let mut forked = None;
    for _ in 0..100 {
        forked = boot_a.console_rpc(
            &u1,
            "meta/fork_member",
            json!({ "source_member_id": IDENTITY, "agent_identity": SIDE }),
        );
        let unavailable = forked.as_ref().is_some_and(|answer| {
            answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
        });
        if !unavailable {
            break;
        }
        boot_a.pump();
        thread::sleep(Duration::from_millis(100));
    }
    let forked = forked.expect("the fork answered");
    assert!(
        forked.get("error").is_none_or(Value::is_null),
        "the side chat is forked on the Console: {forked}\n{}",
        boot_a.stderr_tail()
    );
    boot_a.shutdown_and_reap();
    drop(boot_a);

    // Boot B: a send to the side chat is admitted natively, then the
    // process dies: the side chat owes that work.
    let mut boot_b = boot_with_agent_memory(
        "start-b",
        state.path(),
        &host_config,
        Some("after_console_admission"),
        private,
    );
    if agent_owes_work {
        // A run of the agent that the model does not answer while this
        // gateway lives: it is in flight when the gateway dies.
        boot_b.call(
            "meta/send",
            json!({ "identity": IDENTITY, "content": format!("AGENT-HELD {HOLD_ONCE} wait") }),
        );
        let held = eventually(SETTLE_TIMEOUT, || {
            boot_b.pump();
            model_saw(&log, "AGENT-HELD")
        });
        assert!(
            held,
            "the agent's run reaches the model before the gateway dies\n{}",
            boot_b.stderr_tail()
        );
    }
    let owed = boot_b.console_rpc(
        &u1,
        "meta/console/send",
        send_to(SIDE, "SIDE-OWED carry on", "SIDE-OWED"),
    );
    if let Some(answer) = &owed {
        assert!(
            answer.get("error").is_none_or(Value::is_null),
            "the send to the side chat is its own send: {answer}"
        );
    }
    assert_died_at(&mut boot_b, "after_console_admission");
    drop(boot_b);

    // Boot C: the side chat's work is owed at the start. Then the agent
    // runs, then the side chat is asked to store a note.
    let mut boot_c = boot_with_agent_memory("start-c", state.path(), &host_config, None, private);
    let owed_ran = eventually(SETTLE_TIMEOUT, || {
        boot_c.pump();
        model_saw(&log, "SIDE-OWED")
    });
    assert!(
        owed_ran,
        "the side chat runs the work it owed at the start\n{}",
        boot_c.stderr_tail()
    );
    console_turn(
        &mut boot_c,
        &log,
        &u1,
        IDENTITY,
        "AGENT-SECOND say hello again",
        "AGENT-SECOND",
    );
    console_turn(
        &mut boot_c,
        &log,
        &u1,
        SIDE,
        "SIDE-AFTER-AGENT carry on",
        "SIDE-AFTER-AGENT",
    );
    let remember = json!({
        "action": "remember",
        "kind": "fact",
        "title": SIDE_NOTE,
        "description": "a note made in a side chat",
        "body": "The side chat stored this note after its mob's start.",
        "tags": [],
    });
    console_turn(
        &mut boot_c,
        &log,
        &u1,
        SIDE,
        &format!("SIDE-REMEMBER {CALL_TOOL}memory {remember}"),
        "SIDE-REMEMBER",
    );
    // The turn that called the tool ends with the request that carries the
    // tool's answer.
    let answered = eventually(SETTLE_TIMEOUT, || {
        boot_c.pump();
        log.lock().unwrap().requests.iter().any(|request| {
            messages_with(request, "SIDE-REMEMBER") > 0
                && request["messages"]
                    .as_array()
                    .and_then(|messages| messages.last())
                    .is_some_and(|last| last["role"] == "tool")
        })
    });
    assert!(
        answered,
        "the side chat's `memory` call is answered\n{}",
        boot_c.stderr_tail()
    );
    // The tools the model was offered in the newest request that carries
    // each step's marker, in the order of the steps.
    let offered_at = |marker: &str| -> Vec<String> {
        log.lock()
            .unwrap()
            .requests
            .iter()
            .rev()
            .find(|request| messages_with(request, marker) > 0)
            .and_then(|request| request["tools"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect()
    };
    let steps: Vec<(&str, Vec<String>)> = [
        "SIDE-OWED",
        "AGENT-SECOND",
        "SIDE-AFTER-AGENT",
        "SIDE-REMEMBER",
    ]
    .into_iter()
    .map(|marker| (marker, offered_at(marker)))
    .collect();
    let offered = offered_at("SIDE-REMEMBER");
    boot_c.shutdown_and_reap();

    let keys = note_scope_keys(state.path(), SIDE_NOTE);
    eprintln!(
        "side chat start evidence: agent_owes_work={agent_owes_work} private={private} \
         note_scope_keys={keys:?} offered_by_step={steps:?}"
    );
    assert!(
        keys.len() == 1 && keys[0] != IDENTITY && keys[0].contains(SIDE),
        "the side chat's note is kept under the side chat's own key: scope keys {keys:?}, \
         offered {offered:?}"
    );
    if private {
        // A private member's key: `private/<its owner>/<its name>`.
        assert!(
            keys[0].starts_with("private/") && keys[0].ends_with(&format!("/{SIDE}")),
            "a private side chat's note is kept under its owner and its name: {keys:?}"
        );
    } else {
        assert_eq!(
            keys[0], SIDE,
            "a shared side chat's note is kept under its name"
        );
    }
}
