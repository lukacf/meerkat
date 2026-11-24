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

    /// [`Self::spawn`] of the stdio gateway binary at `binary` instead of
    /// this build's.
    fn spawn_binary(binary: &std::path::Path) -> Self {
        Self::spawn_binary_with_identity_root(binary, None)
    }

    /// `spawn`, with the comms identity root at `identity_root` instead of
    /// a directory of this gateway's own. No provider key reaches the
    /// gateway, whichever its test process holds.
    fn spawn_with_identity_root(identity_root: Option<&std::path::Path>) -> Self {
        Self::spawn_binary_with_identity_root(
            std::path::Path::new(env!("CARGO_BIN_EXE_rpc_gateway")),
            identity_root,
        )
    }

    /// [`Self::spawn_with_identity_root`] of the stdio gateway binary at
    /// `binary`.
    fn spawn_binary_with_identity_root(
        binary: &std::path::Path,
        identity_root: Option<&std::path::Path>,
    ) -> Self {
        Self::spawn_binary_logging(binary, identity_root, None)
    }

    /// [`Self::spawn_with_identity_root`], with the gateway's log filter
    /// set to `log_filter` when one is given.
    fn spawn_logging(identity_root: Option<&std::path::Path>, log_filter: Option<&str>) -> Self {
        Self::spawn_binary_logging(
            std::path::Path::new(env!("CARGO_BIN_EXE_rpc_gateway")),
            identity_root,
            log_filter,
        )
    }

    /// [`Self::spawn_binary_with_identity_root`], with the gateway's log
    /// filter (`RUST_LOG`) set to `log_filter` when one is given; else the
    /// gateway keeps its own default.
    fn spawn_binary_logging(
        binary: &std::path::Path,
        identity_root: Option<&std::path::Path>,
        log_filter: Option<&str>,
    ) -> Self {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let state = tempfile::tempdir().expect("state tempdir");
        let identity_root = identity_root.map_or_else(
            || workspace.path().join("xdg-state"),
            std::path::Path::to_path_buf,
        );
        let mut command = Command::new(binary);
        command
            .arg("--persistent")
            .current_dir(workspace.path())
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .env("XDG_STATE_HOME", identity_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(log_filter) = log_filter {
            command.env("RUST_LOG", log_filter);
        }
        let mut child = command.spawn().expect("spawn rpc_gateway");
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
// The row for a member broken after ready seats its members on a loopback
// model that holds every answer until the row releases it (`HeldModel`).

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

/// The mob id of [`READY_REPORT_MOB_CONFIG`], for its realm's binding.
const READY_REPORT_MOB_ID: &str = "gateway-ready-report-test";

/// The model [`HeldModel`] serves.
const HELD_MODEL: &str = "ready-report-held-model";

/// [`READY_REPORT_MOB_CONFIG`] with its members on [`HELD_MODEL`].
fn held_model_mob_config() -> String {
    let config = READY_REPORT_MOB_CONFIG.replace(
        "model = \"gpt-5.5\"",
        &format!("model = \"{HELD_MODEL}\"\nprovider = \"self_hosted\""),
    );
    assert_ne!(
        config, READY_REPORT_MOB_CONFIG,
        "the profile names its model"
    );
    config
}

/// The binding of `realm` to the loopback server of [`HeldModel`].
fn held_model_realm_binding(realm: &str) -> String {
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
default_model = "{HELD_MODEL}"
provider_default = true
"#
    )
}

/// A streamed answer that ends the turn.
const HELD_MODEL_REPLY: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n\
                                data: {\"choices\":[{\"finish_reason\":\"stop\"}],\
                                \"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n\n\
                                data: [DONE]\n\n";

/// A loopback chat-completions model that holds every answer until the row
/// releases it: a member woken onto it holds its session's live work, and
/// with it a place under `max_sessions`, until then.
struct HeldModel {
    /// The host config that binds the ready-report mob's realm to it.
    host_config: std::path::PathBuf,
    _dir: TempDir,
    release: tokio::sync::watch::Sender<bool>,
}

async fn held_chat_completions(
    axum::extract::State(mut released): axum::extract::State<tokio::sync::watch::Receiver<bool>>,
) -> impl axum::response::IntoResponse {
    let _ = released.wait_for(|released| *released).await;
    ([("content-type", "text/event-stream")], HELD_MODEL_REPLY)
}

async fn held_models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

impl HeldModel {
    fn start() -> Self {
        let (release, released) = tokio::sync::watch::channel(false);
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
                        axum::routing::post(held_chat_completions),
                    )
                    .route(
                        "/chat/completions",
                        axum::routing::post(held_chat_completions),
                    )
                    .route("/v1/models", axum::routing::get(held_models))
                    .route("/models", axum::routing::get(held_models))
                    .with_state(released);
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
        let dir = tempfile::tempdir().expect("held model config dir");
        let host_config = dir.path().join("config.toml");
        let mut config = format!(
            r#"
[self_hosted]
default_model = "{HELD_MODEL}"

[self_hosted.servers.loop]
base_url = "http://{address}"

[self_hosted.models."{HELD_MODEL}"]
server = "loop"
remote_model = "{HELD_MODEL}"
display_name = "Ready report held model"
family = "{HELD_MODEL}"
"#
        );
        config.push_str(&held_model_realm_binding("global"));
        config.push_str(&held_model_realm_binding(&format!(
            "mob.{READY_REPORT_MOB_ID}"
        )));
        std::fs::write(&host_config, config).expect("write host config");
        Self {
            host_config,
            _dir: dir,
            release,
        }
    }

    /// Answer every held request, and every later one at once.
    fn release(&self) {
        self.release.send(true).expect("release the model");
    }
}

/// The stdio gateway's default log filter restated, plus the mob crate at
/// DEBUG: the filter of the second boot of the row for a member broken after
/// ready. The mob crate writes a member's wake at DEBUG and its retries at
/// INFO, which the default filter drops; with them a failing run's kept
/// stderr says what became of each wake.
const WAKES_LOGGED: &str = "warn,meerkat_meta=info,rpc_gateway=info,meerkat_mob=debug";

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

/// What the status looks of a row answered, kept for a failure's text only:
/// per look the time since the row began to look, and its `broken_members`
/// answer.
#[derive(Default)]
struct StatusLooks {
    looks: Vec<(Duration, String)>,
}

impl StatusLooks {
    /// How many of the last looks a failure's text shows one by one.
    const SHOWN: usize = 20;

    /// Keep the answer `status` gave `elapsed` after the row began to look.
    fn record(&mut self, elapsed: Duration, status: &Value) {
        let answer = match status
            .get("result")
            .and_then(|result| result.get("broken_members"))
        {
            Some(broken) => broken.to_string(),
            None if status.get("error").is_some() => format!("error: {}", status["error"]),
            None => "absent".to_string(),
        };
        self.looks.push((elapsed, answer));
    }

    /// The looks in short: their count, each run of looks that gave one
    /// answer with its first and last time, and the last [`Self::SHOWN`]
    /// looks one by one.
    fn told(&self) -> String {
        let mut runs: Vec<(Duration, Duration, usize, &str)> = Vec::new();
        for (elapsed, answer) in &self.looks {
            match runs.last_mut() {
                Some((_, last, count, same)) if *same == answer.as_str() => {
                    *last = *elapsed;
                    *count += 1;
                }
                _ => runs.push((*elapsed, *elapsed, 1, answer.as_str())),
            }
        }
        let runs: Vec<String> = runs
            .iter()
            .map(|(first, last, count, answer)| {
                format!("{count} from {first:.3?} to {last:.3?} answered broken_members {answer}")
            })
            .collect();
        let last: Vec<String> = self
            .looks
            .iter()
            .skip(self.looks.len().saturating_sub(Self::SHOWN))
            .map(|(elapsed, answer)| format!("{elapsed:.3?}: {answer}"))
            .collect();
        format!(
            "{} look(s); by answer: {runs:?}; the last {}: {last:?}",
            self.looks.len(),
            last.len()
        )
    }
}

/// Per request of `requests`, what its response in `answers` accepted: the
/// completion baseline's epoch and the fencing token, or the error it
/// answered. For a failure's text only.
fn accepted_sends(requests: &[(String, &str, Value)], answers: &[Value]) -> Vec<String> {
    requests
        .iter()
        .zip(answers)
        .map(|((id, _, _), answer)| match answer.get("result") {
            Some(result) => format!(
                "{id}: epoch {} token {}",
                result["completion_baseline"]["epoch"], result["fencing_token"]
            ),
            None => format!("{id}: {}", answer["error"]),
        })
        .collect()
}

/// What one read answered, in short: its result, or its error.
fn answer_of(response: &Value) -> String {
    match response.get("result") {
        Some(result) => result.to_string(),
        None => format!("error: {}", response["error"]),
    }
}

/// Per member a request of `requests` names, what the gateway answers now
/// for its completion cursor and then for its member status: the cursors of
/// all members first (that read touches no session), the member statuses
/// after them. A read the gateway does not answer ends the reads, and the
/// text says so: the reads never replace the failure text they are part of.
/// For a failure's text only.
fn member_reads(boot: &mut ReadyReportBoot, requests: &[(String, &str, Value)]) -> String {
    let identities: Vec<&str> = requests
        .iter()
        .filter_map(|(_, _, params)| params["identity"].as_str())
        .collect();
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cursors: Vec<String> = identities
            .iter()
            .map(|identity| {
                let answer = boot.call(
                    &format!("cursor-{identity}"),
                    "meta/completion_cursor",
                    json!({ "identity": identity }),
                );
                format!("{identity}: {}", answer_of(&answer))
            })
            .collect();
        let statuses: Vec<String> = identities
            .iter()
            .map(|identity| {
                let answer = boot.call(
                    &format!("member-status-{identity}"),
                    "meta/member_status",
                    json!({ "member_id": identity }),
                );
                format!("{identity}: {}", answer_of(&answer))
            })
            .collect();
        format!("completion cursors {cursors:?}; member statuses {statuses:?}")
    }));
    read.unwrap_or_else(|_| "unknown (the gateway did not answer a read)".to_string())
}

