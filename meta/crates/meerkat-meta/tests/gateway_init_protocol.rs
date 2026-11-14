//! `meta/init`, accepted then settled (#550), against the REAL rpc_gateway.
//!
//! - An SDK that does not opt in gets exactly the legacy single response and
//!   never sees `accepted`, progress or a settlement.
//! - An opted-in SDK gets `accepted` (echoing its `init_id`) before any
//!   startup work, ordered `meta/init_progress` phases, and one `ready`
//!   `meta/init_settled` carrying the init result.
//! - A refusal before the first durable phase settles `failed` with
//!   `durable_effects: "none"`; one after it settles `failed` with
//!   `"possible"`; a refusal before acceptance stays the plain error response.
//! - A `meta/shutdown` sent while init is still running settles the init
//!   `failed` (reason `shutdown_requested`) only after cleanup, and is then
//!   answered.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;
use tempfile::TempDir;

/// Hang detector, not a latency bound (see `gateway_exit_reason`).
const BACKSTOP: Duration = Duration::from_secs(90);

const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-init-protocol-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true
"#;

struct Gateway {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_lines: mpsc::Receiver<String>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    _workspace: TempDir,
    state: TempDir,
}

impl Gateway {
    fn spawn() -> Self {
        Self::spawn_with_identity_root(None)
    }

