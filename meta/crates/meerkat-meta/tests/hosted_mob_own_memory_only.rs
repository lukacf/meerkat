//! The profiles a launch lists their own memory only, in a mob its host
//! creates beside the launch mob, through the REAL stdio gateway.
//!
//! A persistent gateway is launched with agent memory, an identity policy
//! and a loopback chat-completions model that keeps every request it is
//! sent. Its host creates a second mob through `meta/mobs/create` and two
//! long-term agents in it through `meta/identity/create`: one of a profile
//! the launch lists its own memory only, one of a profile it does not list.
//! The host writes a record into each agent's own memory through
//! `meta/agent_memory/remember`: after these writes the second mob's
//! memory directory stands, made by the gateway, a directory of its own
//! below the launch's memory root. The gateway is stopped. That
//! directory is opened by the store's public open and given a record
//! shared by the mob's agents. The gateway is launched again on the same
//! state and each agent takes one turn: its first. What the model was sent
//! for each turn is what the row reads.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use meerkat_meta::memory::MemoryAuthor;
use meerkat_meta::{
    AgentMemoryProvider, MemoryKind, MemoryScope, MetaStorageLayout, NewMemoryRecord,
    SqliteAgentMemoryStore,
};
use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const LAUNCH_MOB: &str = "own-memory-only-launch";
const SECOND_MOB: &str = "own-memory-only-second";
const MODEL: &str = "own-memory-only-model";

/// The profile the launch lists its own memory only.
const LISTED: &str = "keeper";
/// The profile the launch does not list.
const UNLISTED: &str = "worker";

/// The launch mob's one roster agent. It takes no turn.
const OF_THE_LAUNCH_MOB: &str = "launch-clerk";
/// The second mob's agent of the listed profile.
const KEEPER: &str = "keeper-a";
/// The second mob's agent of the unlisted profile.
const WORKER: &str = "worker-a";

const SHARED_BY_THE_SECOND_MOB: &str = "The second mob closes its till at noon";
const OF_THE_KEEPER: &str = "The keeper files receipts in the blue binder";
const OF_THE_WORKER: &str = "The worker counts stock on Thursdays";

const TURN_OF_THE_KEEPER: &str = "KEEPER-TURN say what you know";
const TURN_OF_THE_WORKER: &str = "WORKER-TURN say what you know";

/// The directory, below a launch's memory root, that holds the stores of
/// the mobs its host creates beside the launch mob.
const HOSTED_MOBS_DIR: &str = "hosted-mobs.d";

/// Hang detectors, not latency bounds.
const INIT_WAIT: Duration = Duration::from_mins(3);
const ANSWER_WAIT: Duration = Duration::from_mins(2);
const EXIT_WAIT: Duration = Duration::from_mins(1);

/// A mob of the two profiles, both keeping memory, both on the loopback
/// model, whose agents run turn by turn.
fn mob_config(mob_id: &str) -> String {
    format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.{LISTED}]
model = "{MODEL}"
provider = "self_hosted"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.{LISTED}.tools]
comms = true
memory = true

[profiles.{UNLISTED}]
model = "{MODEL}"
provider = "self_hosted"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.{UNLISTED}.tools]
comms = true
memory = true
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

/// The host's configuration: the loopback model, bound in the realm of
/// each of the two mobs.
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
display_name = "Own memory only test model"
family = "{MODEL}"
"#
    );
    config.push_str(&realm_binding("global"));
    config.push_str(&realm_binding(&format!("mob.{LAUNCH_MOB}")));
    config.push_str(&realm_binding(&format!("mob.{SECOND_MOB}")));
    config
}

// ---------------------------------------------------------------------------
// Loopback model that keeps every request
// ---------------------------------------------------------------------------