/// What a failing row keeps of its run, for its failure text only: what
/// each send was accepted with, where the gateway's whole stderr was written
/// (in the row's own directory, which is kept for reading after such a
/// failure; that file can name members and sessions and is not for sharing
/// as it is), what the gateway answers for each member's completion cursor
/// and member status, and what the status looks answered. The stderr is
/// written before the member reads are made, so the kept log ends at the
/// failure.
fn kept_of_the_run(
    home: &mut ReadyReportHome,
    boot: &mut ReadyReportBoot,
    requests: &[(String, &str, Value)],
    answers: &[Value],
    looks: &StatusLooks,
) -> String {
    let stderr = boot.stderr_kept_in(home.kept());
    let members = member_reads(boot, requests);
    format!(
        "accepted sends: {:?}\nmember reads: {members}\nstatus looks: {}\ngateway stderr: {stderr}",
        accepted_sends(requests, answers),
        looks.told()
    )
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

    /// This home's directory, kept after the row ends: a failing row's
    /// text names a file in it.
    fn kept(&mut self) -> &std::path::Path {
        self.root.disable_cleanup(true);
        self.root.path()
    }

    /// Boot a gateway over this home with a roster of `members` identities
    /// and `runtime_options`, through its ready settlement. With
    /// `customizer` the SDK side also answers the agent customizer, and
    /// counts the member builds it is asked for.
    fn boot(
        &self,
        init_id: &str,
        members: usize,
        runtime_options: Value,
        customizer: bool,
    ) -> ReadyReportBoot {
        self.boot_logging(init_id, members, runtime_options, customizer, None, None)
    }

    /// [`Self::boot`], with the gateway's log filter set to `log_filter`
    /// when one is given, and the members on `model` instead of the demo
    /// model when one is given.
    fn boot_logging(
        &self,
        init_id: &str,
        members: usize,
        mut runtime_options: Value,
        customizer: bool,
        log_filter: Option<&str>,
        model: Option<&HeldModel>,
    ) -> ReadyReportBoot {
        let state = self.root.path().join("state");
        std::fs::create_dir_all(&state).expect("state directory");
        let gateway = Gateway::spawn_logging(Some(&self.root.path().join("xdg-state")), log_filter);
        let mob_config = match model {
            Some(model) => {
                runtime_options["meerkat_config_path"] = json!(model.host_config);
                held_model_mob_config()
            }
            None => {
                runtime_options["demo_llm"] = json!(true);
                READY_REPORT_MOB_CONFIG.to_string()
            }
        };
        let mut params = json!({
            "init_protocol": "accepted_then_settled",
            "init_id": init_id,
            "mob_config": mob_config,
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

    /// Write every stderr line the gateway wrote so far to a file in
    /// `directory` and say where: the line count and the path, or why the
    /// file could not be written. For a failure's text only.
    fn stderr_kept_in(&self, directory: &std::path::Path) -> String {
        let lines = self
            .gateway
            .stderr_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let path = directory.join("gateway-stderr.log");
        match std::fs::write(&path, lines.join("\n")) {
            Ok(()) => format!("{} line(s) in {}", lines.len(), path.display()),
            Err(error) => format!(
                "{} line(s), not written to {}: {error}",
                lines.len(),
                path.display()
            ),
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
///
/// The members answer from a model that holds every answer until the row
/// releases it, so the two members woken first hold their sessions' live
/// work, and the limit's two places, while the third is woken: the limit is
/// met whatever the order and the pace of the wakes on a loaded host.
#[test]
fn a_member_broken_after_ready_is_named_by_status_and_not_by_the_ready_report() {
    let model = HeldModel::start();
    let mut home = ReadyReportHome::new();
    home.boot_logging(
        "init-rr3-seat",
        3,
        json!({ "max_sessions": 2 }),
        false,
        None,
        Some(&model),
    )
    .stop();

    // This boot logs each member's wake (see `WAKES_LOGGED`): what a failing
    // run keeps of the gateway's stderr then names the wakes.
    let mut lazy = home.boot_logging(
        "init-rr3-lazy",
        3,
        json!({ "max_sessions": 2 }),
        false,
        Some(WAKES_LOGGED),
        Some(&model),
    );
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
    // What each status look answered: kept for a failure's text only.
    let mut looks = StatusLooks::default();
    let broken = loop {
        let status = lazy.call("status-after", "meta/status", json!({}));
        looks.record(start.elapsed(), &status);
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
                 gateway log lines naming the limit: {limit_lines:?}\n{}",
                kept_of_the_run(&mut home, &mut lazy, &requests, &deliveries, &looks)
            );
        }
        thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        broken.len(),
        1,
        "{broken:?}\n{}",
        kept_of_the_run(&mut home, &mut lazy, &requests, &deliveries, &looks)
    );
    for (member, entry) in &broken {
        assert!(
            entry["error"]
                .as_str()
                .is_some_and(|error| error.contains(SESSION_LIMIT_REASON)),
            "{member}: {entry}\n{}",
            kept_of_the_run(&mut home, &mut lazy, &requests, &deliveries, &looks)
        );
    }
    assert!(
        lazy.settled.get("broken_members").is_none(),
        "the settlement is not rewritten"
    );
    // The held turns end before the gateway is stopped.
    model.release();
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

// What a launch that names no mob shows from outside: the members of its
// init result, the files it leaves in a fresh state directory, the code and
// text of an init refusal at each stage, the order of listener bind,
// directory claim and first store open, and the shutdown answer with the
// release of the directory. A change to how the runtime is composed inside
// must leave every one of them as it is.

/// Every file and directory under `root`, as a path relative to it;
/// directories end in `/`. SQLite's `-wal` and `-shm` companions are left
/// out: whether one exists at a given moment is the SQLite library's choice.
fn tree(root: &std::path::Path) -> std::collections::BTreeSet<String> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut std::collections::BTreeSet<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.insert(format!("{relative}/"));
                walk(root, &path, out);
            } else if !relative.ends_with("-wal") && !relative.ends_with("-shm") {
                out.insert(relative);
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(root, root, &mut out);
    out
}

/// Ask a served gateway to shut down, check the answer is the completed
/// shutdown of a stock persistent gateway, and reap it.
fn shut_down(gateway: &mut Gateway) {
    gateway.send(json!({ "jsonrpc": "2.0", "id": "bye", "method": "meta/shutdown", "params": {} }));
    let tail = gateway.read_until("the shutdown answer", |_| None, |m| m["id"] == "bye");
    let answer = tail.last().unwrap();
    gateway_shutdown::assert_persistent_shutdown_completed(answer, "a stock persistent gateway");
    let mut members: Vec<&str> = answer["result"]
        .as_object()
        .expect("a shutdown result object")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        ["runtime_cleanup_completed", "shutdown"],
        "{answer}"
    );
    let status = gateway.wait_exit();
    assert!(status.success(), "gateway exit status: {status}");
}

/// The init result of a fresh persistent launch has exactly these members,
/// and the ones a launch decides alone have these values.
#[test]
fn a_fresh_persistent_init_result_has_exactly_these_members() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({}));
    let response = gateway.next("the legacy init response");
    assert_eq!(response["id"], "init");
    let result = response["result"].as_object().expect("an init result");
    let mut members: Vec<&str> = result.keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
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
        ],
        "{response}"
    );
    let result = &response["result"];
    assert!(
        result["http_base_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://127.0.0.1:")),
        "{response}"
    );
    assert!(result["contract_version"].is_string(), "{response}");
    assert!(
        result["runtime_fingerprint"]
            .as_str()
            .is_some_and(|fingerprint| fingerprint.len() == 64
                && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "{response}"
    );
    assert!(
        result["stdio_shutdown_horizon_ms"]
            .as_u64()
            .is_some_and(|horizon| horizon > 0),
        "{response}"
    );
    let decided_alone = json!({
        "control_listen_address": result["control_listen_address"],
        "http_public_base_url": result["http_public_base_url"],
        "identity_bootstrap": result["identity_bootstrap"],
        "loaded_modules": result["loaded_modules"],
        "member_restore": result["member_restore"],
        "runtime_origin": result["runtime_origin"],
        "stdio_shutdown_handshake": result["stdio_shutdown_handshake"],
    });
    assert_eq!(
        decided_alone,
        json!({
            "control_listen_address": null,
            "http_public_base_url": null,
            "identity_bootstrap": null,
            "loaded_modules": [],
            "member_restore": { "boot_set": "observed" },
            "runtime_origin": "workspace_config",
            "stdio_shutdown_handshake": true,
        }),
        "{response}"
    );
    shut_down(&mut gateway);
}

