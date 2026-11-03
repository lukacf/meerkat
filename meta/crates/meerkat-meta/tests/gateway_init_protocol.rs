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
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let state = tempfile::tempdir().expect("state tempdir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .current_dir(workspace.path())
            .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
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
