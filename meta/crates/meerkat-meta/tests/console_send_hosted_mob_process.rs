//! A Console send to a member of a SELECTED hosted mob, and its recovery,
//! over the real gateway process and its SQLite stores.
//!
//! One `rpc_gateway --persistent` process is launched with its launch mob
//! and a roster of two members, `desk` and `control`. Its stdio host creates
//! a second mob beside it and, in that mob, an agent that is also named
//! `desk`. The Console is served over HTTP under JWT sign-in; the sender is
//! a signed-in user of the allowlist, and the host enforces no access rules
//! (the default), so the sender may address every mob the host serves. The
//! model is a loopback chat-completions server run by this test, which
//! records every request: no provider is called.
//!
//! The row, through the Console's own request door (`/console/rpc`, the
//! methods `meta/console/send` and `meta/console/recover_send`, the mob
//! selected by `mob_id` as every per-mob Console method takes it):
//!
//! 1. a send to `desk` of the selected hosted mob is accepted, reaches the
//!    model once, and is kept in the Console store under that mob;
//! 2. the recovery by the same idempotency key, for the same mob, reads that
//!    send (the same input frame);
//! 3. `desk` of the launch mob holds no send, and the key finds none there;
//! 4. the same reads after the gateway is stopped and started again over the
//!    same state directory.
//!
//! Before the row asks the send, two readings in the same process show that
//! a refusal there is the door's and not the rig's: the same sender reads
//! the hosted mob's agents through a Console method that is served per mob,
//! and a Console send and recovery for the launch mob's `control` member
//! deliver (the launch mob's `desk` is left untouched: it is the bystander).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const LAUNCH_MOB: &str = "console-send-hosted-launch";
const SECOND_MOB: &str = "console-send-hosted-second";
const MODEL: &str = "console-send-hosted-test";
/// The member both mobs have under one name.
const DESK: &str = "desk";
/// A second member of the launch mob, for the rig's own control send.
const CONTROL: &str = "control";
const SECRET: &str = "console-send-hosted-signing-key-with-enough-bytes";
const ISSUER: &str = "http://127.0.0.1/meta-gateway";
const AUDIENCE: &str = "persistent-gateway";
const SENDER_EMAIL: &str = "sender@example.com";
const HOSTED_KEY: &str = "hosted-send-key-1";
const HOSTED_TEXT: &str = "HOSTED-SEND please check the second mob's desk";
const CONTROL_KEY: &str = "control-send-key-1";
const CONTROL_TEXT: &str = "CONTROL-SEND please check the launch mob's control member";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SETTLE_TIMEOUT: Duration = Duration::from_mins(2);
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
display_name = "Console send hosted mob test model"
family = "{MODEL}"
"#
    );
    config.push_str(&realm_binding("global"));
    config.push_str(&realm_binding(&format!("mob.{LAUNCH_MOB}")));
    config.push_str(&realm_binding(&format!("mob.{SECOND_MOB}")));
    config
}

fn roster_spec(identity: &str) -> Value {
    json!({
        "identity": identity,
        "profile": "specialist",
        "addressability": "addressable",
        "display_name": null,
        "labels": {},
        "context": null,
        "additional_instructions": []
    })
}