/// A fresh persistent launch that is served and then shut down leaves
/// exactly these files in its state directory.
#[test]
fn a_fresh_persistent_launch_leaves_exactly_these_files() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({}));
    let response = gateway.next("the legacy init response");
    assert!(
        response["result"]["http_base_url"].is_string(),
        "{response}"
    );
    let served = tree(gateway.state.path());
    shut_down(&mut gateway);
    let left = tree(gateway.state.path());
    let expected: std::collections::BTreeSet<String> = [
        "blobs/",
        "council.sqlite3",
        "council.sqlite3.mfence",
        "event_log.sqlite3",
        "event_log.sqlite3.composition.json",
        "event_log.sqlite3.incarnation-custody",
        "event_log.sqlite3.mfence",
        "meta-runtime.lock",
        "meta/",
        "meta/delivery/",
        "meta/delivery/cold-delivery.lock",
        "meta/jobs.sqlite3",
        "meta/jobs.sqlite3.mfence",
        "meta_console.sqlite3",
        "meta_console.sqlite3.mfence",
        "meta_metadata.sqlite3",
        "meta_metadata.sqlite3.mfence",
        "mobs/",
        "mobs/child-mobs.lock",
        "mobs/realm_profiles.db",
        "mobs/realm_profiles.db.mfence",
        "peer_key.ed25519",
        "runtime.sqlite",
        "runtime.sqlite.execution-custody",
        "runtime.sqlite.execution-custody-admission",
        "runtime.sqlite.mfence",
        "runtime.sqlite.mpresence",
        "schedule.sqlite",
        "schedule.sqlite.mfence",
        "workgraph.sqlite3",
        "workgraph.sqlite3.mfence",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(served, expected, "while served");
    assert_eq!(left, expected, "after shutdown");
}