    /// `spawn`, with the comms identity root at `identity_root` instead of
    /// a directory of this gateway's own. No provider key reaches the
    /// gateway, whichever its test process holds.
    fn spawn_with_identity_root(identity_root: Option<&std::path::Path>) -> Self {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let state = tempfile::tempdir().expect("state tempdir");
        let identity_root = identity_root.map_or_else(
            || workspace.path().join("xdg-state"),
            std::path::Path::to_path_buf,
        );
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .current_dir(workspace.path())
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .env("XDG_STATE_HOME", identity_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr_lines);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                sink.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(line);
            }
        });
        let (tx, stdout_lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            stdout_lines,
            stderr_lines,
            _workspace: workspace,
            state,
        }
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{value}").expect("write");
        stdin.flush().expect("flush");
    }

    fn init(&mut self, params: Value) {
        let state = self.state.path().to_path_buf();
        self.init_at(params, &state);
    }

    /// `init` against `state` instead of this gateway's own state directory.
    fn init_at(&mut self, mut params: Value, state: &std::path::Path) {
        params["mob_config"] = json!(MOB_CONFIG);
        params["persistent_state"] = json!(state);
        self.send(json!({
            "jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": params,
        }));
    }

    /// Next stdout message within the backstop, or a panic naming the wait.
    fn next(&mut self, waiting_for: &str) -> Value {
        let start = Instant::now();
        loop {
            let remaining = BACKSTOP.saturating_sub(start.elapsed());
            let line = self
                .stdout_lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| {
                    panic!(
                        "no stdout line while waiting for {waiting_for}\nstderr:\n{}",
                        self.stderr_lines.lock().unwrap().join("\n")
                    )
                });
            if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                return message;
            }
        }
    }

    /// Read messages until `done` accepts one, answering provider callbacks
    /// with `answer` (or a method-not-found error) on the way. Returns every
    /// message seen, the accepted one last.
    fn read_until(
        &mut self,
        waiting_for: &str,
        mut answer: impl FnMut(&str) -> Option<Value>,
        mut done: impl FnMut(&Value) -> bool,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let message = self.next(waiting_for);
            if let (Some(method), Some(id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let reply = match answer(method) {
                    Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    None => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("no handler for {method}") },
                    }),
                };
                self.send(reply);
            }
            let finished = done(&message);
            seen.push(message);
            if finished {
                return seen;
            }
        }
    }

    /// Close stdin, then wait for and reap the gateway, returning its actual
    /// exit status for the caller to judge.
    fn wait_exit(&mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        let start = Instant::now();
        while start.elapsed() < BACKSTOP {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("gateway did not exit");
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_settled(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("meta/init_settled")
}

fn progress_phases(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.get("method").and_then(Value::as_str) == Some("meta/init_progress"))
        .map(|m| {
            m["params"]["phase"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[test]
fn legacy_init_gets_exactly_the_single_response() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({}));
    let response = gateway.next("the legacy init response");
    assert_eq!(response["id"], "init");
    let result = &response["result"];
    assert!(result["http_base_url"].is_string(), "{response}");
    assert!(
        result.get("init_state").is_none(),
        "legacy SDK saw accepted: {response}"
    );
    assert_eq!(result["stdio_shutdown_handshake"], true);
    // Nothing init-protocol-shaped follows: shut down and check the tail.
    gateway.send(json!({ "jsonrpc": "2.0", "id": "bye", "method": "meta/shutdown", "params": {} }));
    let tail = gateway.read_until("the shutdown answer", |_| None, |m| m["id"] == "bye");
    assert!(
        tail.iter().all(|m| !is_settled(m)
            && m.get("method").and_then(Value::as_str) != Some("meta/init_progress")),
        "{tail:?}"
    );
}

#[test]
fn opted_in_init_is_accepted_then_progresses_then_settles_ready() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({ "init_protocol": "accepted_then_settled", "init_id": "init-t1" }));
    let accepted = gateway.next("the accepted response");
    assert_eq!(accepted["id"], "init");
    assert_eq!(accepted["result"]["init_state"], "accepted", "{accepted}");
    assert_eq!(accepted["result"]["init_id"], "init-t1");
    assert_eq!(accepted["result"]["provider_callback_timeout_ms"], 130_000);
    assert_eq!(accepted["result"]["stdio_shutdown_handshake"], true);
    assert!(
        accepted["result"].get("http_base_url").is_none(),
        "accepted is not readiness"
    );

    let seen = gateway.read_until("the ready settlement", |_| None, is_settled);
    let settled = seen.last().unwrap();
    assert_eq!(settled["params"]["init_id"], "init-t1");
    assert_eq!(settled["params"]["outcome"], "ready", "{settled}");
    assert!(settled["params"]["http_base_url"].is_string());
    assert!(
        settled.get("id").is_none(),
        "a settlement is a notification"
    );
    let phases = progress_phases(&seen);
    for expected in [
        "storage",
        "owner_publication",
        "prepare",
        "schedules",
        "serve",
    ] {
        assert!(
            phases.contains(&expected.to_string()),
            "missing {expected} in {phases:?}"
        );
    }
    assert!(
        seen.iter()
            .all(|m| m.get("id").and_then(Value::as_str) != Some("init")),
        "the init request is answered once"
    );
}

// -- The ready report names the members the start left broken --------------
//
// Each row boots real gateways over one state directory and one comms
// identity root, as the restarts of one installation do. The first boot of a
// row seats its roster under the default restore, which builds no member, so
// the session limit is not reached; a later boot restores the members as its
// row says. The members answer from the demo model: no provider key is set.

/// The mob of the ready-report rows: turn-driven members a roster seats.
const READY_REPORT_MOB_CONFIG: &str = r#"
[mob]
id = "gateway-ready-report-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.default.tools]
comms = true
"#;

/// Core's refusal of a session past `runtime_options.max_sessions = 2`.
const SESSION_LIMIT_REASON: &str = "Max sessions reached (2/2)";

/// The gateway's ready-time warning about the members its mob holds broken.
const READY_LOG_MESSAGE: &str = "the mob's start left members broken";

/// The most members that warning names; it counts the rest.
const READY_LOG_NAMED_MEMBERS: usize = 8;

/// The keys of the init result of a start that left no member broken.
const HEALTHY_INIT_RESULT_KEYS: [&str; 11] = [
    "contract_version",
    "control_listen_address",
    "http_base_url",
    "http_public_base_url",
    "identity_bootstrap",
    "loaded_modules",
    "member_restore",
    "runtime_fingerprint",
    "runtime_origin",
    "stdio_shutdown_handshake",
    "stdio_shutdown_horizon_ms",
];

fn roster_identities(members: usize) -> Vec<String> {
    (1..=members).map(|index| format!("m{index:02}")).collect()
}

fn object_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The `broken_members` entries of an init result or a `meta/status`
/// result, by member id. Empty when the field is absent.
fn broken_members(report: &Value) -> std::collections::BTreeMap<String, Value> {
    report
        .get("broken_members")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let member = entry["member_id"].as_str().unwrap_or_else(|| {
                        panic!("a broken member entry names its member: {entry}")
                    });
                    (member.to_string(), entry.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The directories the boots of one ready-report row share.
struct ReadyReportHome {
    root: TempDir,
}

impl ReadyReportHome {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("ready-report home"),
        }
    }

    /// Boot a gateway over this home with a roster of `members` identities
    /// and `runtime_options`, through its ready settlement. With
    /// `customizer` the SDK side also answers the agent customizer, and
    /// counts the member builds it is asked for.
    fn boot(
        &self,
        init_id: &str,
        members: usize,
        mut runtime_options: Value,
        customizer: bool,
    ) -> ReadyReportBoot {
        let state = self.root.path().join("state");
        std::fs::create_dir_all(&state).expect("state directory");
        let gateway = Gateway::spawn_with_identity_root(Some(&self.root.path().join("xdg-state")));
        runtime_options["demo_llm"] = json!(true);
        let mut params = json!({
            "init_protocol": "accepted_then_settled",
            "init_id": init_id,
            "mob_config": READY_REPORT_MOB_CONFIG,
            "persistent_state": state,
            "has_roster_provider": true,
            "runtime_options": runtime_options,
        });
        if customizer {
            params["has_agent_customizer"] = json!(true);
        }
        let roster = roster_identities(members)
            .into_iter()
            .map(|identity| {
                json!({
                    "identity": identity,
                    "profile": "default",
                    "addressability": "addressable",
                    "display_name": null,
                    "labels": {},
                    "context": null,
                    "additional_instructions": [],
                })
            })
            .collect();
        let mut boot = ReadyReportBoot {
            gateway,
            roster: Value::Array(roster),
            customizer_builds: 0,
            settled: Value::Null,
        };
        boot.gateway.send(json!({
            "jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": params,
        }));
        let seen = boot.pump("the ready settlement", is_settled);
        boot.settled = seen.last().expect("a settlement")["params"].clone();
        assert_eq!(
            boot.settled["outcome"], "ready",
            "{init_id}: {}",
            boot.settled
        );
        boot
    }
}

/// One boot of a ready-report row, past its ready settlement.
struct ReadyReportBoot {
    gateway: Gateway,
    /// What the roster callback answers.
    roster: Value,
    /// The member builds the agent customizer was asked for so far.
    customizer_builds: usize,
    /// The `meta/init_settled` params.
    settled: Value,
}

impl ReadyReportBoot {
    /// Read messages until `done` accepts one, answering the roster and the
    /// agent customizer callbacks on the way. Returns every message seen,
    /// the accepted one last.
    fn pump(&mut self, waiting_for: &str, mut done: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let message = self.gateway.next(waiting_for);
            if let (Some(method), Some(id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let reply = match method {
                    "callback/roster_provider/roster" => {
                        json!({ "jsonrpc": "2.0", "id": id, "result": self.roster })
                    }
                    "callback/agent_customizer/customize_build" => {
                        self.customizer_builds += 1;
                        json!({ "jsonrpc": "2.0", "id": id, "result": message["params"]["draft"] })
                    }
                    "callback/agent_customizer/after_create" => {
                        json!({ "jsonrpc": "2.0", "id": id, "result": null })
                    }
                    other => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32601, "message": format!("no handler for {other}") },
                    }),
                };
                self.gateway.send(reply);
            }
            let finished = done(&message);
            seen.push(message);
            if finished {
                return seen;
            }
        }
    }

    /// Send a request and return its response.
    fn call(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.gateway.send(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        let mut seen = self.pump(method, |message| {
            message.get("method").is_none() && message["id"] == id
        });
        seen.pop().expect("the response")
    }

    /// Send every `(id, method, params)` request at once and return the
    /// responses in request order.
    fn call_all(&mut self, requests: &[(String, &str, Value)]) -> Vec<Value> {
        for (id, method, params) in requests {
            self.gateway.send(json!({
                "jsonrpc": "2.0", "id": id, "method": method, "params": params,
            }));
        }
        let mut pending: std::collections::BTreeSet<&str> =
            requests.iter().map(|(id, _, _)| id.as_str()).collect();
        let seen = self.pump("the responses of the requests sent together", |message| {
            if message.get("method").is_none()
                && let Some(id) = message["id"].as_str()
            {
                pending.remove(id);
            }
            pending.is_empty()
        });
        requests
            .iter()
            .map(|(id, _, _)| {
                seen.iter()
                    .find(|message| message.get("method").is_none() && message["id"] == id.as_str())
                    .cloned()
                    .expect("the response")
            })
            .collect()
    }

    /// The members `meta/list_members` reads broken.
    fn members_listed_broken(&mut self) -> std::collections::BTreeSet<String> {
        let listed = self.call("list-members", "meta/list_members", json!({}));
        listed["result"]
            .as_array()
            .unwrap_or_else(|| panic!("a member list: {listed}"))
            .iter()
            .filter(|member| member["state"] == "broken")
            .map(|member| {
                member["agent_identity"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a listed member names itself: {member}"))
                    .to_string()
            })
            .collect()
    }

    /// The gateway's stderr lines that carry `needle`, waiting a while for
    /// the first: the gateway writes its log before the line it answers with,
    /// and this process reads the two pipes on separate threads.
    fn stderr_lines_with(&self, needle: &str) -> Vec<String> {
        let start = Instant::now();
        loop {
            let lines: Vec<String> = self
                .gateway
                .stderr_lines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter(|line| line.contains(needle))
                .cloned()
                .collect();
            if !lines.is_empty() || start.elapsed() > Duration::from_secs(10) {
                return lines;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Shut the gateway down and require an ordinary exit.
    fn stop(mut self) {
        let answer = self.call("bye", "meta/shutdown", json!({}));
        assert_eq!(answer["result"]["shutdown"], true, "{answer}");
        let status = self.gateway.wait_exit();
        assert!(status.success(), "gateway exit status: {status}");
    }
}

/// A start that restores more members than `max_sessions` admits is ready,
/// and its ready report names exactly the members Core holds broken, each
/// with Core's reason: the same entries `meta/status` answers with. The
/// session limit's refusal carries no machine code, so the entries carry
/// none. The gateway also warns once, with the count and the names.
#[test]
fn ready_report_names_exactly_the_members_the_start_left_broken() {
    let home = ReadyReportHome::new();
    let seated = home.boot("init-rr1-seat", 3, json!({ "max_sessions": 2 }), false);
    assert!(
        seated.settled.get("broken_members").is_none(),
        "a start that builds no member leaves none broken: {}",
        seated.settled
    );
    seated.stop();

    let mut eager = home.boot(
        "init-rr1-eager",
        3,
        json!({ "member_restore": "eager", "max_sessions": 2 }),
        false,
    );
    let listed_broken = eager.members_listed_broken();
    assert_eq!(
        listed_broken.len(),
        1,
        "two sessions for three restored members leave one broken: {listed_broken:?}"
    );
    assert!(
        eager.settled.get("broken_members").is_some(),
        "the ready report names the members the start left broken: {}",
        eager.settled
    );
    let named = broken_members(&eager.settled);
    assert_eq!(
        named
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        listed_broken,
        "{}",
        eager.settled
    );
    for (member, entry) in &named {
        assert_eq!(
            object_keys(entry),
            ["error", "member_id"],
            "{member}: a session limit refusal has no machine code: {entry}"
        );
        assert!(
            entry["error"]
                .as_str()
                .is_some_and(|error| error.contains(SESSION_LIMIT_REASON)),
            "{member}: {entry}"
        );
    }

    let status = eager.call("status", "meta/status", json!({}));
    assert_eq!(
        status["result"]["broken_members"], eager.settled["broken_members"],
        "meta/status names the same members: {status}"
    );

    let warnings = eager.stderr_lines_with(READY_LOG_MESSAGE);
    assert_eq!(warnings.len(), 1, "one ready-time warning: {warnings:?}");
    assert!(
        warnings[0].contains("broken_member_count=1"),
        "{warnings:?}"
    );
    for member in named.keys() {
        assert!(warnings[0].contains(member.as_str()), "{warnings:?}");
    }
    eager.stop();
}

/// A start that left no member broken writes the init result it wrote
/// before a ready report could name one: the same keys with no
/// `broken_members`, as the legacy response and as the ready settlement,
/// with or without restored members.
#[test]
fn ready_report_of_a_healthy_start_is_unchanged() {
    let mut legacy = Gateway::spawn();
    legacy.init(json!({}));
    let response = legacy.next("the legacy init response");
    assert_eq!(
        object_keys(&response["result"]),
        HEALTHY_INIT_RESULT_KEYS,
        "{response}"
    );

    let home = ReadyReportHome::new();
    home.boot("init-rr2-seat", 3, json!({}), false).stop();
    let eager = home.boot(
        "init-rr2-eager",
        3,
        json!({ "member_restore": "eager" }),
        false,
    );
    let mut settled_keys: Vec<&str> = HEALTHY_INIT_RESULT_KEYS.to_vec();
    settled_keys.extend(["init_id", "outcome"]);
    settled_keys.sort_unstable();
    assert_eq!(
        object_keys(&eager.settled),
        settled_keys,
        "{}",
        eager.settled
    );
    assert_eq!(
        eager.settled["identity_bootstrap"]["counts"]["broken"], 0,
        "{}",
        eager.settled
    );
    assert_eq!(
        eager.settled["member_restore"],
        json!({ "boot_set": "observed" })
    );
    assert!(
        eager.stderr_lines_with(READY_LOG_MESSAGE).is_empty(),
        "a healthy start warns of no broken member"
    );
    eager.stop();
}

/// The ready report is a snapshot of the moment the gateway became ready. A
/// member that breaks later (here: its first wake finds no session left) is
/// named by `meta/status`, which reads the mob when it is asked; the
/// settlement written at ready named none.
#[test]
fn a_member_broken_after_ready_is_named_by_status_and_not_by_the_ready_report() {
    let home = ReadyReportHome::new();
    home.boot("init-rr3-seat", 3, json!({ "max_sessions": 2 }), false)
        .stop();

    let mut lazy = home.boot("init-rr3-lazy", 3, json!({ "max_sessions": 2 }), false);
    assert!(
        lazy.settled.get("broken_members").is_none(),
        "the default restore builds no member, so none is broken at ready: {}",
        lazy.settled
    );
    let before = lazy.call("status-before", "meta/status", json!({}));
    assert!(before["result"].get("broken_members").is_none(), "{before}");

    // Three deliveries sent together wake three members at once, and the
    // limit admits two sessions. What a delivery itself answers is not this
    // row's subject: the answers are kept for the failure text only.
    let requests: Vec<(String, &str, Value)> = roster_identities(3)
        .into_iter()
        .map(|identity| {
            (
                format!("send-{identity}"),
                "meta/send",
                json!({ "identity": identity, "content": "wake" }),
            )
        })
        .collect();
    let deliveries = lazy.call_all(&requests);
    let start = Instant::now();
    let broken = loop {
        let status = lazy.call("status-after", "meta/status", json!({}));
        let broken = broken_members(&status["result"]);
        if !broken.is_empty() {
            break broken;
        }
        if start.elapsed() >= BACKSTOP {
            let listed = lazy.call("list-after", "meta/list_members", json!({}));
            let limit_lines = lazy.stderr_lines_with(SESSION_LIMIT_REASON);
            panic!(
                "no member read broken after its wake met the session limit\n\
                 deliveries: {deliveries:?}\nmembers: {listed}\n\
                 gateway log lines naming the limit: {limit_lines:?}"
            );
        }
        thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(broken.len(), 1, "{broken:?}");
    for (member, entry) in &broken {
        assert!(
            entry["error"]
                .as_str()
                .is_some_and(|error| error.contains(SESSION_LIMIT_REASON)),
            "{member}: {entry}"
        );
    }
    assert!(
        lazy.settled.get("broken_members").is_none(),
        "the settlement is not rewritten"
    );
    lazy.stop();
}

/// The ready-time warning names at most `READY_LOG_NAMED_MEMBERS` members
/// and counts the rest; the ready report itself lists every one.
#[test]
fn ready_log_line_names_a_bounded_number_of_members_and_counts_the_rest() {
    let members = READY_LOG_NAMED_MEMBERS + 3;
    let home = ReadyReportHome::new();
    home.boot(
        "init-rr4-seat",
        members,
        json!({ "max_sessions": 2 }),
        false,
    )
    .stop();

    let eager = home.boot(
        "init-rr4-eager",
        members,
        json!({ "member_restore": "eager", "max_sessions": 2 }),
        false,
    );
    let named = broken_members(&eager.settled);
    assert_eq!(
        named.len(),
        members - 2,
        "the ready report lists every broken member: {}",
        eager.settled
    );
    let warnings = eager.stderr_lines_with(READY_LOG_MESSAGE);
    assert_eq!(warnings.len(), 1, "one ready-time warning: {warnings:?}");
    let line = &warnings[0];
    assert!(
        line.contains(&format!("broken_member_count={}", named.len())),
        "{line}"
    );
    let named_in_line = named
        .keys()
        .filter(|member| line.contains(member.as_str()))
        .count();
    assert_eq!(named_in_line, READY_LOG_NAMED_MEMBERS, "{line}");
    assert!(line.contains("and 1 more"), "{line}");
    eager.stop();
}

/// Reading the broken members builds no member: a restart under the default
/// restore asks the agent customizer for no member build before ready, and
/// for none when `meta/status` reads the same list.
#[test]
fn the_ready_report_and_the_status_read_build_no_member() {
    let home = ReadyReportHome::new();
    home.boot("init-rr5-seat", 3, json!({}), true).stop();

    let mut lazy = home.boot("init-rr5-lazy", 3, json!({}), true);
    assert_eq!(
        lazy.customizer_builds, 0,
        "the default restore builds no member before ready"
    );
    assert!(
        lazy.settled.get("broken_members").is_none(),
        "{}",
        lazy.settled
    );
    let status = lazy.call("status", "meta/status", json!({}));
    assert!(status["result"].get("broken_members").is_none(), "{status}");
    assert_eq!(
        lazy.customizer_builds, 0,
        "reading the broken members built a member"
    );
    lazy.stop();
}

#[test]
fn refusal_before_the_first_durable_phase_settles_failed_with_no_durable_effects() {
    let mut gateway = Gateway::spawn();
    // A top-level `http_listen` is refused after acceptance and before any
    // store opens.
    gateway.init(json!({
        "init_protocol": "accepted_then_settled", "init_id": "init-t2",
        "http_listen": "127.0.0.1:0",
    }));
    assert_eq!(gateway.next("accepted")["result"]["init_state"], "accepted");
    let seen = gateway.read_until("the failed settlement", |_| None, is_settled);
    let settled = &seen.last().unwrap()["params"];
    assert_eq!(settled["outcome"], "failed", "{settled}");
    assert_eq!(settled["code"], -32602);
    assert_eq!(settled["durable_effects"], "none");
    assert!(
        progress_phases(&seen).is_empty(),
        "no phase began: {seen:?}"
    );
    // A refused init is a failed gateway run: exit status 1.
    let status = gateway.wait_exit();
    assert_eq!(status.code(), Some(1), "refused init exit status: {status}");
}

#[test]
fn refusal_after_storage_began_settles_failed_with_possible_durable_effects() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({
        "init_protocol": "accepted_then_settled", "init_id": "init-t3",
        "has_roster_provider": true,
    }));
    assert_eq!(gateway.next("accepted")["result"]["init_state"], "accepted");
    // The identity roster callback fails at prepare, before the mob is
    // built and after storage ran.
    let seen = gateway.read_until("the failed settlement", |_| None, is_settled);
    let settled = &seen.last().unwrap()["params"];
    assert_eq!(settled["outcome"], "failed", "{settled}");
    assert_eq!(settled["durable_effects"], "possible");
    let phases = progress_phases(&seen);
    assert!(
        phases.contains(&"storage".to_string()) && phases.contains(&"prepare".to_string()),
        "{phases:?}"
    );
    // A refused init is a failed gateway run: exit status 1.
    let status = gateway.wait_exit();
    assert_eq!(status.code(), Some(1), "refused init exit status: {status}");
}

/// 0.9.4 removed SDK-hosted continuity. Each of its init params is refused by
/// name, alone or together and whatever its value, with INVALID_PARAMS that
/// names the removal and the migration guide, before any store opens: a host
/// built for the removed path never starts on a different storage shape.
#[test]
fn removed_sdk_hosted_continuity_init_params_are_refused_by_name() {
    let scratch = tempfile::tempdir().expect("scratch tempdir");
    let cases = [
        (
            json!({ "has_continuity_store": true }),
            vec!["has_continuity_store"],
        ),
        (
            json!({ "has_lease_provider": true }),
            vec!["has_lease_provider"],
        ),
        (
            json!({ "scratch_dir": scratch.path() }),
            vec!["scratch_dir"],
        ),
        (
            json!({ "has_continuity_store": false }),
            vec!["has_continuity_store"],
        ),
        (
            json!({
                "has_roster_provider": true,
                "has_continuity_store": true,
                "has_lease_provider": true,
                "scratch_dir": scratch.path(),
            }),
            vec!["has_continuity_store", "has_lease_provider", "scratch_dir"],
        ),
    ];
    for (mut params, named) in cases {
        params["init_protocol"] = json!("accepted_then_settled");
        params["init_id"] = json!("init-removed-continuity");
        let mut gateway = Gateway::spawn();
        gateway.init(params.clone());
        assert_eq!(gateway.next("accepted")["result"]["init_state"], "accepted");
        let seen = gateway.read_until("the failed settlement", |_| None, is_settled);
        let settled = &seen.last().unwrap()["params"];
        assert_eq!(settled["outcome"], "failed", "{params}: {settled}");
        assert_eq!(settled["code"], -32602, "{params}: {settled}");
        assert_eq!(settled["durable_effects"], "none", "{params}: {settled}");
        let message = settled["message"].as_str().unwrap_or_default();
        for key in named {
            assert!(message.contains(key), "{params}: {message}");
        }
        assert!(
            message.contains("removed in Meta 0.9.4"),
            "{params}: {message}"
        );
        assert!(
            message.contains("meta_gateway storage-migrate-native"),
            "{params}: {message}"
        );
        assert!(
            progress_phases(&seen).is_empty(),
            "no phase began: {seen:?}"
        );
        let status = gateway.wait_exit();
        assert_eq!(status.code(), Some(1), "refused init exit status: {status}");
    }
}

/// One Meta runtime per state directory: a second gateway on a directory a
/// running gateway holds refuses init typed (STORAGE_RESOLUTION, -32014)
/// before it opens any store, and the directory is free again once the
/// first gateway has exited.
#[test]
fn a_second_gateway_on_a_held_state_directory_is_refused_typed() {
    let ready = |gateway: &mut Gateway, waiting_for: &str| {
        assert_eq!(gateway.next("accepted")["result"]["init_state"], "accepted");
        let seen = gateway.read_until(waiting_for, |_| None, is_settled);
        let settled = seen.last().unwrap()["params"].clone();
        assert_eq!(settled["outcome"], "ready", "{settled}");
    };
    let mut first = Gateway::spawn();
    first.init(json!({ "init_protocol": "accepted_then_settled", "init_id": "init-first" }));
    ready(&mut first, "the first gateway's ready settlement");
    let state = first.state.path().to_path_buf();

    let mut second = Gateway::spawn();
    second.init_at(
        json!({ "init_protocol": "accepted_then_settled", "init_id": "init-second" }),
        &state,
    );
    assert_eq!(second.next("accepted")["result"]["init_state"], "accepted");
    let seen = second.read_until("the second gateway's settlement", |_| None, is_settled);
    let settled = &seen.last().unwrap()["params"];
    assert_eq!(settled["outcome"], "failed", "{settled}");
    assert_eq!(settled["code"], -32014, "{settled}");
    let message = settled["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&format!(
            "state directory {} is in use by another Meta runtime",
            state.display()
        )),
        "{message}"
    );
    let status = second.wait_exit();
    assert_eq!(status.code(), Some(1), "refused init exit status: {status}");

    // The first gateway's exit releases the directory.
    first.send(json!({ "jsonrpc": "2.0", "id": "bye", "method": "meta/shutdown", "params": {} }));
    first.read_until("the shutdown answer", |_| None, |m| m["id"] == "bye");
    let status = first.wait_exit();
    assert!(status.success(), "first gateway exit status: {status}");

    let mut third = Gateway::spawn();
    third.init_at(
        json!({ "init_protocol": "accepted_then_settled", "init_id": "init-third" }),
        &state,
    );
    ready(&mut third, "the third gateway's ready settlement");
}

#[test]
fn refusal_before_acceptance_stays_the_plain_error_response() {
    let mut gateway = Gateway::spawn();
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled", "init_id": "init-t4",
            "mob_config": "this is [not valid toml",
        },
    }));
    let response = gateway.next("the init refusal");
    assert_eq!(response["id"], "init");
    assert_eq!(response["error"]["code"], -32602, "{response}");
    // A refused init is a failed gateway run: exit status 1.
    let status = gateway.wait_exit();
    assert_eq!(status.code(), Some(1), "refused init exit status: {status}");
}