/// The identity policy of a launch whose stdio host may create agents: the
/// one rule allows the host's creates, and every other change is denied.
fn the_host_creates_agents() -> Value {
    json!({
        "enabled": true,
        "approval": { "rules": [{
            "change": ["create"],
            "requester": { "kind": "host", "surface": "rpc" },
            "outcome": { "kind": "allow" },
        }] },
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

/// Whether a model request carries `marker` in one of its messages.
fn carries(request: &Value, marker: &str) -> bool {
    request["messages"].as_array().is_some_and(|messages| {
        messages
            .iter()
            .any(|message| message_text(message).contains(marker))
    })
}

/// How many of the recorded model requests carry `marker` in a message.
fn requests_with(log: &Arc<Mutex<Vec<Value>>>, marker: &str) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|request| carries(request, marker))
        .count()
}

/// A request that carries this marker is not answered while its model's
/// hold is on ([`ModelHold`]): its run stays in flight until the test
/// releases it.
const HOLD_MARKER: &str = "HOLD-UNTIL-RELEASED";

/// The hold of one test's own model. Each test starts a model of its own
/// and shares this with it, so a hold never reaches another test's model,
/// whether the tests run in one process or in several.
#[derive(Default)]
struct ModelHold {
    /// Set by the test that wants its model to hold the answers to the
    /// requests that carry [`HOLD_MARKER`].
    on: std::sync::atomic::AtomicBool,
    /// How many requests entered the hold.
    held: std::sync::atomic::AtomicUsize,
}

/// What a test's model serves from: the log of every request it received,
/// and its hold.
#[derive(Clone)]
struct Model {
    log: Arc<Mutex<Vec<Value>>>,
    hold: Arc<ModelHold>,
}

/// A request whose last message is a user message with this marker is
/// answered with one call of [`PEERS_TOOL`] and no text. The request that
/// follows it in the run ends with the tool's answer, or with a later user
/// message, and is answered with a final text.
const CALL_PEERS: &str = "CALL-THE-PEERS-TOOL";
/// The comms tool that lists a member's peers: read-only, no arguments. A
/// member of the rig's profile has it (`[profiles.specialist.tools] comms`).
const PEERS_TOOL: &str = "peers";

async fn chat_completions(
    axum::extract::State(Model { log, hold }): axum::extract::State<Model>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    use std::sync::atomic::Ordering;
    let asks_hold = carries(&request, HOLD_MARKER);
    let asks_call = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .is_some_and(|last| last["role"] == "user" && message_text(last).contains(CALL_PEERS));
    let number = {
        let mut log = log.lock().unwrap();
        log.push(request);
        log.len()
    };
    if asks_hold && hold.on.load(Ordering::SeqCst) {
        hold.held.fetch_add(1, Ordering::SeqCst);
        while hold.on.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let payload = if asks_call {
        let delta = json!({ "choices": [{ "delta": { "tool_calls": [{
            "index": 0,
            "id": format!("call_peers_{number}"),
            "type": "function",
            "function": { "name": PEERS_TOOL, "arguments": "{}" },
        }] } }] });
        format!(
            "data: {delta}\n\n\
             data: {{\"choices\":[{{\"finish_reason\":\"tool_calls\"}}],\
             \"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n\n\
             data: [DONE]\n\n"
        )
    } else {
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string()
    };
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

/// Start a model that never holds an answer; returns its base URL.
fn start_model(log: Arc<Mutex<Vec<Value>>>) -> String {
    start_model_with_hold(log, Arc::default())
}

/// Start a model whose hold the test keeps; returns its base URL.
fn start_model_with_hold(log: Arc<Mutex<Vec<Value>>>, hold: Arc<ModelHold>) -> String {
    let model = Model { log, hold };
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
                .with_state(model);
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
    /// One gateway process. Its session identities go below `xdg_state`, a
    /// directory of this test, and no provider key reaches it.
    fn start(label: &str, xdg_state: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .env_remove("RUST_LOG")
            .env("XDG_STATE_HOME", xdg_state)
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
            "RKAT_GOOGLE_API_KEY",
            "GOOGLE_API_KEY",
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

    /// One request of the stdio host: the whole response.
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

    /// One request of the stdio host that must succeed: its result.
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

    /// The launch's roster callback is answered with the launch mob's two
    /// members; a gateway asks its launch's roster for its launch mob alone.
    fn answer_callback(&mut self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        match method {
            "callback/roster_provider/roster" => {
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": [roster_spec(DESK), roster_spec(CONTROL)],
                }));
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

    fn shutdown_and_reap(&mut self) {
        let response = self.request("meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        gateway_shutdown::assert_persistent_shutdown_completed(
            &response,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        drop(self.stdin.take());
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                break status;
            }
            if start.elapsed() >= EXIT_TIMEOUT {
                let tail = self.stderr_tail();
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "[{}] gateway did not exit within {EXIT_TIMEOUT:?}\n{tail}",
                    self.label
                );
            }
            self.pump();
            thread::sleep(Duration::from_millis(25));
        };
        assert!(
            status.success(),
            "[{}] gateway exited with {status}\n{}",
            self.label,
            self.stderr_tail()
        );
    }

    /// One Console JSON-RPC request over HTTP as `bearer`, through the
    /// Console's own request door: the whole response body.
    fn console_rpc(&self, bearer: &str, method: &str, params: Value) -> Value {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_mins(1))
            .build()
            .expect("HTTP client")
            .post(format!("{}/console/rpc", self.http_base_url))
            .bearer_auth(bearer)
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
            .send()
            .unwrap_or_else(|error| {
                panic!(
                    "[{}] the Console did not answer {method}: {error}\n{}",
                    self.label,
                    self.stderr_tail()
                )
            });
        response.json().unwrap_or(Value::Null)
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Launch the gateway over `state`: the launch mob with its roster, the
/// Console over HTTP under JWT sign-in, and an identity policy under which
/// the stdio host creates agents.
fn boot(label: &str, state: &Path, host_config: &Path, xdg_state: &Path) -> Gateway {
    let mut gateway = Gateway::start(label, xdg_state);
    let response = gateway.request(
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": mob_config(LAUNCH_MOB),
            "has_roster_provider": true,
            "runtime_options": {
                "meerkat_config_path": host_config,
                "identity_bootstrap_mode": { "mode": "eager_materialize" },
                "identity_creation": the_host_creates_agents(),
                "http_listen": "127.0.0.1:0",
                "auth_config": {
                    "provider": "jwt",
                    "shared_secret": SECRET,
                    "email_allowlist": [SENDER_EMAIL]
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
    gateway.http_base_url = response["result"]["http_base_url"]
        .as_str()
        .expect("HTTP base URL")
        .trim_end_matches('/')
        .to_string();
    gateway
}

/// `params` with the mob selected as a per-mob Console method takes it.
fn in_mob(mob: &str, mut params: Value) -> Value {
    params["mob_id"] = json!(mob);
    params
}

fn send_params(identity: &str, text: &str, key: &str) -> Value {
    json!({
        "identity": identity,
        "content": text,
        "origin": "console",
        "idempotency_key": key,
        "handling_mode": "queue"
    })
}

fn recover_params(identity: &str, key: &str) -> Value {
    json!({ "identity": identity, "origin": "console", "idempotency_key": key })
}

/// Whether a Console answer carries no error.
fn answered(response: &Value) -> bool {
    response.get("error").is_none_or(Value::is_null)
}

/// The Console sends the store under `state` holds, in order: the mob each
/// is kept under (empty for the launch mob), its identity and its status.
/// Read with the gateway stopped.
fn stored_sends(state: &Path) -> Vec<(String, String, String)> {
    let connection = rusqlite::Connection::open_with_flags(
        state.join("meta_console.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the Console store read-only");
    let mut statement = connection
        .prepare(
            "SELECT mob_id, identity, status FROM console_frames \
             WHERE kind = 'user_input' AND source_kind = 'send' ORDER BY cursor_seq",
        )
        .expect("prepare the send query");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query the sends")
        .collect::<Result<Vec<(String, String, String)>, _>>()
        .expect("read the sends")
}

/// What the row reads of the hosted send and of the bystander in one
/// process life: the recovery for the selected mob, and the recovery of the
/// same key for the launch mob's member of the same name.
fn read_hosted_send_and_bystander(gateway: &Gateway, bearer: &str) -> (Value, Value) {
    let hosted = gateway.console_rpc(
        bearer,
        "meta/console/recover_send",
        in_mob(SECOND_MOB, recover_params(DESK, HOSTED_KEY)),
    );
    let bystander = gateway.console_rpc(
        bearer,
        "meta/console/recover_send",
        recover_params(DESK, HOSTED_KEY),
    );
    (hosted, bystander)
}

/// Record that an expectation of a step does not hold. The row goes on, so
/// that every step runs and prints its answers, and fails at its end with
/// every expectation that did not hold.
fn expect(failures: &mut Vec<String>, holds: bool, what: String) {
    if !holds {
        eprintln!("ROW EXPECTATION NOT MET: {what}");
        failures.push(what);
    }
}

/// The timeline of the member `desk` as the Console reads it for one mob:
/// the selected hosted mob, or the launch mob when none is named.
fn timeline_of_desk(gateway: &Gateway, bearer: &str, mob: Option<&str>) -> Value {
    let params = json!({ "identity": DESK, "limit": 50 });
    gateway.console_rpc(
        bearer,
        "meta/console/query_timeline",
        match mob {
            Some(mob) => in_mob(mob, params),
            None => params,
        },
    )
}

#[test]
fn a_console_send_to_a_selected_hosted_mob_persists_under_it_and_its_recovery_reads_it() {
    let state = tempfile::tempdir().expect("state dir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let xdg_state = tempfile::tempdir().expect("XDG state dir");
    let log = Arc::new(Mutex::new(Vec::new()));
    let base_url = start_model(Arc::clone(&log));
    let host_config = config_dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");
    let sender = token("sender-subject", SENDER_EMAIL);

    let mut gateway = boot("life-1", state.path(), &host_config, xdg_state.path());

    // The stdio host creates the second mob and, in it, an agent of the
    // name the launch mob's roster also has.
    gateway.call(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    let created = gateway.call(
        "meta/identity/create",
        json!({
            "mob_id": SECOND_MOB,
            "request_id": "desk-of-the-second-mob",
            "agents": [{ "name": DESK, "profile": "specialist" }],
        }),
    );
    assert_eq!(
        created["outcome"],
        json!("done"),
        "the create of the agent `desk` in the second mob commits: {created}"
    );

    // READING 1 (the rig): the same sender reads the hosted mob's agents
    // through a Console method that is served per mob. A refusal of the
    // send below is then not a refusal of this sender or of this mob.
    let listed = gateway.console_rpc(&sender, "meta/identity/list", in_mob(SECOND_MOB, json!({})));
    eprintln!("ROW reading 1, meta/identity/list of the hosted mob: {listed}");
    assert!(
        answered(&listed) && listed["result"].to_string().contains(DESK),
        "the sender reads the hosted mob's agent through the Console: {listed}\n{}",
        gateway.stderr_tail()
    );

    // READING 2 (the rig): a Console send and its recovery deliver for the
    // launch mob's `control` member. The launch mob's `desk` is not sent to.
    let control = gateway.console_rpc(
        &sender,
        "meta/console/send",
        send_params(CONTROL, CONTROL_TEXT, CONTROL_KEY),
    );
    eprintln!("ROW reading 2, the control send to the launch mob: {control}");
    assert!(
        answered(&control),
        "the Console send works for a member of the launch mob: {control}\n{}",
        gateway.stderr_tail()
    );
    let control_delivered = eventually(SETTLE_TIMEOUT, || {
        gateway.console_rpc(
            &sender,
            "meta/console/recover_send",
            recover_params(CONTROL, CONTROL_KEY),
        )["result"]["status"]
            == "delivered"
    });
    // A send is marked delivered at its admission, which can precede the
    // model's request: wait for the request before counting it.
    let control_reached = eventually(SETTLE_TIMEOUT, || requests_with(&log, CONTROL_TEXT) >= 1);
    let control_requests = requests_with(&log, CONTROL_TEXT);
    eprintln!(
        "ROW reading 2, model requests that carry the control send's text: {control_requests}"
    );
    assert!(
        control_delivered && control_reached && control_requests == 1,
        "the control send is delivered and reaches the model once ({control_requests} requests)\n{}",
        gateway.stderr_tail()
    );

    // STEP 1: the send to `desk` of the SELECTED hosted mob.
    let sent = gateway.console_rpc(
        &sender,
        "meta/console/send",
        in_mob(SECOND_MOB, send_params(DESK, HOSTED_TEXT, HOSTED_KEY)),
    );
    eprintln!("ROW step 1, meta/console/send to the selected hosted mob: {sent}");
    assert!(
        answered(&sent),
        "STEP 1 REFUSED: the Console's send to a member of the selected hosted mob: {sent}\n{}",
        gateway.stderr_tail()
    );
    let frame = sent["result"]["input_frame_id"].clone();
    assert!(
        frame.is_string(),
        "the send answers its input frame: {sent}"
    );

    // From here every step runs and prints its answers; an expectation that
    // does not hold is recorded, and the row fails at its end with all of
    // them.
    let mut failures = Vec::new();
    let frame_text = frame.as_str().unwrap_or_default().to_string();

    // STEP 2: the recovery by the same key, for the same mob, reads it.
    let delivered = eventually(SETTLE_TIMEOUT, || {
        read_hosted_send_and_bystander(&gateway, &sender).0["result"]["status"] == "delivered"
    });
    let (recovered, bystander) = read_hosted_send_and_bystander(&gateway, &sender);
    eprintln!("ROW step 2, meta/console/recover_send for the selected hosted mob: {recovered}");
    eprintln!(
        "ROW step 3, meta/console/recover_send of the same key for the launch mob: {bystander}"
    );
    expect(
        &mut failures,
        answered(&recovered),
        format!("STEP 2: the recovery for the selected mob answers: {recovered}"),
    );
    expect(
        &mut failures,
        recovered["result"]["input_frame_id"] == frame,
        format!("STEP 2: the recovery reads the send it was asked about: {recovered}"),
    );
    expect(
        &mut failures,
        delivered,
        format!(
            "STEP 2: the recovery for the selected mob reads the send as delivered: {recovered}"
        ),
    );
    // As for the control send: the request can follow the delivered mark.
    let _reached = eventually(SETTLE_TIMEOUT, || requests_with(&log, HOSTED_TEXT) >= 1);
    let model_requests = requests_with(&log, HOSTED_TEXT);
    eprintln!("ROW step 1, model requests that carry the hosted send's text: {model_requests}");
    expect(
        &mut failures,
        model_requests == 1,
        format!("STEP 1: the send reached the model once ({model_requests} requests)"),
    );
    let hosted_timeline = timeline_of_desk(&gateway, &sender, Some(SECOND_MOB));
    let launch_timeline = timeline_of_desk(&gateway, &sender, None);
    eprintln!(
        "ROW step 2, the timeline of `desk` read for the selected hosted mob: {hosted_timeline}"
    );
    eprintln!("ROW step 3, the timeline of `desk` read for the launch mob: {launch_timeline}");
    expect(
        &mut failures,
        answered(&hosted_timeline) && hosted_timeline.to_string().contains(&frame_text),
        format!(
            "STEP 2: the hosted mob's timeline of `desk` shows the send's frame: {hosted_timeline}"
        ),
    );

    // STEP 3: the launch mob's member of the name holds no send, and the
    // key finds none there.
    expect(
        &mut failures,
        bystander["result"]["input_frame_id"] != frame,
        format!(
            "STEP 3: the key reads the hosted mob's send for the launch mob's member: {bystander}"
        ),
    );
    expect(
        &mut failures,
        bystander["result"]["status"] != "delivered",
        format!("STEP 3: the key finds a delivered send for the launch mob's member: {bystander}"),
    );
    expect(
        &mut failures,
        !launch_timeline.to_string().contains(&frame_text),
        format!(
            "STEP 3: the launch mob's timeline of `desk` shows the hosted send's frame: {launch_timeline}"
        ),
    );

    gateway.shutdown_and_reap();
    drop(gateway);
    let stored = stored_sends(state.path());
    eprintln!("ROW the Console store's sends after the first life: {stored:?}");
    let of_desk: Vec<_> = stored
        .iter()
        .filter(|(_, identity, _)| identity == DESK)
        .collect();
    expect(
        &mut failures,
        of_desk.len() == 1,
        format!("STEP 1 and 3: one send is kept for a member named `desk`: {stored:?}"),
    );
    expect(
        &mut failures,
        of_desk.iter().all(|(mob, _, _)| mob == SECOND_MOB),
        format!(
            "STEP 1: the send is kept under the selected hosted mob, and none under the launch mob: {stored:?}"
        ),
    );

    // STEP 4: the same reads from a gateway started again over the same
    // state directory.
    let mut again = boot("life-2", state.path(), &host_config, xdg_state.path());
    let (recovered_again, bystander_again) = read_hosted_send_and_bystander(&again, &sender);
    eprintln!(
        "ROW step 4, the recovery for the selected hosted mob after the restart: {recovered_again}"
    );
    eprintln!("ROW step 4, the recovery for the launch mob after the restart: {bystander_again}");
    expect(
        &mut failures,
        answered(&recovered_again) && recovered_again["result"]["input_frame_id"] == frame,
        format!(
            "STEP 4: the recovery for the selected mob reads the same send after the restart: {recovered_again}"
        ),
    );
    expect(
        &mut failures,
        recovered_again["result"]["status"] == "delivered",
        format!("STEP 4: the send reads delivered after the restart: {recovered_again}"),
    );
    expect(
        &mut failures,
        bystander_again["result"]["input_frame_id"] != frame
            && bystander_again["result"]["status"] != "delivered",
        format!("STEP 4: the launch mob's member still holds no send: {bystander_again}"),
    );
    let hosted_timeline_again = timeline_of_desk(&again, &sender, Some(SECOND_MOB));
    let launch_timeline_again = timeline_of_desk(&again, &sender, None);
    eprintln!(
        "ROW step 4, the timeline of `desk` for the selected hosted mob after the restart: {hosted_timeline_again}"
    );
    eprintln!(
        "ROW step 4, the timeline of `desk` for the launch mob after the restart: {launch_timeline_again}"
    );
    expect(
        &mut failures,
        answered(&hosted_timeline_again)
            && hosted_timeline_again.to_string().contains(&frame_text)
            && !launch_timeline_again.to_string().contains(&frame_text),
        format!(
            "STEP 4: the send's frame is the hosted mob's after the restart: {hosted_timeline_again} {launch_timeline_again}"
        ),
    );
    let model_requests_again = requests_with(&log, HOSTED_TEXT);
    eprintln!(
        "ROW step 4, model requests that carry the hosted send's text: {model_requests_again}"
    );
    expect(
        &mut failures,
        model_requests_again == model_requests,
        format!(
            "STEP 4: the restart delivered the send no second time ({model_requests} requests before, {model_requests_again} after)"
        ),
    );
    again.shutdown_and_reap();
    drop(again);
    let stored_again = stored_sends(state.path());
    eprintln!("ROW the Console store's sends after the second life: {stored_again:?}");
    expect(
        &mut failures,
        stored_again
            .iter()
            .map(|(mob, identity, _)| (mob, identity))
            .eq(stored.iter().map(|(mob, identity, _)| (mob, identity))),
        format!(
            "STEP 4: the restart kept the stored sends and their mobs: {stored:?} then {stored_again:?}"
        ),
    );

    assert!(
        failures.is_empty(),
        "{} expectation(s) of steps 2 to 4 did not hold:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

const UNRECOVERED_HOSTED_KEY: &str = "hosted-send-key-never-recovered";
const UNRECOVERED_HOSTED_TEXT: &str = "UNRECOVERED-HOSTED please check the second mob's desk";
const UNRECOVERED_CONTROL_KEY: &str = "control-send-key-never-recovered";
const UNRECOVERED_CONTROL_TEXT: &str = "UNRECOVERED-CONTROL please check the launch mob's control";

/// The status of the frame `frame` in a timeline answer, when it is there.
fn frame_status(timeline: &Value, frame: &Value) -> Option<String> {
    timeline["result"]["frames"]
        .as_array()?
        .iter()
        .find(|entry| entry["id"] == *frame)
        .and_then(|entry| entry["status"].as_str())
        .map(str::to_string)
}

/// The timeline of `identity` as the Console reads it for one mob.
fn timeline_of(gateway: &Gateway, bearer: &str, identity: &str, mob: Option<&str>) -> Value {
    let params = json!({ "identity": identity, "limit": 50 });
    gateway.console_rpc(
        bearer,
        "meta/console/query_timeline",
        match mob {
            Some(mob) => in_mob(mob, params),
            None => params,
        },
    )
}

/// A send to a member of the selected hosted mob is marked delivered by its
/// own delivery: its recovery is NEVER asked in this row, so nothing but
/// the delivery can have settled the frame. After the send's one model
/// request, the member's timeline read for that mob and the stored send
/// row (read with the gateway stopped) say `delivered`.
///
/// The same reading is made first for a member of the launch mob, whose
/// recovery is not asked either: it shows that a delivered send reads
/// `delivered` in this rig without a recovery.
#[test]
fn a_console_send_to_a_hosted_mob_reads_delivered_without_its_recovery_being_asked() {
    let state = tempfile::tempdir().expect("state dir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let xdg_state = tempfile::tempdir().expect("XDG state dir");
    let log = Arc::new(Mutex::new(Vec::new()));
    let base_url = start_model(Arc::clone(&log));
    let host_config = config_dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");
    let sender = token("sender-subject", SENDER_EMAIL);

    let mut gateway = boot("unrecovered", state.path(), &host_config, xdg_state.path());
    gateway.call(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    let created = gateway.call(
        "meta/identity/create",
        json!({
            "mob_id": SECOND_MOB,
            "request_id": "desk-of-the-second-mob",
            "agents": [{ "name": DESK, "profile": "specialist" }],
        }),
    );
    assert_eq!(created["outcome"], json!("done"), "{created}");

    // THE RIG's READING: a send to the launch mob's `control` member, whose
    // recovery is never asked, reads delivered in its timeline.
    let control = gateway.console_rpc(
        &sender,
        "meta/console/send",
        send_params(CONTROL, UNRECOVERED_CONTROL_TEXT, UNRECOVERED_CONTROL_KEY),
    );
    eprintln!("ROW unrecovered, the control send to the launch mob: {control}");
    assert!(answered(&control), "{control}\n{}", gateway.stderr_tail());
    let control_frame = control["result"]["input_frame_id"].clone();
    let control_marked = eventually(SETTLE_TIMEOUT, || {
        frame_status(
            &timeline_of(&gateway, &sender, CONTROL, None),
            &control_frame,
        )
        .as_deref()
            == Some("delivered")
    });
    let control_timeline = timeline_of(&gateway, &sender, CONTROL, None);
    eprintln!("ROW unrecovered, the launch mob's timeline of `control`: {control_timeline}");
    // A send is marked delivered at its admission, which can precede the
    // model's request: wait for the request before counting it.
    let control_reached = eventually(SETTLE_TIMEOUT, || {
        requests_with(&log, UNRECOVERED_CONTROL_TEXT) >= 1
    });
    let control_requests = requests_with(&log, UNRECOVERED_CONTROL_TEXT);
    eprintln!(
        "ROW unrecovered, model requests that carry the control send's text: {control_requests}"
    );
    assert!(
        control_marked && control_reached && control_requests == 1,
        "a launch mob's send reads delivered without its recovery and reaches the model once ({control_requests} requests): {control_timeline}\n{}",
        gateway.stderr_tail()
    );

    // THE ROW: the send to `desk` of the selected hosted mob. No recovery.
    let sent = gateway.console_rpc(
        &sender,
        "meta/console/send",
        in_mob(
            SECOND_MOB,
            send_params(DESK, UNRECOVERED_HOSTED_TEXT, UNRECOVERED_HOSTED_KEY),
        ),
    );
    eprintln!("ROW unrecovered, meta/console/send to the selected hosted mob: {sent}");
    assert!(
        answered(&sent),
        "the Console's send to a member of the selected hosted mob: {sent}\n{}",
        gateway.stderr_tail()
    );
    let frame = sent["result"]["input_frame_id"].clone();
    let reached_the_model = eventually(SETTLE_TIMEOUT, || {
        requests_with(&log, UNRECOVERED_HOSTED_TEXT) >= 1
    });
    assert!(
        reached_the_model,
        "the hosted send reaches the model\n{}",
        gateway.stderr_tail()
    );
    // The mark of a delivered send follows its admission; the launch mob's
    // reading above showed it inside this bound.
    let marked = eventually(SETTLE_TIMEOUT, || {
        frame_status(
            &timeline_of(&gateway, &sender, DESK, Some(SECOND_MOB)),
            &frame,
        )
        .as_deref()
            == Some("delivered")
    });
    let timeline = timeline_of(&gateway, &sender, DESK, Some(SECOND_MOB));
    let status_in_timeline = frame_status(&timeline, &frame);
    let model_requests = requests_with(&log, UNRECOVERED_HOSTED_TEXT);
    eprintln!("ROW unrecovered, the hosted mob's timeline of `desk`: {timeline}");
    eprintln!(
        "ROW unrecovered, the frame's status in the timeline: {status_in_timeline:?}; model requests: {model_requests}"
    );

    gateway.shutdown_and_reap();
    drop(gateway);
    let stored = stored_sends(state.path());
    eprintln!("ROW unrecovered, the Console store's sends with the gateway stopped: {stored:?}");

    assert_eq!(model_requests, 1, "the hosted send reached the model once");
    assert!(
        marked,
        "the hosted mob's send reads delivered in its timeline without a recovery: {status_in_timeline:?}"
    );
    let of_desk: Vec<_> = stored
        .iter()
        .filter(|(_, identity, _)| identity == DESK)
        .collect();
    assert_eq!(
        of_desk,
        [&(
            SECOND_MOB.to_string(),
            DESK.to_string(),
            "delivered".to_string()
        )],
        "the stored send of the hosted mob's member is marked delivered: {stored:?}"
    );
}

const STEERED_FIRST_KEY: &str = "hosted-send-key-held-run";
const STEERED_FIRST_TEXT: &str =
    "STEERED-FIRST HOLD-UNTIL-RELEASED please start on the second mob's desk";
const STEERED_FIRST_TOOL_TEXT: &str =
    "STEERED-FIRST HOLD-UNTIL-RELEASED please look up the second mob's desk CALL-THE-PEERS-TOOL";
const STEER_KEY: &str = "hosted-steer-key-1";
const STEER_TEXT: &str = "STEER-INTO-THE-RUN please also check the second mob's ledger";
const CONTROL_STEERED_FIRST_KEY: &str = "control-send-key-held-run";
const CONTROL_STEERED_FIRST_TEXT: &str =
    "CONTROL-FIRST HOLD-UNTIL-RELEASED please start on the launch mob's control";
const CONTROL_STEERED_FIRST_TOOL_TEXT: &str =
    "CONTROL-FIRST HOLD-UNTIL-RELEASED please look up the launch mob's control CALL-THE-PEERS-TOOL";
const CONTROL_STEER_KEY: &str = "control-steer-key-1";
const CONTROL_STEER_TEXT: &str = "CONTROL-STEER please also check the launch mob's ledger";
/// How long a steer's frame is read while its member's run is still held.
const HELD_READ_WINDOW: Duration = Duration::from_secs(10);
/// How long the row waits for a steer's own interaction to end before it
/// prints the member's timeline. Not an expectation.
const RUNS_SETTLE_WINDOW: Duration = Duration::from_secs(20);

/// The frame `frame` of a timeline answer, whole, when it is there.
fn frame_of<'a>(timeline: &'a Value, frame: &Value) -> Option<&'a Value> {
    timeline["result"]["frames"]
        .as_array()?
        .iter()
        .find(|entry| entry["id"] == *frame)
}

/// What a timeline answer shows of the member's runs and their ends, for the
/// row's print: each `run_started` and `interaction_complete` frame with its
/// cursor, its source, its run, its interaction, its parent frame, its
/// status and its payload's reason.
fn runs_of(timeline: &Value) -> Vec<String> {
    timeline["result"]["frames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|frame| frame["kind"] == "run_started" || frame["kind"] == "interaction_complete")
        .map(|frame| {
            format!(
                "{} {} source {} run {} interaction {} parent {} status {} reason {}",
                frame["cursor"],
                frame["kind"],
                frame["source"]["kind"],
                frame["payload"]["run_id"],
                frame["interaction_id"],
                frame["parent_frame_id"],
                frame["status"],
                frame["payload"]["reason"]
            )
        })
        .collect()
}

/// Whether a timeline answer holds the end of the interaction `interaction`.
fn interaction_ended(timeline: &Value, interaction: &Value) -> bool {
    timeline["result"]["frames"]
        .as_array()
        .is_some_and(|frames| {
            frames.iter().any(|frame| {
                frame["kind"] == "interaction_complete" && frame["interaction_id"] == *interaction
            })
        })
}

/// The steer terminal of the send frame `frame` in a timeline answer, when
/// one is there: a frame of kind `interaction_complete` whose payload's
/// reason is `steer_delivered` and whose parent is that send's frame.
fn steer_terminal_of<'a>(timeline: &'a Value, frame: &Value) -> Option<&'a Value> {
    timeline["result"]["frames"]
        .as_array()?
        .iter()
        .find(|entry| {
            entry["kind"] == "interaction_complete"
                && entry["payload"]["reason"] == "steer_delivered"
                && entry["parent_frame_id"] == *frame
        })
}

/// How many `run_started` frames of a timeline answer belong to the
/// interaction `interaction`.
fn runs_started_under(timeline: &Value, interaction: &Value) -> usize {
    timeline["result"]["frames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|frame| frame["kind"] == "run_started" && frame["interaction_id"] == *interaction)
        .count()
}

/// Whether a model request carries the answer of a tool call.
fn carries_a_tool_answer(request: &Value) -> bool {
    request["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"))
}

/// How the model ends its held answer to a member's first send.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeldAnswer {
    /// A final text: the member's run has no further model request.
    FinalText,
    /// One call of the `peers` tool: the member's run makes a next request,
    /// the one that carries the tool's answer.
    ToolCall,
}

/// One member's part of a steer row: the mob it is addressed in (none for
/// the launch mob), the member, and the texts and keys of its two sends.
struct SteerTarget {
    label: &'static str,
    mob: Option<&'static str>,
    identity: &'static str,
    first_text: &'static str,
    first_key: &'static str,
    steer_text: &'static str,
    steer_key: &'static str,
    held_answer: HeldAnswer,
}

impl SteerTarget {
    fn hosted(held_answer: HeldAnswer) -> Self {
        Self {
            label: "the hosted mob's `desk`",
            mob: Some(SECOND_MOB),
            identity: DESK,
            first_text: match held_answer {
                HeldAnswer::FinalText => STEERED_FIRST_TEXT,
                HeldAnswer::ToolCall => STEERED_FIRST_TOOL_TEXT,
            },
            first_key: STEERED_FIRST_KEY,
            steer_text: STEER_TEXT,
            steer_key: STEER_KEY,
            held_answer,
        }
    }

    fn launch(held_answer: HeldAnswer) -> Self {
        Self {
            label: "the launch mob's `control`",
            mob: None,
            identity: CONTROL,
            first_text: match held_answer {
                HeldAnswer::FinalText => CONTROL_STEERED_FIRST_TEXT,
                HeldAnswer::ToolCall => CONTROL_STEERED_FIRST_TOOL_TEXT,
            },
            first_key: CONTROL_STEERED_FIRST_KEY,
            steer_text: CONTROL_STEER_TEXT,
            steer_key: CONTROL_STEER_KEY,
            held_answer,
        }
    }

    fn addressed(&self, params: Value) -> Value {
        match self.mob {
            Some(mob) => in_mob(mob, params),
            None => params,
        }
    }

    fn timeline(&self, gateway: &Gateway, bearer: &str) -> Value {
        timeline_of(gateway, bearer, self.identity, self.mob)
    }

    /// Start the member's run with a send whose model answer is held, steer
    /// the member while that run is in flight, and read the steer's frame
    /// while the answer is still held. Returns the steer's answer.
    fn hold_a_run_and_steer(
        &self,
        gateway: &Gateway,
        bearer: &str,
        log: &Arc<Mutex<Vec<Value>>>,
        hold: &ModelHold,
        failures: &mut Vec<String>,
    ) -> Value {
        use std::sync::atomic::Ordering;
        let label = self.label;
        let held_before = hold.held.load(Ordering::SeqCst);
        let first = gateway.console_rpc(
            bearer,
            "meta/console/send",
            self.addressed(send_params(self.identity, self.first_text, self.first_key)),
        );
        eprintln!("ROW steer, {label}: the first send: {first}");
        let in_flight = answered(&first)
            && eventually(SETTLE_TIMEOUT, || {
                hold.held.load(Ordering::SeqCst) > held_before
            });
        if !in_flight {
            hold.on.store(false, Ordering::SeqCst);
            panic!(
                "the first send's run of {label} reaches the model and is held there: {first}\n{}",
                gateway.stderr_tail()
            );
        }
        let offered: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .find(|request| carries(request, self.first_text))
            .and_then(|request| request["tools"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();
        eprintln!("ROW steer, {label}: the tools the held request offers the model: {offered:?}");
        if self.held_answer == HeldAnswer::ToolCall {
            expect(
                failures,
                offered.iter().any(|tool| tool == PEERS_TOOL),
                format!(
                    "{label} has the `{PEERS_TOOL}` tool the model is about to call: {offered:?}"
                ),
            );
        }

        let mut steer_params = send_params(self.identity, self.steer_text, self.steer_key);
        steer_params["handling_mode"] = json!("steer");
        let steer = gateway.console_rpc(bearer, "meta/console/send", self.addressed(steer_params));
        eprintln!("ROW steer, {label}: meta/console/send with handling_mode steer: {steer}");
        expect(
            failures,
            answered(&steer),
            format!("the steer to {label}, whose run is in flight, is accepted: {steer}"),
        );
        let steer_frame = &steer["result"]["input_frame_id"];
        let at_once = frame_status(&self.timeline(gateway, bearer), steer_frame);
        let delivered_while_held = eventually(HELD_READ_WINDOW, || {
            frame_status(&self.timeline(gateway, bearer), steer_frame).as_deref()
                == Some("delivered")
        });
        let held = self.timeline(gateway, bearer);
        eprintln!(
            "ROW steer, {label}: the steer's frame while the run is held: {}",
            frame_of(&held, steer_frame).map_or_else(|| "absent".to_string(), Value::to_string)
        );
        eprintln!(
            "ROW steer, {label}: WHILE HELD the steer's frame read {at_once:?} at once and {:?} at the end of the window (delivered within it: {delivered_while_held}); steer terminal while held: {}; runs and ends while held: {:?}",
            frame_status(&held, steer_frame),
            steer_terminal_of(&held, steer_frame)
                .map_or_else(|| "absent".to_string(), Value::to_string),
            runs_of(&held)
        );
        steer
    }

    /// With the held answers released: the model receives the steer, the
    /// steer's frame reads delivered, and the member's timeline holds the
    /// steer's terminal (an `interaction_complete` frame with the reason
    /// `steer_delivered`, its parent and its cause the steer's frame, its
    /// source synthetic; under the hosted mob for that mob's member). Where
    /// the held answer was a tool call, the request that carries the tool's
    /// answer carries the steer, and the steer starts no run of its own.
    /// Returns the member's timeline.
    fn read_after_the_release(
        &self,
        gateway: &Gateway,
        bearer: &str,
        log: &Arc<Mutex<Vec<Value>>>,
        steer: &Value,
        failures: &mut Vec<String>,
    ) -> Value {
        let label = self.label;
        let steer_frame = &steer["result"]["input_frame_id"];
        let steer_interaction = &steer["result"]["interaction_id"];
        let carried = eventually(SETTLE_TIMEOUT, || requests_with(log, self.steer_text) >= 1);
        let marked = eventually(SETTLE_TIMEOUT, || {
            frame_status(&self.timeline(gateway, bearer), steer_frame).as_deref()
                == Some("delivered")
        });
        let settled = eventually(RUNS_SETTLE_WINDOW, || {
            interaction_ended(&self.timeline(gateway, bearer), steer_interaction)
        });
        let (with_first, with_steer, with_both, with_tool_answer, with_steer_and_tool_answer) = {
            let log = log.lock().unwrap();
            let count = |holds: &dyn Fn(&Value) -> bool| {
                log.iter().filter(|request| holds(request)).count()
            };
            (
                count(&|request| carries(request, self.first_text)),
                count(&|request| carries(request, self.steer_text)),
                count(&|request| {
                    carries(request, self.steer_text) && carries(request, self.first_text)
                }),
                count(&|request| {
                    carries(request, self.first_text) && carries_a_tool_answer(request)
                }),
                count(&|request| {
                    carries(request, self.steer_text) && carries_a_tool_answer(request)
                }),
            )
        };
        eprintln!(
            "ROW steer, {label}: model requests: {with_first} with the first send's text, {with_steer} with the steer's text, {with_both} with both, {with_tool_answer} with the first send and a tool's answer, {with_steer_and_tool_answer} with the steer and a tool's answer"
        );
        expect(
            failures,
            carried,
            format!("a model request after the release carries the steer to {label}"),
        );
        expect(
            failures,
            with_both >= 1,
            format!(
                "the request that carries the steer to {label} also carries its first send: the steer reached that member's conversation"
            ),
        );

        let timeline = self.timeline(gateway, bearer);
        let terminal = steer_terminal_of(&timeline, steer_frame);
        let runs_of_the_steer = runs_started_under(&timeline, steer_interaction);
        eprintln!(
            "ROW steer, {label}: the steer's frame after the release: {}",
            frame_of(&timeline, steer_frame).map_or_else(|| "absent".to_string(), Value::to_string)
        );
        eprintln!(
            "ROW steer, {label}: the steer terminal after the release: {}",
            terminal.map_or_else(|| "absent".to_string(), Value::to_string)
        );
        eprintln!("ROW steer, {label}: the timeline: {timeline}");
        eprintln!(
            "ROW steer, {label}: AFTER THE RELEASE the steer's frame reads {:?} (its interaction ended in the timeline: {settled}); run starts under the steer's interaction: {runs_of_the_steer}; runs and ends: {:?}",
            frame_status(&timeline, steer_frame),
            runs_of(&timeline)
        );
        expect(
            failures,
            marked,
            format!(
                "the steer's frame of {label} reads delivered without a recovery: {:?}",
                frame_status(&timeline, steer_frame)
            ),
        );
        // The steer's terminal, in both scenes: a delivered steer has one.
        expect(
            failures,
            terminal.is_some(),
            format!(
                "the timeline of {label} holds the steer's terminal: an interaction_complete frame with the reason steer_delivered whose parent is the steer's frame"
            ),
        );
        if let Some(terminal) = terminal {
            expect(
                failures,
                terminal["caused_by_frame_id"] == *steer_frame
                    && terminal["source"]["kind"] == "synthetic"
                    && terminal["status"] == "completed"
                    && terminal["payload"]["handling_mode"] == "steer",
                format!(
                    "the steer terminal of {label} is caused by the steer's frame, its source is synthetic, its status completed and its handling mode steer: {terminal}"
                ),
            );
            if let Some(mob) = self.mob {
                expect(
                    failures,
                    terminal["mob_id"] == mob,
                    format!("the steer terminal of {label} carries that mob's id: {terminal}"),
                );
            }
        }
        if self.held_answer == HeldAnswer::ToolCall {
            expect(
                failures,
                with_steer_and_tool_answer >= 1,
                format!(
                    "the next request of the run of {label}, the one with the tool's answer, carries the steer ({with_steer_and_tool_answer})"
                ),
            );
            expect(
                failures,
                runs_of_the_steer == 0,
                format!(
                    "the steer to {label} starts no run of its own ({runs_of_the_steer} run starts under its interaction)"
                ),
            );
        }
        timeline
    }
}

/// The steer scene of both rows below: a steer to a member of the selected
/// hosted mob, and the same steer to a member of the launch mob beside it
/// as the control. The model holds its answer to each member's first send,
/// so each member's run is in flight when a second Console send with
/// `handling_mode: "steer"` arrives for it. Each steer is accepted; once the
/// held answers are released the model receives each steer in a request
/// that also carries that member's first send; and each steer's frame, read
/// for its mob, reaches `delivered`. The hosted mob's steer carries that
/// mob's id on its frame and is stored under it. No recovery is asked, and
/// the launch mob's member that shares the hosted member's name is left
/// without a frame.
///
/// What the scene PRINTS and does not judge: each steer's frame while its
/// run is held and after, and each member's run starts and interaction
/// ends, for both mobs side by side.
///
/// Every expectation is recorded and the scene goes on, so that one run
/// prints all of that whatever holds.
fn steer_scene(held_answer: HeldAnswer) {
    use std::sync::atomic::Ordering;

    let state = tempfile::tempdir().expect("state dir");
    let config_dir = tempfile::tempdir().expect("config dir");
    let xdg_state = tempfile::tempdir().expect("XDG state dir");
    let log = Arc::new(Mutex::new(Vec::new()));
    let hold = Arc::new(ModelHold::default());
    let base_url = start_model_with_hold(Arc::clone(&log), Arc::clone(&hold));
    let host_config = config_dir.path().join("config.toml");
    std::fs::write(&host_config, host_config_text(&base_url)).expect("write host config");
    let sender = token("sender-subject", SENDER_EMAIL);
    let hosted = SteerTarget::hosted(held_answer);
    let launch = SteerTarget::launch(held_answer);

    let mut gateway = boot("steer", state.path(), &host_config, xdg_state.path());
    gateway.call(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    let created = gateway.call(
        "meta/identity/create",
        json!({
            "mob_id": SECOND_MOB,
            "request_id": "desk-of-the-second-mob",
            "agents": [{ "name": DESK, "profile": "specialist" }],
        }),
    );
    assert_eq!(created["outcome"], json!("done"), "{created}");

    // Each member's first send starts its run, and the model holds its
    // answer; each member is steered while its run is in flight.
    let mut failures = Vec::new();
    hold.on.store(true, Ordering::SeqCst);
    let hosted_steer = hosted.hold_a_run_and_steer(&gateway, &sender, &log, &hold, &mut failures);
    let launch_steer = launch.hold_a_run_and_steer(&gateway, &sender, &log, &hold, &mut failures);
    let before_release = requests_with(&log, STEER_TEXT) + requests_with(&log, CONTROL_STEER_TEXT);
    expect(
        &mut failures,
        before_release == 0,
        format!(
            "no model request carries a steer while the first answers are held ({before_release})"
        ),
    );

    // The held answers are released.
    hold.on.store(false, Ordering::SeqCst);
    let hosted_timeline =
        hosted.read_after_the_release(&gateway, &sender, &log, &hosted_steer, &mut failures);
    launch.read_after_the_release(&gateway, &sender, &log, &launch_steer, &mut failures);
    let all_requests = log.lock().unwrap().len();
    eprintln!("ROW steer, model requests in all: {all_requests}");

    let hosted_frame = frame_of(&hosted_timeline, &hosted_steer["result"]["input_frame_id"])
        .cloned()
        .unwrap_or(Value::Null);
    expect(
        &mut failures,
        hosted_frame["mob_id"] == SECOND_MOB,
        format!("the hosted mob's steer carries that mob's id on its frame: {hosted_frame}"),
    );
    let bystander = timeline_of(&gateway, &sender, DESK, None);
    eprintln!("ROW steer, the launch mob's timeline of `desk`: {bystander}");
    expect(
        &mut failures,
        bystander["result"]["frames"]
            .as_array()
            .is_some_and(Vec::is_empty),
        format!("the launch mob's member of the hosted member's name has no frame: {bystander}"),
    );

    gateway.shutdown_and_reap();
    drop(gateway);
    let stored = stored_sends(state.path());
    eprintln!("ROW steer, the Console store's sends with the gateway stopped: {stored:?}");
    let of_desk: Vec<_> = stored
        .iter()
        .filter(|(_, identity, _)| identity == DESK)
        .collect();
    expect(
        &mut failures,
        of_desk.len() == 2 && of_desk.iter().all(|(mob, _, _)| mob == SECOND_MOB),
        format!(
            "the first send and the steer are stored under the hosted mob, and none under the launch mob: {stored:?}"
        ),
    );
    expect(
        &mut failures,
        of_desk.iter().all(|(_, _, status)| status == "delivered"),
        format!("the stored first send and the stored steer read delivered: {stored:?}"),
    );
    let of_control: Vec<_> = stored
        .iter()
        .filter(|(_, identity, _)| identity == CONTROL)
        .collect();
    expect(
        &mut failures,
        of_control.len() == 2
            && of_control
                .iter()
                .all(|(mob, _, status)| mob.is_empty() && status == "delivered"),
        format!(
            "the launch mob's first send and steer are stored under no hosted mob and read delivered: {stored:?}"
        ),
    );

    assert!(
        failures.is_empty(),
        "{} expectation(s) of the steer row did not hold:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A steer to a member whose run in flight ends with the held answer: the
/// model's held answer to the first send is a final text, so that run has
/// no further model request, and the steer starts a run of its own after
/// it. Its frame reads delivered and its terminal is in the timeline all
/// the same. See [`steer_scene`].
#[test]
fn a_console_steer_to_a_hosted_mobs_running_member_reaches_the_model_and_reads_delivered() {
    steer_scene(HeldAnswer::FinalText);
}

/// A steer to a member whose run in flight goes on: the model's held answer
/// to the first send is one call of the `peers` tool, so that run makes a
/// next model request, the one that carries the tool's answer. That request
/// carries the steer, the steer starts no run of its own, and the member's
/// timeline holds the steer's terminal. See [`steer_scene`].
#[test]
fn a_console_steer_to_a_hosted_mobs_member_in_a_tool_round_joins_its_run_and_is_marked() {
    steer_scene(HeldAnswer::ToolCall);
}