/// The answer to a refused `meta/init`: the settled failure of an opted-in
/// init, or the plain error response of one refused before acceptance.
fn init_refusal(gateway: &mut Gateway) -> (i64, String) {
    let first = gateway.next("the init answer");
    let refusal = if first.get("error").is_some() {
        first["error"].clone()
    } else {
        assert_eq!(first["result"]["init_state"], "accepted", "{first}");
        let seen = gateway.read_until("the init settlement", |_| None, is_settled);
        let settled = seen.last().unwrap()["params"].clone();
        assert_eq!(settled["outcome"], "failed", "{settled}");
        settled
    };
    (
        refusal["code"].as_i64().expect("a refusal code"),
        refusal["message"]
            .as_str()
            .expect("a refusal message")
            .to_string(),
    )
}

/// Each stage of init refuses with its own code and text: the option
/// parse, the mob definition, the listener bind, the directory claim and
/// the check of the launch mob's store.
#[test]
fn init_refusals_keep_their_code_and_text_at_each_stage() {
    let opted_in = || json!({ "init_protocol": "accepted_then_settled", "init_id": "init-guard" });

    // The option parse.
    let mut gateway = Gateway::spawn();
    let mut params = opted_in();
    params["runtime_options"] = json!({ "no_such_option": true, "nor_this_one": 1 });
    gateway.init(params);
    assert_eq!(
        init_refusal(&mut gateway),
        (
            -32602,
            "unsupported runtime_options fields: no_such_option, nor_this_one".to_string()
        )
    );
    assert_eq!(gateway.wait_exit().code(), Some(1));

    // The mob definition.
    let mut gateway = Gateway::spawn();
    let state = gateway.state.path().to_path_buf();
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled", "init_id": "init-guard",
            "mob_config": "this is [not valid toml",
            "persistent_state": state,
        },
    }));
    let (code, message) = init_refusal(&mut gateway);
    assert_eq!(code, -32602, "{message}");
    assert!(
        message.starts_with("Invalid mob_config TOML: "),
        "{message}"
    );
    assert_eq!(gateway.wait_exit().code(), Some(1));
    assert_eq!(tree(&state), Default::default(), "nothing was created");

    // The listener bind.
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a loopback port");
    let taken = holder.local_addr().expect("held address");
    let mut gateway = Gateway::spawn();
    let mut params = opted_in();
    params["runtime_options"] = json!({ "http_listen": taken.to_string() });
    gateway.init(params);
    let (code, message) = init_refusal(&mut gateway);
    assert_eq!(code, -32603, "{message}");
    assert!(
        message.starts_with(&format!("failed to bind HTTP listener {taken}: ")),
        "{message}"
    );
    assert_eq!(gateway.wait_exit().code(), Some(1));
    drop(holder);

    // The directory claim.
    let mut gateway = Gateway::spawn();
    let state = gateway.state.path().to_path_buf();
    let claim = meerkat_meta::state_dir_claim::StateDirClaim::acquire(&state).expect("claim");
    gateway.init(opted_in());
    assert_eq!(
        init_refusal(&mut gateway),
        (
            -32014,
            format!(
                "state directory {} is in use by another Meta runtime",
                state.display()
            )
        )
    );
    assert_eq!(gateway.wait_exit().code(), Some(1));
    drop(claim);

    // The launch mob's store: one this build cannot read refuses the launch
    // at the storage check, after the claim and before any store is created.
    let mut gateway = Gateway::spawn();
    let state = gateway.state.path().to_path_buf();
    let store = state.join("event_log.sqlite3");
    std::fs::write(&store, b"not a database").expect("plant a store");
    gateway.init(opted_in());
    assert_eq!(
        init_refusal(&mut gateway),
        (
            -32014,
            format!(
                "cannot check state directory {} before startup ({}): {} is not a mob store \
                 this build reads: the file is not a SQLite database",
                state.display(),
                store.display(),
                store.display()
            )
        )
    );
    assert_eq!(gateway.wait_exit().code(), Some(1));
    let left: Vec<String> = tree(&state).into_iter().collect();
    assert_eq!(left, ["event_log.sqlite3", "meta-runtime.lock"]);
}