#[test]
fn shutdown_during_init_settles_failed_after_cleanup_then_is_answered() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({
        "init_protocol": "accepted_then_settled", "init_id": "init-t5",
        "has_roster_provider": true,
    }));
    assert_eq!(gateway.next("accepted")["result"]["init_state"], "accepted");
    // With persistent state the gateway runs two roster callbacks: owner
    // publication (before prepare) and the boot roster (at prepare, before
    // the mob build; the identity bootstrap takes that roster). Answer the
    // first, hold the second, ask for shutdown, then answer it with an empty
    // roster: init builds the mob and stops at the next gateway boundary,
    // where a runtime exists to shut down.
    let mut seen = Vec::new();
    let mut roster_calls = 0;
    let pending_roster = loop {
        let message = gateway.next("the identity roster callback");
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = message.get("id").cloned();
        seen.push(message);
        match (method.as_deref(), id) {
            (Some("callback/roster_provider/roster"), Some(id)) => {
                roster_calls += 1;
                if roster_calls == 2 {
                    break id;
                }
                gateway.send(json!({ "jsonrpc": "2.0", "id": id, "result": [] }));
            }
            (Some(method), Some(id)) => {
                gateway.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": method } }));
            }
            _ => {}
        }
    };
    let phases = progress_phases(&seen);
    assert!(
        phases.contains(&"prepare".to_string()),
        "held the post-prepare roster: {phases:?}"
    );
    gateway
        .send(json!({ "jsonrpc": "2.0", "id": "stop", "method": "meta/shutdown", "params": {} }));
    gateway.send(json!({ "jsonrpc": "2.0", "id": pending_roster, "result": [] }));
    let rest = gateway.read_until("the shutdown answer", |_| None, |m| m["id"] == "stop");
    seen.extend(rest);
    let settled_at = seen
        .iter()
        .position(is_settled)
        .expect("a settlement before the shutdown answer");
    let answered_at = seen.iter().position(|m| m["id"] == "stop").unwrap();
    assert!(
        settled_at < answered_at,
        "settled only after cleanup, before the answer"
    );
    let settled = &seen[settled_at]["params"];
    assert_eq!(settled["outcome"], "failed", "{settled}");
    assert_eq!(settled["data"]["reason"], "shutdown_requested");
    assert_eq!(settled["durable_effects"], "possible");
    let phases = progress_phases(&seen);
    // The mob was built before the next gateway boundary (the identity
    // bootstrap's start) saw the shutdown and ran cleanup.
    let roster = phases
        .iter()
        .position(|p| p == "roster")
        .expect("roster phase");
    let cleanup = phases
        .iter()
        .position(|p| p == "cleanup")
        .expect("cleanup phase");
    assert!(roster < cleanup, "{phases:?}");
    // The runtime was built; its durable child root is released with every
    // other phase settled.
    gateway_shutdown::assert_persistent_shutdown_completed(
        &seen[answered_at],
        "shutdown answered after init cleanup",
    );
    // A shutdown requested during init is an ordinary stop: exit 0.
    let status = gateway.wait_exit();
    assert!(
        status.success(),
        "shutdown-during-init exit status: {status}"
    );
}

