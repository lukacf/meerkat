//! A member's schedule that creates a session, on the shipped stdio gateway.
//!
//! Each case is the `rpc_gateway --persistent` binary over a state directory
//! of its own, driven over real stdio as an SDK drives it. The model is a
//! loopback chat-completions server run by this test (a self-hosted server
//! that needs no key): a user message holding `CALL <tool> <json>` makes it
//! call that tool, everything else is answered "ok", and every request is
//! recorded, so what the member's tool call was answered is read from the
//! member's next request.
//!
//! A schedule whose target is a session created on demand has the gateway
//! build a fresh session at the fire, with the gateway's own tools. A mob
//! member is not built as the gateway builds a session by default, so its
//! write of such a schedule is refused and the schedule store holds nothing
//! for it. A gateway whose host configuration allows it by name
//! (`[tools] agent_schedules_may_create_sessions = true`) takes the same
//! write, and the member pauses and deletes what it wrote. On the stock
//! gateway the member's schedule for itself is stored as before.
//!
//! A schedule whose target spawns or forks a helper has the gateway create
//! a helper at the fire, from the role the schedule names. A member whose
//! profile holds the schedule tools and not the mob tools is refused it;
//! a member that holds the mob tools writes it as before.
//!
//! The last row runs the one read-only statement an operator uses to find
//! the schedules of these three kinds, on a store that holds each.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const MOB_ID: &str = "schedule-session-creation";
const MODEL: &str = "schedule-session-creation-test";
const IDENTITY: &str = "clerk";
/// A second member, whose profile also holds the mob tools.
const FOREMAN: &str = "foreman";
const REFUSAL: &str = "schedule_session_creation_not_allowed";
const ALLOWANCE: &str = "\n[tools]\nagent_schedules_may_create_sessions = true\n";

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(2);

fn mob_config() -> String {
    format!(
        r#"
[mob]
id = "{MOB_ID}"

[profiles.clerk]
model = "{MODEL}"
provider = "self_hosted"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.clerk.tools]
comms = true
schedule = true

[profiles.foreman]
model = "{MODEL}"
provider = "self_hosted"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.foreman.tools]
comms = true
schedule = true
mob = true
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

/// The host configuration: the loopback model for the gateway's realms,
/// followed by `more`.
fn host_config_text(base_url: &str, more: &str) -> String {
    let mut config = format!(
        r#"
[self_hosted]
default_model = "{MODEL}"

[self_hosted.servers.loop]
base_url = "{base_url}"

[self_hosted.models."{MODEL}"]
server = "loop"
remote_model = "{MODEL}"
display_name = "Schedule session creation test model"
family = "{MODEL}"
"#
    );
    config.push_str(more);
    config.push_str(&realm_binding("global"));
    config.push_str(&realm_binding(&format!("mob.{MOB_ID}")));
    config
}

// ---------------------------------------------------------------------------
// Loopback model
// ---------------------------------------------------------------------------

type ModelLog = Arc<Mutex<Vec<Value>>>;

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

/// The tool call the last message of `request` asks for: a user message
/// with a line `CALL <tool> <json arguments>`.
fn scripted_call(request: &Value) -> Option<(String, String)> {
    let last = request["messages"].as_array()?.last()?;
    if last["role"].as_str() != Some("user") {
        return None;
    }
    let text = message_text(last);
    let rest = &text[text.find("CALL ")? + "CALL ".len()..];
    let (name, arguments) = rest.split_once(' ')?;
    let arguments: Value = serde_json::Deserializer::from_str(arguments)
        .into_iter::<Value>()
        .next()?
        .ok()?;
    Some((name.to_string(), arguments.to_string()))
}