/// Init binds its listener, then claims the state directory, then opens
/// its first store: a taken port refuses a launch on a held directory as a
/// bind failure, and a held directory refuses a launch before it creates
/// anything there.
#[test]
fn init_binds_its_listener_then_claims_the_directory_then_opens_stores() {
    let opted_in = || json!({ "init_protocol": "accepted_then_settled", "init_id": "init-order" });
    let state = tempfile::tempdir().expect("state tempdir");
    let claim = meerkat_meta::state_dir_claim::StateDirClaim::acquire(state.path()).expect("claim");
    let held = tree(state.path());
    assert_eq!(held.len(), 1, "the claim's own file: {held:?}");

    // Bind before claim.
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("hold a loopback port");
    let taken = holder.local_addr().expect("held address");
    let mut gateway = Gateway::spawn();
    let mut params = opted_in();
    params["runtime_options"] = json!({ "http_listen": taken.to_string() });
    gateway.init_at(params, state.path());
    let (code, message) = init_refusal(&mut gateway);
    assert_eq!(code, -32603, "the bind refuses first: {message}");
    assert_eq!(gateway.wait_exit().code(), Some(1));
    drop(holder);
    assert_eq!(tree(state.path()), held);

    // Claim before the first store open.
    let mut gateway = Gateway::spawn();
    gateway.init_at(opted_in(), state.path());
    let (code, message) = init_refusal(&mut gateway);
    assert_eq!(code, -32014, "{message}");
    assert_eq!(gateway.wait_exit().code(), Some(1));
    assert_eq!(
        tree(state.path()),
        held,
        "a refused claim created nothing in the directory"
    );
    drop(claim);
}

/// A served gateway holds its state directory; a clean shutdown answers
/// with the completed shutdown and nothing else, and once the gateway has
/// exited the directory is free.
#[test]
fn a_clean_shutdown_answers_completed_and_frees_the_state_directory() {
    let mut gateway = Gateway::spawn();
    gateway.init(json!({}));
    let response = gateway.next("the legacy init response");
    assert!(
        response["result"]["http_base_url"].is_string(),
        "{response}"
    );
    let state = gateway.state.path().to_path_buf();
    let refused = meerkat_meta::state_dir_claim::StateDirClaim::acquire(&state)
        .expect_err("a served gateway holds its state directory");
    assert_eq!(
        refused.to_string(),
        format!(
            "state directory {} is in use by another Meta runtime",
            state.display()
        )
    );
    shut_down(&mut gateway);
    meerkat_meta::state_dir_claim::StateDirClaim::acquire(&state)
        .expect("the directory is free after the gateway exits");
}

// Two more launch shapes, pinned from outside before the launch mob is
// built by the mob set like any other mob: a launch that brings no mob
// definition, and an identity-first launch over a fresh state directory and
// again over the state it left.