#[test]
fn ordinary_request_during_init_is_refused_at_once_and_served_after_ready() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({
        "init_protocol": "accepted_then_settled", "init_id": "init-t6",
        "has_roster_provider": true,
    }));
    assert_eq!(gateway.next("accepted")["result"]["init_state"], "accepted");
    // While the identity roster callback is held (init cannot progress), an
    // ordinary request - what a reentrant provider callback would send - is
    // answered at once with the typed refusal instead of queueing behind init.
    let mut roster_calls = 0;
    let held = loop {
        let message = gateway.next("the identity roster callback");
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = message.get("id").cloned();
        match (method.as_deref(), id) {
            (Some("callback/roster_provider/roster"), Some(id)) => {
                roster_calls += 1;
                if roster_calls == 2 {
                    break id;
                }
                gateway.send(json!({ "jsonrpc": "2.0", "id": id, "result": [] }));
            }
            (Some(method), Some(id)) => gateway.send(
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": method } }),
            ),
            _ => {}
        }
    };
    gateway.send(
        json!({ "jsonrpc": "2.0", "id": "reentrant", "method": "meta/status", "params": {} }),
    );
    let refusal = gateway.next("the init-in-progress refusal");
    assert_eq!(
        refusal["id"], "reentrant",
        "answered before the held callback: {refusal}"
    );
    assert_eq!(
        refusal["error"]["code"],
        meerkat_meta::INIT_IN_PROGRESS_CODE
    );
    assert_eq!(refusal["error"]["data"]["kind"], "init_in_progress");

    gateway.send(json!({ "jsonrpc": "2.0", "id": held, "result": [] }));
    let seen = gateway.read_until("the ready settlement", |_| None, is_settled);
    assert_eq!(seen.last().unwrap()["params"]["outcome"], "ready");
    gateway.send(json!({ "jsonrpc": "2.0", "id": "after", "method": "meta/status", "params": {} }));
    let served = gateway.read_until("the status answer", |_| None, |m| m["id"] == "after");
    let status = served.last().unwrap();
    assert!(
        status.get("result").is_some(),
        "served after init settled: {status}"
    );
}