/// A streamed answer that ends the turn.
const DONE_REPLY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n\
                          data: {\"choices\":[{\"finish_reason\":\"stop\"}],\
                          \"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n\n\
                          data: [DONE]\n\n";

struct Model {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn chat_completions(
    axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    requests.lock().unwrap().push(request);
    ([("content-type", "text/event-stream")], DONE_REPLY)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

impl Model {
    fn start() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::clone(&requests);
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
        }
    }

    /// How many requests the model was sent.
    fn asked(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// The first request the model was sent that holds `turn_text`, as one
    /// JSON text: the first model request of the turn with that text.
    fn first_request_of(&self, turn_text: &str) -> String {
        let requests = self.requests.lock().unwrap();
        requests
            .iter()
            .map(Value::to_string)
            .find(|request| request.contains(turn_text))
            .unwrap_or_else(|| {
                panic!(
                    "the model was sent no request that holds {turn_text:?}; it was sent {}",
                    requests.len()
                )
            })
    }
}

// ---------------------------------------------------------------------------
// Child gateway process
// ---------------------------------------------------------------------------

struct Gateway {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
}

impl Gateway {
    fn spawn(scratch: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .current_dir(scratch)
            .env("XDG_STATE_HOME", scratch.join("xdg-state"))
            .env("XDG_CONFIG_HOME", scratch.join("xdg-config"))
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
        let (tx, lines) = mpsc::channel();
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
            child,
            stdin: Some(stdin),
            lines,
            stderr,
            next_id: 0,
        }
    }

    /// A gateway launched on `state` with agent memory, the loopback model
    /// of `host_config`, an identity policy that lets its host create
    /// agents, and `own_memory_only`: the profiles the launch lists their
    /// own memory only.
    fn launched(
        scratch: &Path,
        state: &Path,
        host_config: &Path,
        own_memory_only: &[&str],
    ) -> Self {
        let mut gateway = Self::spawn(scratch);
        let mut agent_memory = json!({ "selection": "always" });
        if !own_memory_only.is_empty() {
            agent_memory["own_memory_only_profiles"] = json!(own_memory_only);
        }
        let launched = gateway.answer_to(
            "meta/init",
            json!({
                "persistent_state": state,
                "mob_config": mob_config(LAUNCH_MOB),
                "has_roster_provider": true,
                "runtime_options": {
                    "meerkat_config_path": host_config,
                    "identity_creation": {
                        "enabled": true,
                        "approval": {
                            "rules": [{ "change": ["create"], "outcome": { "kind": "allow" } }]
                        }
                    },
                    "agent_memory": agent_memory,
                }
            }),
            INIT_WAIT,
        );
        assert!(
            launched.get("error").is_none(),
            "the launch is accepted: {launched}\n{}",
            gateway.stderr_tail()
        );
        gateway
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
    }

    fn stderr_tail(&self) -> String {
        let Ok(sink) = self.stderr.lock() else {
            return "[stderr unavailable]".to_string();
        };
        let start = sink.len().saturating_sub(80);
        format!("--- gateway stderr ---\n{}", sink[start..].join("\n"))
    }

    /// The gateway's request to its host is answered as the SDK host of
    /// the row answers: the launch mob's roster is one agent, and the host
    /// has no other provider.
    fn answer_callback(&mut self, message: &Value) {
        let (Some(asked), Some(id)) = (
            message.get("method").and_then(Value::as_str),
            message.get("id").cloned(),
        ) else {
            return;
        };
        let answer = match asked {
            "callback/roster_provider/roster" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": [{
                    "identity": OF_THE_LAUNCH_MOB,
                    "profile": UNLISTED,
                    "addressability": "addressable",
                    "display_name": null,
                    "labels": {},
                    "context": null,
                    "additional_instructions": []
                }],
            }),
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("no handler for {other}") },
            }),
        };
        self.send(&answer);
    }

    /// One request and its answer, error or not.
    fn answer_to(&mut self, method: &str, params: Value, wait: Duration) -> Value {
        self.next_id += 1;
        let id = format!("r{}", self.next_id);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let start = Instant::now();
        loop {
            let remaining = wait.saturating_sub(start.elapsed());
            let line = self.lines.recv_timeout(remaining).unwrap_or_else(|_| {
                panic!(
                    "no answer to {method} within {wait:?}\n{}",
                    self.stderr_tail()
                )
            });
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message["id"] == json!(id) && message.get("method").is_none() {
                return message;
            }
            self.answer_callback(&message);
        }
    }

    /// The result of a request that must succeed.
    fn result(&mut self, method: &str, params: Value) -> Value {
        let answer = self.answer_to(method, params.clone(), ANSWER_WAIT);
        assert!(
            answer.get("error").is_none(),
            "{method} {params} succeeds: {answer}\n{}",
            self.stderr_tail()
        );
        answer["result"].clone()
    }

    /// The host writes a record titled `title` into the own memory of the
    /// second mob's agent `name`, through the gateway.
    fn remember_in_the_second_mob(&mut self, name: &str, title: &str) {
        let written = self.result(
            "meta/agent_memory/remember",
            json!({
                "mob_id": SECOND_MOB,
                "identity": name,
                "title": title,
                "body": format!("{title}, in full."),
            }),
        );
        assert_eq!(
            written["title"],
            json!(title),
            "the host's write answers the record it wrote for {name}: {written}"
        );
    }

    /// One tracked turn of the second mob's agent `name`, waited until it
    /// completes.
    fn turn_in_the_second_mob(&mut self, name: &str, text: &str) {
        let sent = self.result(
            "meta/send",
            json!({
                "mob_id": SECOND_MOB,
                "identity": name,
                "content": text,
                "track_turn": true,
            }),
        );
        let ticket = sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("an untracked turn: {sent}"))
            .to_string();
        let waited = self.result(
            "meta/wait_for_turn",
            json!({
                "mob_id": SECOND_MOB,
                "identity": name,
                "ticket": ticket,
                "timeout_ms": 90_000,
            }),
        );
        assert_eq!(
            waited["state"],
            json!("completed"),
            "the turn {text:?} of {name} completes: {waited}\n{}",
            self.stderr_tail()
        );
    }

    /// The ids of the mobs the gateway lists, in its order.
    fn listed_mobs(&mut self) -> Vec<String> {
        self.result("meta/mobs/list", json!({}))["mobs"]
            .as_array()
            .expect("a list of mobs")
            .iter()
            .map(|mob| mob["mob_id"].as_str().expect("a mob id").to_string())
            .collect()
    }

    /// The shutdown handshake, which must complete its cleanup, then the
    /// exit: no process holds the state directory afterwards.
    fn shut_down(mut self, context: &str) {
        let shutdown = self.answer_to("meta/shutdown", json!({}), INIT_WAIT);
        gateway_shutdown::assert_persistent_shutdown_completed(
            &shutdown,
            &format!("{context}\n{}", self.stderr_tail()),
        );
        drop(self.stdin.take());
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                break status;
            }
            assert!(
                start.elapsed() < EXIT_WAIT,
                "{context}: the gateway did not exit within {EXIT_WAIT:?}\n{}",
                self.stderr_tail()
            );
            while let Ok(line) = self.lines.try_recv() {
                if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                    self.answer_callback(&message);
                }
            }
            thread::sleep(Duration::from_millis(25));
        };
        assert!(
            status.success(),
            "{context}: exit status {status}\n{}",
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

// ---------------------------------------------------------------------------
// The second mob's shared record, written while no gateway runs
// ---------------------------------------------------------------------------

/// Write, into the second mob's own store below the memory root of the
/// launch on `state`, a record shared by the mob's agents, with the
/// application as its author. The directory is the one the gateway made
/// for that mob by the time of the host's writes: the open here makes none.
fn write_the_second_mobs_shared_record(state: &Path) {
    let memory_root = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
        .agent_memory_root()
        .expect("the launch's memory root")
        .path;
    let directory = memory_root.join(HOSTED_MOBS_DIR).join(SECOND_MOB);
    assert!(
        directory.is_dir(),
        "the gateway made the second mob's memory directory by the host's writes: {}",
        directory.display()
    );
    let store = SqliteAgentMemoryStore::open(directory).expect("the second mob's own memory store");
    let scope = MemoryScope::Mob {
        realm: meerkat_core::mob_realm_id(SECOND_MOB)
            .expect("the second mob's realm")
            .as_str()
            .to_string(),
        mob: SECOND_MOB.to_string(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("a runtime for the store's write");
    runtime.block_on(async {
        store
            .remember_authored(
                &scope,
                NewMemoryRecord {
                    kind: MemoryKind::Fact,
                    title: SHARED_BY_THE_SECOND_MOB.to_string(),
                    description: format!("{SHARED_BY_THE_SECOND_MOB}, in short."),
                    body: format!("{SHARED_BY_THE_SECOND_MOB}, in full."),
                    tags: Vec::new(),
                    evidence: Vec::new(),
                    verification: None,
                },
                MemoryAuthor::Application,
            )
            .await
            .expect("the second mob's shared record");
    });
    drop(store);
}

/// A launch lists the profile `keeper` its own memory only. Its host
/// creates a second mob beside the launch mob through the gateway, and in
/// it an agent of that profile and an agent of the profile `worker`, which
/// the launch does not list. The host writes a record into each agent's own
/// memory through the gateway, and, with the gateway stopped, a record
/// shared by the mob's agents into the second mob's store. At its first turn
/// the model request of the listed profile's agent holds that agent's own
/// record and not the mob's shared one; the request of the other agent
/// holds its own record and the mob's shared one. The list of the launch
/// reaches the agents of a mob the host created after the start.
#[test]
fn an_agent_of_a_listed_profile_in_a_mob_created_beside_the_launch_mob_is_listed_its_own_memory_only_through_a_real_gateway()
 {
    let model = Model::start();
    let scratch = tempfile::tempdir().expect("scratch tempdir");
    let state = scratch.path().join("state");
    let host_config = scratch.path().join("host-config.toml");
    std::fs::write(&host_config, host_config_text(&model.base_url)).expect("write host config");

    // The first launch: the second mob and its two agents, by host calls.
    let mut first = Gateway::launched(scratch.path(), &state, &host_config, &[LISTED]);
    let created = first.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    assert_eq!(created["mob"]["mob_id"], json!(SECOND_MOB), "{created}");
    assert_eq!(created["mob"]["state"], json!("running"), "{created}");
    for (name, profile) in [(KEEPER, LISTED), (WORKER, UNLISTED)] {
        let agent = first.result(
            "meta/identity/create",
            json!({
                "mob_id": SECOND_MOB,
                "request_id": format!("create-{name}"),
                "agents": [{ "name": name, "profile": profile }],
            }),
        );
        assert_eq!(
            agent["outcome"],
            json!("done"),
            "the host creates the second mob's agent {name}: {agent}"
        );
    }
    // The host writes each agent's own record through the gateway. After
    // these writes the second mob's memory directory stands, made by the
    // gateway.
    first.remember_in_the_second_mob(KEEPER, OF_THE_KEEPER);
    first.remember_in_the_second_mob(WORKER, OF_THE_WORKER);
    assert_eq!(
        model.asked(),
        0,
        "no agent took a turn in the first launch: each agent's first turn follows the records"
    );
    first.shut_down("the first launch, which created the second mob and its two agents");

    write_the_second_mobs_shared_record(&state);

    // The second launch, on the same state: each agent's first turn.
    let mut second = Gateway::launched(scratch.path(), &state, &host_config, &[LISTED]);
    assert_eq!(
        second.listed_mobs(),
        vec![LAUNCH_MOB, SECOND_MOB],
        "the second launch serves the mob the first one created"
    );
    second.turn_in_the_second_mob(KEEPER, TURN_OF_THE_KEEPER);
    second.turn_in_the_second_mob(WORKER, TURN_OF_THE_WORKER);

    let of_the_worker = model.first_request_of(TURN_OF_THE_WORKER);
    assert!(
        of_the_worker.contains(OF_THE_WORKER),
        "the unlisted profile's agent is listed its own record: {of_the_worker}"
    );
    assert!(
        of_the_worker.contains(SHARED_BY_THE_SECOND_MOB),
        "the unlisted profile's agent is listed its mob's shared record: {of_the_worker}"
    );
    let of_the_keeper = model.first_request_of(TURN_OF_THE_KEEPER);
    assert!(
        of_the_keeper.contains(OF_THE_KEEPER),
        "the listed profile's agent is listed its own record: {of_the_keeper}"
    );
    assert!(
        !of_the_keeper.contains(SHARED_BY_THE_SECOND_MOB),
        "the listed profile's agent is not listed its mob's shared record: {of_the_keeper}"
    );

    second.shut_down("the second launch, in which each of the second mob's agents took a turn");
}