/// A launch that brings no mob definition serves the built-in mob: the same
/// init result members as a launch with a definition, the origin that says
/// no definition was given, a running mob, and the launch mob's store at its
/// fixed place.
#[test]
fn a_launch_without_a_mob_definition_serves_the_built_in_mob() {
    let mut gateway = Gateway::spawn();
    let state = gateway.state.path().to_path_buf();
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": { "persistent_state": state },
    }));
    let response = gateway.next("the legacy init response");
    assert_eq!(response["id"], "init");
    let result = response["result"].as_object().expect("an init result");
    let mut members: Vec<&str> = result.keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(
        members,
        [
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
        ],
        "{response}"
    );
    assert_eq!(
        response["result"]["runtime_origin"], "fallback_minimal",
        "{response}"
    );
    assert_eq!(
        response["result"]["identity_bootstrap"],
        Value::Null,
        "{response}"
    );

    gateway
        .send(json!({ "jsonrpc": "2.0", "id": "status", "method": "meta/status", "params": {} }));
    let seen = gateway.read_until("the status answer", |_| None, |m| m["id"] == "status");
    let status = seen.last().unwrap();
    assert_eq!(status["result"]["mob_state"], "Some(Running)", "{status}");

    let served = tree(&state);
    assert!(served.contains("event_log.sqlite3"), "{served:?}");
    assert!(served.contains("mobs/"), "{served:?}");
    shut_down(&mut gateway);
    assert_eq!(
        tree(&state),
        served,
        "a clean shutdown leaves the same files"
    );
}

/// The mob of the identity-first launch row: one profile whose members can
/// message their peers, which a member of a mob needs.
const IDENTITY_FIRST_MOB_CONFIG: &str = r#"
[mob]
id = "gateway-init-protocol-identity-first"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
"#;

/// Launch an identity-first gateway over `state` with one identity on the
/// default profile, until its init settles. Returns the settlement.
fn identity_first_init(gateway: &mut Gateway, state: &std::path::Path, init_id: &str) -> Value {
    identity_first_init_with(
        gateway,
        state,
        init_id,
        json!([{ "identity": "keeper", "profile": "default" }]),
    )
}

/// [`identity_first_init`] with the roster its roster provider answers.
fn identity_first_init_with(
    gateway: &mut Gateway,
    state: &std::path::Path,
    init_id: &str,
    roster: Value,
) -> Value {
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled",
            "init_id": init_id,
            "mob_config": IDENTITY_FIRST_MOB_CONFIG,
            "persistent_state": state,
            "has_roster_provider": true,
            "runtime_options": { "demo_llm": true },
        },
    }));
    let accepted = gateway.next("the init acceptance");
    assert_eq!(accepted["result"]["init_state"], "accepted", "{accepted}");
    let seen = gateway.read_until(
        "the identity-first init settlement",
        |method| (method == "callback/roster_provider/roster").then(|| roster.clone()),
        is_settled,
    );
    seen.last().unwrap().clone()
}

/// The roster of the rollback row: its one identity runs turn by turn, so
/// that a send to it is a turn the gateway reports the end of.
fn turn_driven_roster() -> Value {
    json!([{
        "identity": "keeper", "profile": "default", "runtime_mode_override": "turn_driven"
    }])
}

/// Ask a served gateway to shut down and reap it, whatever its shutdown
/// answer carries beside the completed shutdown.
fn shut_down_served(gateway: &mut Gateway) {
    gateway.send(json!({ "jsonrpc": "2.0", "id": "bye", "method": "meta/shutdown", "params": {} }));
    let tail = gateway.read_until("the shutdown answer", |_| None, |m| m["id"] == "bye");
    let answer = tail.last().unwrap();
    assert_eq!(answer["result"]["shutdown"], true, "{answer}");
    let status = gateway.wait_exit();
    assert!(status.success(), "gateway exit status: {status}");
}

/// The identity bootstrap an identity-first init settled with: complete,
/// ready, the one identity of the roster reported and none broken.
fn assert_identity_bootstrap_ready(settled: &Value) {
    assert_eq!(settled["params"]["outcome"], "ready", "{settled}");
    let bootstrap = &settled["params"]["identity_bootstrap"];
    assert!(
        bootstrap.is_object(),
        "an identity-first launch reports its identity bootstrap: {settled}"
    );
    assert_eq!(bootstrap["complete"], true, "{settled}");
    assert_eq!(bootstrap["ready"], true, "{settled}");
    assert_eq!(bootstrap["counts"]["broken"], 0, "{settled}");
    assert!(bootstrap["identities"]["keeper"].is_object(), "{settled}");
}

/// An identity-first launch settles ready with its identity bootstrap
/// reported, over a fresh state directory and again over the state that
/// launch left: the second launch finds the launch mob stopped by the first
/// one's shutdown and runs it again.
#[test]
fn an_identity_first_launch_settles_ready_fresh_and_again_over_its_own_state() {
    let mut first = Gateway::spawn();
    let state = first.state.path().to_path_buf();
    let settled = identity_first_init(&mut first, &state, "identity-first-fresh");
    assert_identity_bootstrap_ready(&settled);
    assert_eq!(
        settled["params"]["runtime_origin"], "workspace_config",
        "{settled}"
    );
    shut_down_served(&mut first);

    let mut second = Gateway::spawn();
    let settled = identity_first_init(&mut second, &state, "identity-first-again");
    assert_identity_bootstrap_ready(&settled);
    second.send(json!({ "jsonrpc": "2.0", "id": "status", "method": "meta/status", "params": {} }));
    let seen = second.read_until(
        "the status answer",
        |method| {
            (method == "callback/roster_provider/roster")
                .then(|| json!([{ "identity": "keeper", "profile": "default" }]))
        },
        |m| m["id"] == "status",
    );
    let status = seen.last().unwrap();
    assert_eq!(status["result"]["mob_state"], "Some(Running)", "{status}");
    shut_down_served(&mut second);
}