async fn chat_completions(
    axum::extract::State(log): axum::extract::State<ModelLog>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let call = scripted_call(&request);
    let number = {
        let mut log = log.lock().unwrap();
        log.push(request);
        log.len()
    };
    let usage = json!({ "prompt_tokens": 1, "completion_tokens": 1 });
    let chunks = match call {
        Some((name, arguments)) => vec![
            json!({ "choices": [{ "delta": { "tool_calls": [{
                "index": 0,
                "id": format!("call_{number}"),
                "type": "function",
                "function": { "name": name, "arguments": arguments }
            }] } }] }),
            json!({ "choices": [{ "finish_reason": "tool_calls" }], "usage": usage }),
        ],
        None => vec![
            json!({ "choices": [{ "delta": { "content": "ok" } }] }),
            json!({ "choices": [{ "finish_reason": "stop" }], "usage": usage }),
        ],
    };
    let mut payload = String::new();
    for chunk in chunks {
        payload.push_str(&format!("data: {chunk}\n\n"));
    }
    payload.push_str("data: [DONE]\n\n");
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

fn start_model(log: ModelLog) -> String {
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
// Child gateway process
// ---------------------------------------------------------------------------

struct Gateway {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
    log: ModelLog,
    schedule_db: PathBuf,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

impl Gateway {
    /// The stdio gateway over a fresh state directory, its host
    /// configuration the loopback model's followed by `more`, with two
    /// members: `clerk`, whose profile switches on comms and the schedule
    /// tools, and `foreman`, whose profile also switches on the mob tools.
    fn boot(label: &str, more: &str) -> Self {
        let state = tempfile::tempdir().expect("state dir");
        let config_dir = tempfile::tempdir().expect("config dir");
        let log: ModelLog = Arc::default();
        let base_url = start_model(Arc::clone(&log));
        let host_config = config_dir.path().join("config.toml");
        std::fs::write(&host_config, host_config_text(&base_url, more)).expect("write host config");

        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .current_dir(config_dir.path())
            .env_remove("RUST_LOG")
            // A state root of the case's own: the default one is shared by
            // every gateway process of the user.
            .env("XDG_STATE_HOME", config_dir.path().join("xdg-state"))
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
        let schedule_db =
            meerkat_meta::MetaStorageLayout::with_injected_roots(state.path().to_path_buf(), None)
                .schedule_db();
        let mut gateway = Self {
            label: label.to_string(),
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
            next_id: 0,
            log,
            schedule_db,
            _dirs: (state, config_dir),
        };
        let response = gateway.request(
            "meta/init",
            json!({
                "persistent_state": gateway._dirs.0.path(),
                "mob_config": mob_config(),
                "has_roster_provider": true,
                "runtime_options": {
                    "meerkat_config_path": host_config,
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
        gateway
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
                let member = |identity: &str| {
                    json!({
                        "identity": identity,
                        "profile": identity,
                        "addressability": "addressable",
                        "display_name": null,
                        "labels": {},
                        "context": null,
                        "additional_instructions": []
                    })
                };
                self.send(&json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": [member(IDENTITY), member(FOREMAN)]
                }));
            }
            other if other.starts_with("callback/") => panic!(
                "[{}] unexpected host callback {other}: this harness provides only a roster",
                self.label
            ),
            _ => {}
        }
    }

    /// [`Self::turn_of`] for the member `clerk`.
    fn member_turn(&mut self, content: &str) -> String {
        self.turn_of(IDENTITY, content)
    }

    /// One turn of the member `identity` on `content`, to its end: what
    /// the member's tool call of that turn was answered, read from the
    /// request the member's model got next (empty when the turn made no
    /// tool call).
    fn turn_of(&mut self, identity: &str, content: &str) -> String {
        let before = self.log.lock().unwrap().len();
        let sent = self.call(
            "meta/send",
            json!({ "identity": identity, "content": content, "track_turn": true }),
        );
        let ticket = sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("[{}] the send tracks its turn: {sent}", self.label))
            .to_string();
        let waited = self.call(
            "meta/wait_for_turn",
            json!({ "identity": identity, "ticket": ticket, "timeout_ms": 90_000 }),
        );
        assert_eq!(
            waited["state"],
            "completed",
            "[{}] the member's turn ends: {waited}\n{}",
            self.label,
            self.stderr_tail()
        );
        let log = self.log.lock().unwrap();
        log[before..]
            .iter()
            .filter_map(|request| request["messages"].as_array()?.last())
            .filter(|last| last["role"].as_str() == Some("tool"))
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The schedules the gateway's store holds that are not deleted, as
    /// (id, phase, target), read from the store file as an operator reads
    /// it: read-only, beside the live gateway.
    fn stored_schedules(&self) -> Vec<(String, String, Value)> {
        if !self.schedule_db.exists() {
            return Vec::new();
        }
        let connection = rusqlite::Connection::open_with_flags(
            &self.schedule_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open the schedule store read-only");
        let mut statement = connection
            .prepare(
                "SELECT schedule_id, phase, CAST(schedule_json AS TEXT) \
                 FROM schedule_schedules WHERE phase <> 'deleted' ORDER BY created_at_ms",
            )
            .expect("prepare the schedule query");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .expect("query the schedules")
            .map(|row| {
                let (id, phase, document) = row.expect("read a schedule");
                let document: Value = serde_json::from_str(&document).expect("schedule JSON");
                (id, phase, document["target"].clone())
            })
            .collect()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn creates_a_session(target: &Value) -> bool {
    target["type"].as_str() == Some("materialize_on_demand_session")
}

/// A schedule due in a day whose target is a session created on demand.
fn session_creating_schedule() -> String {
    format!(
        "CALL meerkat_schedule_create {}",
        json!({
            "name": "a fresh session",
            "trigger": {
                "type": "once",
                "due_at_utc": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            },
            "target": {
                "target_kind": "session",
                "type": "materialize_on_demand_session",
                "create": { "model": MODEL, "provider": "self_hosted" },
                "action": { "type": "prompt", "prompt": "Run the report." }
            }
        })
    )
}

/// On the stock stdio gateway a member's schedule that creates a session is
/// refused with the typed denial and nothing is stored for it. What the
/// refusal leaves the member: its schedule for itself is stored as before.
#[test]
fn a_members_schedule_that_creates_a_session_is_refused_on_the_stdio_gateway() {
    let mut gateway = Gateway::boot("stock", "");
    let answer = gateway.member_turn(&session_creating_schedule());
    assert!(
        answer.contains(REFUSAL),
        "the member's write is refused as a schedule that creates a session: {answer:?}\n{}",
        gateway.stderr_tail()
    );
    let stored = gateway.stored_schedules();
    assert!(
        stored.is_empty(),
        "nothing is stored for the member's write: {stored:?}"
    );

    let own = format!(
        "CALL meerkat_schedule_create {}",
        json!({
            "name": "a note to self",
            "trigger": {
                "type": "once",
                "due_at_utc": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            },
            "target": {
                "target_kind": "mob",
                "type": "member",
                "mob_id": MOB_ID,
                "member_id": IDENTITY,
                "action": { "type": "send", "content": "Check the queue." }
            }
        })
    );
    let answer = gateway.member_turn(&own);
    let stored = gateway.stored_schedules();
    assert!(
        stored.len() == 1 && !creates_a_session(&stored[0].2),
        "the member's schedule for itself is stored as before: {answer:?} {stored:?}\n{}",
        gateway.stderr_tail()
    );
}

/// On a stdio gateway whose host configuration allows it by name the same
/// write is taken: the store holds the schedule with its on-demand target,
/// which is the field an operator looks for. The member then pauses and
/// deletes it, as any member that holds the schedule tools can on a gateway
/// that does not allow the write.
#[test]
fn a_gateway_that_allows_it_takes_a_members_schedule_that_creates_a_session() {
    let mut gateway = Gateway::boot("allowed", ALLOWANCE);
    let answer = gateway.member_turn(&session_creating_schedule());
    let stored = gateway.stored_schedules();
    assert!(
        stored.len() == 1 && creates_a_session(&stored[0].2) && stored[0].1 == "active",
        "the member's schedule is stored: {answer:?} {stored:?}\n{}",
        gateway.stderr_tail()
    );
    let id = stored[0].0.clone();

    let answer = gateway.member_turn(&format!(
        "CALL meerkat_schedule_pause {}",
        json!({ "schedule_id": id })
    ));
    let stored = gateway.stored_schedules();
    assert!(
        stored.len() == 1 && stored[0].1 == "paused",
        "the member pauses it: {answer:?} {stored:?}"
    );
    let answer = gateway.member_turn(&format!(
        "CALL meerkat_schedule_delete {}",
        json!({ "schedule_id": id })
    ));
    let stored = gateway.stored_schedules();
    assert!(
        stored.is_empty(),
        "the member deletes it: {answer:?} {stored:?}"
    );
}

/// A schedule due in a day whose target spawns (`spawn_helper`) or forks
/// (`fork_helper`, from `writer`) a helper of the member's own mob.
fn helper_creating_schedule(kind: &str, writer: &str) -> String {
    let mut target = json!({
        "target_kind": "mob",
        "type": kind,
        "mob_id": MOB_ID,
        "member_id": format!("helper-of-{writer}-{kind}"),
        "prompt": "Do the scheduled work."
    });
    if kind == "fork_helper" {
        target["source_member_id"] = json!(writer);
    }
    format!(
        "CALL meerkat_schedule_create {}",
        json!({
            "name": format!("a helper ({kind})"),
            "trigger": {
                "type": "once",
                "due_at_utc": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            },
            "target": target
        })
    )
}

/// On the stock stdio gateway a member whose profile holds the schedule
/// tools and not the mob tools is refused a schedule that spawns or forks a
/// helper in its own mob, with the typed denial, and nothing is stored for
/// it. A member whose profile holds the mob tools writes one as before.
#[test]
fn a_member_without_the_mob_tools_schedules_no_helper_on_the_stdio_gateway() {
    let mut gateway = Gateway::boot("helpers", "");
    for kind in ["spawn_helper", "fork_helper"] {
        let answer = gateway.member_turn(&helper_creating_schedule(kind, IDENTITY));
        assert!(
            answer.contains(REFUSAL),
            "the member's {kind} schedule is refused: {answer:?}\n{}",
            gateway.stderr_tail()
        );
    }
    let stored = gateway.stored_schedules();
    assert!(
        stored.is_empty(),
        "nothing is stored for the member without the mob tools: {stored:?}"
    );

    let answer = gateway.turn_of(FOREMAN, &helper_creating_schedule("spawn_helper", FOREMAN));
    let stored = gateway.stored_schedules();
    assert!(
        stored.len() == 1 && stored[0].2["type"] == "spawn_helper",
        "the member that holds the mob tools schedules a helper as before: {answer:?} \
         {stored:?}\n{}",
        gateway.stderr_tail()
    );
}

/// The operator's one read-only statement for the schedules that have the
/// host create a session or a helper when they fire, run on the store of a
/// gateway that holds one of each kind beside a schedule of another kind:
/// it returns the three and leaves the other out. The gateway allows its
/// agents these schedules by name, so that its members can write all three.
#[test]
fn the_store_statement_finds_the_schedules_that_create_a_session_or_a_helper() {
    let mut gateway = Gateway::boot("statement", ALLOWANCE);
    gateway.member_turn(&session_creating_schedule());
    gateway.turn_of(FOREMAN, &helper_creating_schedule("spawn_helper", FOREMAN));
    gateway.turn_of(FOREMAN, &helper_creating_schedule("fork_helper", FOREMAN));
    let own = format!(
        "CALL meerkat_schedule_create {}",
        json!({
            "name": "a note to self",
            "trigger": {
                "type": "once",
                "due_at_utc": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
            },
            "target": {
                "target_kind": "mob",
                "type": "member",
                "mob_id": MOB_ID,
                "member_id": IDENTITY,
                "action": { "type": "send", "content": "Check the queue." }
            }
        })
    );
    gateway.member_turn(&own);
    let all = gateway.stored_schedules();
    assert_eq!(all.len(), 4, "the store holds the four schedules: {all:?}");

    let connection = rusqlite::Connection::open_with_flags(
        &gateway.schedule_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the schedule store read-only");
    let mut statement = connection
        .prepare(
            "SELECT schedule_id, phase, \
             json_extract(CAST(schedule_json AS TEXT),'$.name'), \
             json_extract(CAST(schedule_json AS TEXT),'$.target') \
             FROM schedule_schedules WHERE phase <> 'deleted' AND \
             json_extract(CAST(schedule_json AS TEXT),'$.target.type') IN \
             ('materialize_on_demand_session','spawn_helper','fork_helper')",
        )
        .expect("the operator's statement prepares");
    let mut found: Vec<(String, String)> = statement
        .query_map([], |row| {
            let target: String = row.get(3)?;
            Ok((row.get::<_, String>(1)?, target))
        })
        .expect("the operator's statement runs")
        .map(|row| {
            let (phase, target) = row.expect("a row of the statement");
            let target: Value = serde_json::from_str(&target).expect("the target is JSON");
            (
                phase,
                target["type"]
                    .as_str()
                    .expect("the target's type")
                    .to_string(),
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("active".to_string(), "fork_helper".to_string()),
            (
                "active".to_string(),
                "materialize_on_demand_session".to_string()
            ),
            ("active".to_string(), "spawn_helper".to_string()),
        ],
        "the statement finds the three kinds and leaves the member target out"
    );
}