/// An identity-first init over `state` that lists the profiles `names`
/// their own memory only, read to its settlement.
fn init_listing_own_memory_only(
    gateway: &mut Gateway,
    state: &std::path::Path,
    init_id: &str,
    names: Value,
) -> Value {
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled",
            "init_id": init_id,
            "mob_config": IDENTITY_FIRST_MOB_CONFIG,
            "persistent_state": state,
            "has_roster_provider": true,
            "runtime_options": {
                "demo_llm": true,
                "agent_memory": { "own_memory_only_profiles": names },
            },
        },
    }));
    let accepted = gateway.next("the init acceptance");
    assert_eq!(accepted["result"]["init_state"], "accepted", "{accepted}");
    let seen = gateway.read_until(
        "the init settlement",
        |method| {
            (method == "callback/roster_provider/roster")
                .then(|| json!([{ "identity": "keeper", "profile": "default" }]))
        },
        is_settled,
    );
    seen.last().unwrap().clone()
}

/// The profiles a launch lists their own memory only are profiles of its
/// launch mob. An init that lists a name the launch mob's definition does
/// not have is refused with the option's code and a text that names the
/// option, the name and the mob's profiles, and the gateway exits; an init
/// that lists a profile the definition has settles ready.
#[test]
fn an_init_that_lists_a_profile_its_own_memory_only_is_checked_against_the_launch_mob() {
    let mut refused = Gateway::spawn();
    let state = refused.state.path().to_path_buf();
    let settled = init_listing_own_memory_only(
        &mut refused,
        &state,
        "own-memory-only-unknown",
        json!(["visitor"]),
    );
    assert_eq!(settled["params"]["outcome"], "failed", "{settled}");
    assert_eq!(settled["params"]["code"], -32602, "{settled}");
    let message = settled["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(
            "runtime_options.agent_memory.own_memory_only_profiles names \"visitor\", which is \
             no profile of the mob"
        ) && message.ends_with("(its profiles: default)"),
        "{message}"
    );
    assert_eq!(refused.wait_exit().code(), Some(1));

    let mut taken = Gateway::spawn();
    let state = taken.state.path().to_path_buf();
    let settled = init_listing_own_memory_only(
        &mut taken,
        &state,
        "own-memory-only-listed",
        json!(["default"]),
    );
    assert_identity_bootstrap_ready(&settled);
    shut_down_served(&mut taken);
}

/// A launch store that passes the check of the state directory before
/// startup and cannot then be opened refuses init with the storage code and
/// this text, which names the store. The store here is the one a first
/// launch left, made read-only: the check reads it, and opening it for the
/// launch's mob has to write. The row runs in every ordinary run. As root
/// the permission does not bind and the store opens all the same, so there
/// the row says that it was not exercised and asserts nothing.
#[cfg(unix)]
#[test]
fn a_launch_store_that_cannot_be_opened_refuses_init_with_its_code_and_text() {
    use std::os::unix::fs::PermissionsExt;
    let mut first = Gateway::spawn();
    first.init(json!({}));
    let response = first.next("the legacy init response");
    assert!(
        response["result"]["http_base_url"].is_string(),
        "{response}"
    );
    let state = first.state.path().to_path_buf();
    shut_down(&mut first);
    let store = state.join("event_log.sqlite3");
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o444))
        .expect("a read-only store");
    if std::fs::OpenOptions::new().write(true).open(&store).is_ok() {
        eprintln!("NOT EXERCISED: running as root");
        return;
    }

    let mut second = Gateway::spawn();
    second.init_at(
        json!({ "init_protocol": "accepted_then_settled", "init_id": "init-guard" }),
        &state,
    );
    assert_eq!(
        init_refusal(&mut second),
        (
            -32014,
            format!(
                "failed to open the persistent mob storage at {}: storage error: Internal \
                 error: attempt to write a readonly database (adopted identity declarations \
                 and mob events would not survive gateway restart)",
                store.display()
            )
        )
    );
    assert_eq!(second.wait_exit().code(), Some(1));
}

/// The environment variable that names the stdio gateway binary of the
/// published release a state directory is taken back to.
const ROLLBACK_GATEWAY: &str = "META_ROLLBACK_RPC_GATEWAY";

/// One tracked turn of the identity `keeper` on a served identity-first
/// gateway, sent and waited for to its end.
fn run_one_turn(gateway: &mut Gateway) {
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "send", "method": "meta/send",
        "params": { "identity": "keeper", "content": "hello", "track_turn": true },
    }));
    let seen = gateway.read_until(
        "the answer to the send",
        |method| (method == "callback/roster_provider/roster").then(turn_driven_roster),
        |m| m["id"] == "send",
    );
    let sent = seen.last().unwrap();
    let ticket = sent["result"]["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("a tracked send answers its turn's ticket: {sent}"))
        .to_string();
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "wait", "method": "meta/wait_for_turn",
        "params": { "identity": "keeper", "ticket": ticket, "timeout_ms": 60_000 },
    }));
    let seen = gateway.read_until(
        "the end of the turn",
        |method| (method == "callback/roster_provider/roster").then(turn_driven_roster),
        |m| m["id"] == "wait",
    );
    let waited = seen.last().unwrap();
    assert!(waited.get("error").is_none(), "{waited}");
}

/// The way back to the published release. A state directory this gateway
/// wrote, with one identity served and one turn run to its end, is launched
/// again by the stdio gateway binary of the published release, named by
/// `META_ROLLBACK_RPC_GATEWAY`: its init settles ready with the identity
/// bootstrap complete, and its launch mob runs. The published binary is
/// built from its release tag outside this workspace, so the row is ignored
/// by default and reads as skipped; run it with the variable set and
/// `--run-ignored only`. Run without the variable it fails: it never passes
/// without having run the published binary.
#[test]
#[ignore = "needs META_ROLLBACK_RPC_GATEWAY: the path of the published release's rpc_gateway"]
fn a_published_gateway_serves_a_state_directory_this_gateway_wrote() {
    let published = std::env::var_os(ROLLBACK_GATEWAY).unwrap_or_else(|| {
        panic!(
            "{ROLLBACK_GATEWAY} is not set (the path of the published release's rpc_gateway \
             binary)"
        )
    });
    let mut first = Gateway::spawn();
    let state = first.state.path().to_path_buf();
    let settled =
        identity_first_init_with(&mut first, &state, "rollback-written", turn_driven_roster());
    assert_identity_bootstrap_ready(&settled);
    run_one_turn(&mut first);
    shut_down_served(&mut first);

    let mut second = Gateway::spawn_binary(std::path::Path::new(&published));
    let settled = identity_first_init_with(
        &mut second,
        &state,
        "rollback-published",
        turn_driven_roster(),
    );
    assert_identity_bootstrap_ready(&settled);
    second.send(json!({ "jsonrpc": "2.0", "id": "status", "method": "meta/status", "params": {} }));
    let seen = second.read_until(
        "the status answer of the published gateway",
        |method| (method == "callback/roster_provider/roster").then(turn_driven_roster),
        |m| m["id"] == "status",
    );
    let status = seen.last().unwrap();
    assert_eq!(status["result"]["mob_state"], "Some(Running)", "{status}");
    shut_down_served(&mut second);
}

/// Remove from the launch store under `state` the events that carry the
/// mob's definition, leaving a store that holds events and no definition.
fn remove_the_definition_events(state: &std::path::Path) {
    let store = rusqlite::Connection::open(state.join("event_log.sqlite3"))
        .expect("the store the first launch left");
    let kind = "json_extract(CAST(event_json AS TEXT), '$.event.kind.type')";
    let removed = store
        .execute(
            &format!(
                "DELETE FROM mob_events WHERE {kind} IN ('mob_created', 'mob_definition_updated')"
            ),
            [],
        )
        .expect("remove the events that carry the definition");
    assert!(removed >= 1, "the store held the mob's creation");
    let left: u32 = store
        .query_row("SELECT COUNT(*) FROM mob_events", [], |row| row.get(0))
        .expect("count the events left");
    assert!(left >= 1, "the store still holds events");
}

/// A launch store that holds events and no definition of the launch's mob
/// refuses init with the internal error code and this text, the text of
/// every release: the launch judges the resume from its own open of the
/// store before its mob set exists. The store here is the one a first
/// launch left, with the events that carry the mob's definition removed.
#[test]
fn a_launch_store_with_events_and_no_definition_refuses_init_with_its_code_and_text() {
    let mut first = Gateway::spawn();
    first.init(json!({}));
    let response = first.next("the legacy init response");
    assert!(
        response["result"]["http_base_url"].is_string(),
        "{response}"
    );
    let state = first.state.path().to_path_buf();
    shut_down(&mut first);
    remove_the_definition_events(&state);

    let mut second = Gateway::spawn();
    second.init_at(
        json!({ "init_protocol": "accepted_then_settled", "init_id": "init-guard" }),
        &state,
    );
    assert_eq!(
        init_refusal(&mut second),
        (
            -32603,
            "Runtime bootstrap failed: failed to bootstrap mob runtime: internal error: non-empty \
             mob storage has no canonical definition"
                .to_string()
        )
    );
    assert_eq!(second.wait_exit().code(), Some(1));
}

/// The same store under a launch with a roster provider, which judges the
/// resume from its own open before the bootstrap, where the launch above
/// judges at the start of the bootstrap: the same code and the same text,
/// and the roster provider is never asked.
#[test]
fn a_roster_launch_over_a_store_with_events_and_no_definition_refuses_init_with_its_code_and_text()
{
    let mut first = Gateway::spawn();
    let state = first.state.path().to_path_buf();
    let settled = identity_first_init(&mut first, &state, "roster-fresh");
    assert_identity_bootstrap_ready(&settled);
    shut_down_served(&mut first);
    remove_the_definition_events(&state);

    let mut second = Gateway::spawn();
    second.send(json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled",
            "init_id": "roster-guard",
            "mob_config": IDENTITY_FIRST_MOB_CONFIG,
            "persistent_state": state,
            "has_roster_provider": true,
            "runtime_options": { "demo_llm": true },
        },
    }));
    assert_eq!(
        init_refusal(&mut second),
        (
            -32603,
            "Runtime bootstrap failed: failed to bootstrap mob runtime: internal error: non-empty \
             mob storage has no canonical definition"
                .to_string()
        )
    );
    assert_eq!(second.wait_exit().code(), Some(1));
}
