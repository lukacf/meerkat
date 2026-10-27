//! Regression: the rpc_gateway stdin dispatch loop must serve RPC requests
//! CONCURRENTLY. A turn- or build-running RPC can block on a host callback
//! round-trip (`callback/build_agent`, `callback/call_tool`), and the host
//! may issue further RPCs from inside that callback (a downstream app issued
//! `meta/agent_memory/recall` from a callback tool handler). With the old
//! sequential loop those reentrant requests queued behind the blocked RPC
//! until the callback timed out.
//!
//! The test drives the real binary: it triggers a spawn whose build round-
//! trips `callback/build_agent`, withholds the callback response, and
//! asserts an interleaved `meta/status` still answers.
//!
//! The recall-specific regression goes further: it parks the build of ONE
//! identity on its unanswered callback and asserts
//! `meta/agent_memory/recall` for that SAME identity still answers through
//! the full runtime path (identity status read + memory provider). This is
//! byte-for-byte the downstream app's re-entrancy shape - the host calling back into
//! the gateway from inside `callback/build_agent` - and it must never queue
//! behind the parked build.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

/// Backstop ceiling for every "this either arrives or the gateway is wedged"
/// wait in this file. Each such wait guards a STRUCTURAL property - the
/// dispatch loop either answers the request or it is blocked behind the
/// pending callback, which is the regression under test. The ceiling is only a
/// hang detector, never a latency measurement, so it is deliberately generous:
/// the old mixed 15s/30s ceilings measured runner load instead, and flaked the
/// suite under full parallel CI while passing standalone.
const WEDGE_BACKSTOP: Duration = Duration::from_mins(1);

const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-concurrent-dispatch-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
"#;

/// Same mob, but the profile DECLARES workgraph. Used by the
/// declared-vs-resolved regression (activation-41): a declared category must
/// be present on the member's resolved tool surface.
const MOB_CONFIG_WORKGRAPH: &str = r#"
[mob]
id = "gateway-concurrent-dispatch-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
workgraph = true
"#;

struct Gateway {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Gateway {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (tx, rx) = mpsc::channel();
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
            lines: rx,
        }
    }

    /// Like [`Self::start`], but pipes stderr into a shared buffer so a test
    /// can assert on the gateway's own tracing output. [`Self::start`]
    /// discards it, which is right for every other test here.
    fn start_capturing_stderr(filter: &str) -> (Self, Arc<Mutex<Vec<String>>>) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .env("RUST_LOG", filter)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let stderr = child.stderr.take().expect("gateway stderr");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                sink.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(line);
            }
        });
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        (
            Self {
                child,
                stdin: Some(stdin),
                lines: rx,
            },
            captured,
        )
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        writeln!(
            stdin,
            "{}",
            serde_json::to_string(&value).expect("request json")
        )
        .expect("write request");
        stdin.flush().expect("flush request");
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    /// Wait for and reap the gateway, returning its actual exit status for
    /// the caller to judge.
    fn wait_for_exit(&mut self, deadline: Duration) -> std::process::ExitStatus {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                return status;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        panic!("rpc_gateway did not exit within {deadline:?}");
    }

    /// Read messages until `predicate` matches one; unmatched messages are
    /// handed to `on_other` (e.g. to auto-acknowledge unrelated callbacks).
    fn wait_for(
        &mut self,
        deadline: Duration,
        mut on_other: impl FnMut(&mut Self, &Value),
        predicate: impl Fn(&Value) -> bool,
    ) -> Option<Value> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            let remaining = deadline.saturating_sub(start.elapsed());
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                return None;
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if predicate(&message) {
                return Some(message);
            }
            on_other(self, &message);
        }
        None
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_response_with_id(message: &Value, id: &str) -> bool {
    message.get("method").is_none() && message.get("id").and_then(Value::as_str) == Some(id)
}

fn is_callback_request(message: &Value, method: &str) -> bool {
    message.get("method").and_then(Value::as_str) == Some(method) && message.get("id").is_some()
}

fn initialize_for_shutdown(gateway: &mut Gateway, state_dir: &tempfile::TempDir) {
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |_, _| {},
            |m| is_response_with_id(m, "init"),
        )
        .expect("init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}"
    );
    assert_eq!(init["result"]["stdio_shutdown_handshake"], true);
    assert_eq!(init["result"]["stdio_shutdown_horizon_ms"], 432_000);
}

#[test]
fn persistent_gateway_exits_after_stdin_eof() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    initialize_for_shutdown(&mut gateway, &state_dir);

    gateway.close_stdin();
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
}

#[cfg(unix)]
#[test]
fn persistent_gateway_exits_after_sigint_with_stdin_open() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    initialize_for_shutdown(&mut gateway, &state_dir);

    let status = Command::new("kill")
        .args(["-INT", &gateway.child.id().to_string()])
        .status()
        .expect("send SIGINT to rpc_gateway");
    assert!(status.success(), "kill -INT failed: {status}");
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
}

#[test]
fn rpc_dispatch_serves_requests_while_a_callback_is_pending() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "has_session_builder": true
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |_, _| {},
            |m| is_response_with_id(m, "init"),
        )
        .expect("init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}"
    );

    // Trigger a member build; with has_session_builder the gateway round-trips
    // callback/build_agent and the spawn RPC blocks until we answer it.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "spawn",
        "method": "meta/spawn_member",
        "params": { "profile": "default", "meerkat_id": "worker-1" }
    }));
    let build_callback = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |_, _| {},
            |m| is_callback_request(m, "callback/build_agent"),
        )
        .expect("callback/build_agent request");
    let callback_id = build_callback["id"].clone();

    // The regression: with the callback UNANSWERED (the spawn handler is
    // parked), an interleaved request must still be served. The sequential
    // loop queued it behind the spawn until the callback timed out.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "status",
        "method": "meta/status",
        "params": {}
    }));
    let status = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |_, _| {},
            |m| is_response_with_id(m, "status"),
        )
        .expect("meta/status must answer while callback/build_agent is pending");
    assert!(
        status["result"]["contract_version"].is_string(),
        "status failed: {status}"
    );

    // Unblock the build and let the spawn finish (success or a build error —
    // either proves the handler was parked on the callback, not lost).
    // Auto-acknowledge any further callbacks (e.g. callback/after_create).
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": callback_id,
        "result": {}
    }));
    let spawn = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                if message.get("method").is_some()
                    && let Some(id) = message.get("id").cloned()
                {
                    gateway.send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                }
            },
            |m| is_response_with_id(m, "spawn"),
        )
        .expect("spawn response after callback reply");
    assert!(
        spawn.get("result").is_some() || spawn.get("error").is_some(),
        "spawn response malformed: {spawn}"
    );
}

/// Answer the roster callback with `agent:alpha` and acknowledge every other
/// provider callback with `null`, EXCEPT `callback/build_agent`, which the tests
/// answer (or deliberately withhold) themselves.
fn answer_roster_callback_holding_builds(gateway: &mut Gateway, message: &Value) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    if method == "callback/build_agent" {
        return;
    }
    let Some(id) = message.get("id").cloned() else {
        return;
    };
    let result = match method {
        "callback/roster_provider/roster" => json!([{
            "identity": "agent:alpha",
            "profile": "default",
            "addressability": "addressable",
            "display_name": null,
            "labels": {},
            "context": null,
            "additional_instructions": []
        }]),
        _ => Value::Null,
    };
    gateway.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

/// Declared-vs-resolved capability invariant, CREATE half (activation-41).
/// A callback-built member whose profile declares `tools.workgraph = true`
/// must expose `workgraph_*` on its resolved tool surface when the member is
/// CREATED (the create path threads profile.tools -> category overrides;
/// meerkat-mob build.rs:253). The RESUME half is the open upstream defect:
/// apply_resumed_session_metadata overwrites tool-category overrides from
/// creation-era metadata.tooling with no resume-override mask (build.rs:472,
/// profile.rs:260) - a resumed member created before the flag flip never
/// resolves the category. This test pins the half that works so the upstream
/// merge has a green baseline, and `meta/identity/resolved_tools` is the
/// instrument both fleets now gate on.
#[test]
fn callback_built_member_resolves_declared_workgraph_tools_on_create() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG_WORKGRAPH,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init"),
        )
        .expect("identity-first init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "identity-first init failed: {init}"
    );

    // Trigger the callback-built materialization (fresh member: CREATE path)
    // and answer the build callback immediately.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "dispatch",
        "method": "meta/dispatch",
        "params": {
            "identity": "agent:alpha",
            "dispatch_input": { "content": "hello", "origin": "connector" }
        }
    }));
    let build_callback = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_callback_request(m, "callback/build_agent"),
        )
        .expect("callback/build_agent request for agent:alpha");
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": build_callback["id"].clone(),
        "result": {}
    }));
    let dispatch = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "dispatch"),
        )
        .expect("dispatch response after callback reply");
    assert!(
        dispatch.get("result").is_some(),
        "dispatch must succeed so the member is Active with a live session: {dispatch}"
    );

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "tools",
        "method": "meta/identity/resolved_tools",
        "params": { "identity": "agent:alpha" }
    }));
    let resolved = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "tools"),
        )
        .expect("resolved_tools response");
    let tools: Vec<String> = resolved["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("resolved_tools must return a tools array: {resolved}"))
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    assert!(
        tools.iter().any(|name| name.starts_with("workgraph_")),
        "profile declares tools.workgraph = true, so a CREATED member's \
         resolved surface must carry workgraph_* (fail-open declared-vs-\
         resolved divergence, activation-41 class). Resolved: {tools:?}"
    );
}

/// The downstream app's exact re-entrancy shape (seam inventory row 6): the host calls
/// `meta/agent_memory/recall` from INSIDE a callback the gateway is waiting
/// on, for the very identity whose build is parked on that callback. The
/// recall must be served concurrently through the full runtime path
/// (identity status read + configured memory provider), not queue behind the
/// parked build until the callback deadline. This is the deadlock that drove
/// the downstream app to read the memory sqlite directly; it was fixed by the
/// concurrent dispatch loop (#260) and this test pins the recall path
/// specifically, locks and all.
#[test]
fn agent_memory_recall_answers_while_the_same_identity_build_is_parked() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "agent_memory": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init"),
        )
        .expect("identity-first init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "identity-first init failed: {init}"
    );

    // Lazy bootstrap registered agent:alpha without materializing it. This
    // dispatch triggers the callback-built materialization and parks the
    // dispatch RPC on the unanswered callback/build_agent.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "dispatch",
        "method": "meta/dispatch",
        "params": {
            "identity": "agent:alpha",
            "dispatch_input": { "content": "hello", "origin": "connector" }
        }
    }));
    let build_callback = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_callback_request(m, "callback/build_agent"),
        )
        .expect("callback/build_agent request for agent:alpha");
    let callback_id = build_callback["id"].clone();

    // THE regression: with agent:alpha's build parked (its lifecycle
    // authority held across the callback await), recall for that SAME
    // identity must still answer. A recall that queues behind the build - on
    // the dispatch loop or on any runtime lock the build holds - times this
    // wait out.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "recall",
        "method": "meta/agent_memory/recall",
        "params": { "identity": "agent:alpha" }
    }));
    let recall = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "recall"),
        )
        .expect("agent_memory/recall must answer while callback/build_agent is pending");
    assert!(
        recall["result"]["records"].is_array(),
        "recall must succeed through the configured memory provider, not \
         merely fail fast: {recall}"
    );

    // Unblock the parked build and prove the dispatch was parked, not lost.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": callback_id,
        "result": {}
    }));
    let dispatch = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "dispatch"),
        )
        .expect("dispatch response after callback reply");
    assert!(
        dispatch.get("result").is_some() || dispatch.get("error").is_some(),
        "dispatch response malformed: {dispatch}"
    );
}

/// Drive one materialization to completion: dispatch to `agent:alpha`, answer
/// the held `callback/build_agent` with `{}`, and wait for the dispatch
/// response, answering the roster callback throughout.
fn dispatch_alpha_through_build(gateway: &mut Gateway, dispatch_id: &str) -> Value {
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": dispatch_id,
        "method": "meta/dispatch",
        "params": {
            "identity": "agent:alpha",
            "dispatch_input": { "content": "hello", "origin": "connector" }
        }
    }));
    let build_callback = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_callback_request(m, "callback/build_agent"),
        )
        .expect("callback/build_agent request for agent:alpha");
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": build_callback["id"].clone(),
        "result": {}
    }));
    gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, dispatch_id),
        )
        .expect("dispatch response after callback reply")
}

/// Answer the roster callback as `answer_roster_callback_holding_builds`
/// does, and `callback/build_agent` with `{}`: for a process that may build
/// a member on its own, such as one whose durable mob storage restores it.
fn answer_roster_and_build_callbacks(gateway: &mut Gateway, message: &Value) {
    if is_callback_request(message, "callback/build_agent") {
        gateway.send(json!({
            "jsonrpc": "2.0",
            "id": message["id"].clone(),
            "result": {}
        }));
    } else {
        answer_roster_callback_holding_builds(gateway, message);
    }
}

/// Dispatch to `agent:alpha` and wait for the dispatch response, answering a
/// `callback/build_agent` if one comes, without requiring one.
fn dispatch_alpha_answering_builds(gateway: &mut Gateway, dispatch_id: &str) -> Value {
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": dispatch_id,
        "method": "meta/dispatch",
        "params": {
            "identity": "agent:alpha",
            "dispatch_input": { "content": "hello", "origin": "connector" }
        }
    }));
    gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_and_build_callbacks(gateway, message);
            },
            |m| is_response_with_id(m, dispatch_id),
        )
        .expect("dispatch response")
}

/// Core's identity lease TTL (`IDENTITY_LEASE_MAX_TTL_MS`): a restarted
/// process waits out the previous process's lease on an identity for at most
/// this long after that process last renewed it.
const IDENTITY_LEASE_TTL: Duration = Duration::from_secs(30);

/// Poll `meta/status_identity` for `identity` until Core's reconciler has
/// converged it at `min_revision` or later (a runtime id, a desired intent
/// revision of at least `min_revision`, and an active intent revision equal to
/// it), answering every callback, builds included. Without the minimum an
/// earlier converged revision would satisfy the wait before the latest one
/// lands. Panics with the last status once the lease TTL plus `WEDGE_BACKSTOP`
/// has passed.
fn await_identity_converged(gateway: &mut Gateway, identity: &str, min_revision: u64) -> Value {
    let deadline = Instant::now() + IDENTITY_LEASE_TTL + WEDGE_BACKSTOP;
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        let request_id = format!("converge-{attempt}");
        gateway.send(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "meta/status_identity",
            "params": { "identity": identity }
        }));
        let status = gateway
            .wait_for(
                WEDGE_BACKSTOP,
                |gateway, message| {
                    answer_roster_and_build_callbacks(gateway, message);
                },
                |m| is_response_with_id(m, &request_id),
            )
            .expect("status_identity response")["result"]
            .clone();
        let convergence = &status["convergence"];
        // Two absent revisions would compare equal: the active one must be a
        // revision.
        if status["agent_runtime_id"].is_string()
            && convergence["desired_intent_revision"]
                .as_u64()
                .is_some_and(|desired| desired >= min_revision)
            && convergence["active_intent_revision"].is_u64()
            && convergence["active_intent_revision"] == convergence["desired_intent_revision"]
        {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "{identity} did not converge at revision {min_revision} or later within the \
             identity lease TTL plus {WEDGE_BACKSTOP:?}: {status}"
        );
        // Keep answering callbacks between the reads.
        let _ = gateway.wait_for(
            Duration::from_millis(250),
            |gateway, message| {
                answer_roster_and_build_callbacks(gateway, message);
            },
            |_| false,
        );
    }
}

/// The session `agent:alpha` is bound to, from `meta/status_identity`.
fn session_id_of_alpha(gateway: &mut Gateway, request_id: &str) -> String {
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "meta/status_identity",
        "params": { "identity": "agent:alpha" }
    }));
    let status = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, request_id),
        )
        .expect("status_identity response");
    status["result"]["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("agent:alpha must be bound to a session: {status}"))
        .to_string()
}

/// Fetch `meta/identity/resolved_tools` for `agent:alpha` as a name list.
fn resolved_tools_for_alpha(gateway: &mut Gateway, request_id: &str) -> Vec<String> {
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "meta/identity/resolved_tools",
        "params": { "identity": "agent:alpha" }
    }));
    let resolved = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, request_id),
        )
        .expect("resolved_tools response");
    resolved["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("resolved_tools must return a tools array: {resolved}"))
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Declared-vs-resolved capability invariant, RESUME half (activation-41),
/// pinning the meerkat 0.8.21 resume-tooling merge.
///
/// Mechanism of the defect this pins: member CREATION threads `profile.tools`
/// into per-category tool overrides and stamps the SAME categories into the
/// session's creation-era `metadata.tooling`. Through meerkat 0.8.20 every
/// RESUME then overwrote the current profile's overrides from that
/// creation-era `metadata.tooling` wholesale (`apply_resumed_session_metadata`,
/// no resume-override mask) - so a tool category enabled on the profile AFTER
/// the member was created was unmaskable: it never resolved on ANY resume, on
/// every boot, forever (a downstream app's activation-41: `tools.workgraph = true`
/// added post-creation never surfaced `workgraph_*`). Meerkat 0.8.21 replaces
/// the overwrite with a unified merge (`merge_resumed_tool_category`): an
/// EXPLICIT current-profile setting wins, and creation-era metadata fills
/// only categories the current profile leaves `Inherit`.
///
/// Two real gateway processes share one persistent-state dir and its durable
/// mob storage:
///
/// - PHASE 1 (create era): profile WITHOUT workgraph; dispatch materializes
///   `agent:alpha` via `callback/build_agent`, the turn commits its session,
///   graceful shutdown.
/// - Between the phases the stored definition advances to the workgraph
///   profile, as a later release's one-shot definition update does: the store
///   pins the definition a launch must match, and declaring the mob storage
///   in memory with a roster under `persistent_state` is refused.
/// - PHASE 2 (resume era): new process, profile WITH `tools.workgraph = true`;
///   the durable mob storage restores the member when the mob lifts during
///   init and builds it through the host SessionBuilder there, so phase 2
///   answers that build (`lazy_materialize` defers only the roster pass). The
///   member RESUMES the phase-1 session, and its resolved tool surface must
///   carry `workgraph_*`.
#[test]
fn durable_mob_storage_resumed_member_resolves_tool_category_declared_after_creation() {
    let state_dir = tempfile::tempdir().expect("state dir");

    // ------------------------------------------------------------------
    // PHASE 1: create era. Workgraph is ABSENT from the profile.
    // ------------------------------------------------------------------
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init1",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init1"),
        )
        .expect("phase-1 init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "phase-1 init failed: {init}"
    );

    let dispatch = dispatch_alpha_through_build(&mut gateway, "dispatch1");
    assert!(
        dispatch.get("result").is_some(),
        "phase-1 dispatch must succeed so agent:alpha is Active: {dispatch}"
    );

    // Create-era baseline: the profile does not declare workgraph, so the
    // created member's resolved surface must not carry it.
    let phase1_tools = resolved_tools_for_alpha(&mut gateway, "tools1");
    assert!(
        phase1_tools
            .iter()
            .all(|name| !name.starts_with("workgraph_")),
        "phase-1 profile has no workgraph, resolved surface must not either: {phase1_tools:?}"
    );

    let phase1_session_id = session_id_of_alpha(&mut gateway, "status1");

    // Graceful end of the create era: the explicit handshake keeps callback
    // admission open through runtime shutdown (final flushes through the
    // answerable host), then stdin EOF exits.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "shutdown1",
        "method": "meta/shutdown",
        "params": {}
    }));
    let shutdown = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "shutdown1"),
        )
        .expect("phase-1 shutdown handshake response");
    gateway_shutdown::assert_persistent_shutdown_completed(&shutdown, "phase-1 shutdown");
    gateway.close_stdin();
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
    drop(gateway);
    advance_stored_definition(state_dir.path(), MOB_CONFIG_WORKGRAPH);

    // ------------------------------------------------------------------
    // PHASE 2: resume era. The profile now DECLARES workgraph; agent:alpha's
    // persisted binding makes the build a RESUME by construction, not a
    // recreation.
    // ------------------------------------------------------------------
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init2",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG_WORKGRAPH,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        }
    }));
    // The durable mob storage restores the member that was live at the last
    // stop when the mob lifts during init, and builds it through the host
    // SessionBuilder there, as it has since Meta 0.9.0: answer that build.
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_and_build_callbacks(gateway, message);
            },
            |m| is_response_with_id(m, "init2"),
        )
        .expect("phase-2 init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "phase-2 init failed: {init}"
    );

    // The restored member is live already: the dispatch answers a build
    // callback if one comes, without requiring one.
    let dispatch = dispatch_alpha_answering_builds(&mut gateway, "dispatch2");
    assert!(
        dispatch.get("result").is_some(),
        "phase-2 dispatch must succeed so the resumed member is Active: {dispatch}"
    );

    // THE regression assertion: the resumed member's tool surface must carry
    // the category declared AFTER its creation. At the 0.8.20 pin this list
    // never contains workgraph_* (creation-era metadata.tooling overwrote the
    // explicit profile declaration on resume).
    let phase2_tools = resolved_tools_for_alpha(&mut gateway, "tools2");
    assert!(
        phase2_tools
            .iter()
            .any(|name| name.starts_with("workgraph_")),
        "profile now declares tools.workgraph = true, so the RESUMED member's \
         resolved surface must carry workgraph_* (meerkat 0.8.21 \
         merge_resumed_tool_category: explicit current profile wins over \
         creation-era metadata.tooling). Resolved: {phase2_tools:?}"
    );

    // And it really was a resume: the live member is bound to the phase-1
    // session, not a freshly created one.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "status2",
        "method": "meta/status_identity",
        "params": { "identity": "agent:alpha" }
    }));
    let status = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "status2"),
        )
        .expect("phase-2 status_identity response");
    assert_eq!(
        status["result"]["session_id"].as_str(),
        Some(phase1_session_id.as_str()),
        "resumed member must keep the phase-1 session (no recreation): {status}"
    );
}

/// Advance the stored definition in `state_dir`'s durable mob storage to
/// `definition`, as a later release's one-shot definition update does, then
/// wait until the store has closed.
fn advance_stored_definition(state_dir: &std::path::Path, definition: &str) {
    let path = state_dir.join("event_log.sqlite3");
    let definition = meerkat_mob::MobDefinition::from_toml(definition).expect("definition");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let storage = meerkat_mob::MobStorage::persistent(&path).expect("open the mob store");
        let receipt = storage
            .update_definition(1, definition)
            .await
            .expect("advance the stored definition");
        assert_eq!(receipt.epoch, 2, "{receipt:?}");
    });
    drop(runtime);
    // The store's connection worker closes on its own thread after the store
    // is dropped; the exclusive fence waits for it.
    drop(
        meerkat_sqlite::ExclusiveFence::acquire(&path, Duration::from_secs(30))
            .expect("the advanced store closed"),
    );
}

/// CONTRACT: a PERSISTENT mob storage pins `mob_config`. Two gateway processes
/// over the same state dir, second one carrying a changed definition, must be
/// refused with the typed actionable divergence error before the mob actuates.
///
/// This is the other half of
/// `durable_mob_storage_resumed_member_resolves_tool_category_declared_after_creation`.
/// Durable mob storage is what makes adopted identity declarations survive a
/// restart, and Meerkat refuses a definition that disagrees with the persisted
/// spec store on both create and resume (`sync_definition_with_spec_store`).
/// A changed profile reaches the store through a definition update, never by
/// launching with it, and the operator is told so rather than discovering it
/// as an internal error naming two stores.
#[test]
fn persistent_mob_storage_refuses_changed_definition_on_the_same_state_dir() {
    let state_dir = tempfile::tempdir().expect("state dir");

    // PHASE 1: create era, persistent mob storage (the default on a
    // persistent_state launch), definition WITHOUT workgraph.
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init1",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init1"),
        )
        .expect("phase-1 init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "phase-1 init must succeed so the composition is recorded: {init}"
    );

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "shutdown1",
        "method": "meta/shutdown",
        "params": {}
    }));
    let shutdown = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "shutdown1"),
        )
        .expect("phase-1 shutdown handshake response");
    gateway_shutdown::assert_persistent_shutdown_completed(&shutdown, "phase-1 shutdown");
    gateway.close_stdin();
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
    drop(gateway);

    // PHASE 2: same state dir, CHANGED definition, still persistent.
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init2",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG_WORKGRAPH,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init2"),
        )
        .expect("phase-2 init response");

    // Positive observables. Asserting merely that init failed would pass on any
    // unrelated breakage, and asserting the absence of a success field would
    // pass against a hang.
    let message = init["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("phase-2 must be refused with a typed error: {init}"));
    assert!(
        message.contains("supplied mob definition diverges from the canonical definition"),
        "the refusal must be the composition-divergence error, not an internal store \
         mismatch: {message}"
    );
    assert!(
        message.contains("profiles.default.tools.workgraph"),
        "the refusal must name the diverged field so the operator can act: {message}"
    );
    assert!(
        message.contains("advance it through the declared definition-update ceremony"),
        "the refusal must state the sanctioned remedy: {message}"
    );
}

/// RELEASE CONTRACT: an adopted and applied member tool declaration survives a
/// real process restart.
///
/// This is the user-visible property the persistent-mob-storage repair exists
/// for, and it is deliberately tested across two gateway PROCESSES rather than
/// an in-process rebootstrap. In-process, the comms participant registry keeps
/// the member endpoint bound and a second bootstrap of the same mob collides on
/// its durable generation binding, so an in-process test cannot model a
/// restart. Two processes can.
///
/// Both phases carry the SAME definition: a persistent mob storage pins
/// `mob_config`, which the divergence contract above covers separately. What is
/// under test here is that declaration state written in phase 1 is readable in
/// phase 2 at the revision it was left at.
#[test]
fn adopted_and_applied_declaration_survives_a_real_process_restart() {
    const MOB_ID: &str = "gateway-concurrent-dispatch-test";
    const ALIAS: &str = "agent:alpha";
    let state_dir = tempfile::tempdir().expect("state dir");

    let init_params = |id: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "meta/init",
            "params": {
                "persistent_state": state_dir.path(),
                "mob_config": MOB_CONFIG,
                "has_roster_provider": true,
                "has_session_builder": true,
                "runtime_options": {
                    "demo_llm": true,
                    "identity_bootstrap_mode": { "mode": "lazy_materialize" }
                }
            }
        })
    };
    let declaration = json!({
        "category_overrides": {
            "builtins": "inherit",
            "shell": "enable",
            "comms": "inherit",
            "mob": "inherit",
            "memory": "inherit",
            "schedule": "inherit",
            "workgraph": "inherit",
            "image_generation": "inherit",
            "web_search": "inherit"
        },
        "callback_tools": { "kind": "set", "tools": [] },
        "execution": { "kind": "unrestricted" },
        "application_policy": { "kind": "unmanaged" }
    });

    // ------------------------------------------------------------------
    // PHASE 1: adopt revision 1, then apply revision 2, then exit.
    // ------------------------------------------------------------------
    let mut gateway = Gateway::start();
    gateway.send(init_params("init1"));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init1"),
        )
        .expect("phase-1 init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "phase-1 init failed: {init}"
    );

    let dispatch = dispatch_alpha_through_build(&mut gateway, "dispatch1");
    assert!(
        dispatch.get("result").is_some(),
        "phase-1 dispatch must succeed so {ALIAS} has a bridge session: {dispatch}"
    );
    let session_id = session_id_of_alpha(&mut gateway, "status0");

    // Declarations are keyed on the STABLE durable identity, not on the
    // runtime id: that is a per-binding incarnation, and the apply below moves
    // the identity to a new one. The runtime id is read only to prove that the
    // restart itself keeps the incarnation the apply left.
    let rpc_result = |gateway: &mut Gateway, id: &str, method: &str, params: Value| -> Value {
        gateway.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        let status = gateway
            .wait_for(
                WEDGE_BACKSTOP,
                |gateway, message| {
                    answer_roster_callback_holding_builds(gateway, message);
                },
                |m| is_response_with_id(m, id),
            )
            .expect("status_identity response");
        status["result"].clone()
    };
    let runtime_identity = |status: &Value| -> String {
        status["agent_runtime_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{ALIAS} must have a runtime id: {status}"))
            .to_string()
    };
    // The member the phase-1 dispatch spawned, before any intent existed.
    let before_adoption = rpc_result(
        &mut gateway,
        "status1",
        "meta/status_identity",
        json!({ "identity": ALIAS }),
    );
    // Adoption must state the EXACT existing baseline. `meta/status_identity`
    // does not project runtime_mode, so read it from the member surface, which
    // does, rather than guessing a default that would be rejected as a change.
    // Adoption is for an ALREADY-realized member, and upstream compares the
    // declaration's MATERIALIZED runtime mode AND labels against the live roster
    // entry (`material_runtime_mode != entry.runtime_mode ||
    // material.overlay.labels.unwrap_or_default() != entry.labels`). An omitted
    // field is therefore not "unchanged": it materializes to a default and is
    // rejected as a change. So echo the live entry exactly rather than guessing
    // a mode, and take labels from the ROSTER entry, not from
    // status_identity (whose labels are the identity's and are empty here).
    let members = rpc_result(&mut gateway, "members1", "meta/list_members", json!({}));
    let entry = members
        .as_array()
        .unwrap_or_else(|| panic!("list_members must return an array: {members}"))
        .iter()
        .find(|entry| {
            entry["agent_identity"]
                .as_str()
                .is_some_and(|id| meerkat_meta::member_comms_id::runtime_alias_str(id) == ALIAS)
        })
        .unwrap_or_else(|| panic!("{ALIAS} must be on the roster: {members}"))
        .clone();
    // A missing field here is a surface regression, not something to paper over.
    let baseline_runtime_mode = entry
        .get("runtime_mode")
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| panic!("list_members entry must project runtime_mode: {entry}"))
        .clone();
    let baseline_labels = entry
        .get("labels")
        .unwrap_or_else(|| panic!("list_members entry must project labels: {entry}"))
        .clone();

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "adopt1",
        "method": "mob/adopt_member_identity_declaration",
        "params": {
            "mob_id": MOB_ID,
            "agent_identity": ALIAS,
            "request_id": "adopt-restart-1",
            "precondition": "expected_absent",
            "declaration_scope": "restart-durability-test",
            "declaration_revision": 1,
            "session": {
                "session_id": session_id,
                "lineage_id": format!("session:{session_id}"),
                "lineage_generation": 0,
                "authority_policy": "require_existing"
            },
            "member": {
                "profile_name": "default",
                // NO system_prompt_override, deliberately. This launch wires no
                // SpawnBasePromptSource and the profile declares no skills, which
                // used to force the declaration to carry its own prompt - and
                // meerkat 0.8.28 now REFUSES exactly that for an existing
                // session: "identity adoption cannot restate an existing
                // session's durable system prompt; omit system_prompt_override so
                // the transcript remains authoritative". Adoption reads the
                // persisted transcript instead, so the workaround is both
                // unnecessary and rejected.
                "runtime_mode": baseline_runtime_mode,
                "labels": baseline_labels,
                "execution": { "execution": "controlling_session" }
            },
            "owned_wiring": [],
            "convergence": { "kind": "drain", "max_wait_ms": 5000 }
        }
    }));
    let adoption = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "adopt1"),
        )
        .expect("adopt response");
    assert_eq!(
        adoption["error"],
        Value::Null,
        "adopt failed: {adoption:#?}"
    );
    assert_eq!(
        adoption["result"]["adoption"]["desired_revision"],
        json!(1),
        "adopt must land revision 1: {adoption:#?}"
    );

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "apply1",
        "method": "mob/apply_member_tool_declaration",
        "params": {
            "mob_id": MOB_ID,
            "agent_identity": ALIAS,
            "request_id": "apply-restart-1",
            "expected_intent_revision": 1,
            "declaration": declaration,
            "convergence": { "kind": "drain", "max_wait_ms": 5000 }
        }
    }));
    let apply = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "apply1"),
        )
        .expect("apply response");
    assert_eq!(apply["error"], Value::Null, "apply failed: {apply:#?}");
    assert_eq!(
        apply["result"]["commit"]["desired_revision"],
        json!(2),
        "apply must land revision 2: {apply:#?}"
    );
    let applied_revision = 2;
    // The applied declaration changes the member's tool categories, so Core
    // replaces the member to converge it: it retires the member and spawns its
    // successor at the next generation, on the same session. Await that before
    // the restart, so the comparison after it sees the restart alone; a stop
    // in the middle of the replacement would leave its respawn to the next
    // process. The wait requires the applied revision: the adoption's
    // revision 1 also reads as converged until revision 2 lands.
    let converged = await_identity_converged(&mut gateway, ALIAS, applied_revision);
    let generation = |status: &Value| {
        status["generation"]
            .as_u64()
            .unwrap_or_else(|| panic!("{ALIAS} must have a generation: {status}"))
    };
    assert!(
        generation(&converged) > generation(&before_adoption),
        "converging the applied declaration replaces the member at a later generation: \
         before {before_adoption}, converged {converged}"
    );
    let phase1_runtime = runtime_identity(&converged);

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "shutdown1",
        "method": "meta/shutdown",
        "params": {}
    }));
    let shutdown = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "shutdown1"),
        )
        .expect("phase-1 shutdown handshake response");
    gateway_shutdown::assert_persistent_shutdown_completed(&shutdown, "phase-1 shutdown");
    gateway.close_stdin();
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
    drop(gateway);

    // ------------------------------------------------------------------
    // PHASE 2: a NEW process over the same state dir must still see it.
    // ------------------------------------------------------------------
    let mut gateway = Gateway::start();
    gateway.send(init_params("init2"));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_and_build_callbacks(gateway, message);
            },
            |m| is_response_with_id(m, "init2"),
        )
        .expect("phase-2 init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "phase-2 init failed: {init}"
    );

    // The previous process's incarnation still holds the identity's lease:
    // Core releases no identity lease at shutdown, so the new process reads
    // the identity `await_lease`, with no runtime id, until that lease
    // expires (`IDENTITY_LEASE_TTL` after its last renewal) and Core's
    // reconciler converges it. Await that rather than read it at once.
    //
    // A plain restart RESUMES the incarnation the converged apply left: the
    // generation is minted per identity by Meerkat and is preserved across
    // process replacement, so this is the same runtime id, not a new one.
    // Asserted rather than assumed: a restart that rotated the binding would
    // show here.
    let phase2_runtime = runtime_identity(&await_identity_converged(
        &mut gateway,
        ALIAS,
        applied_revision,
    ));
    assert_eq!(
        phase1_runtime, phase2_runtime,
        "a restart resumes the same incarnation; a changed runtime id here would mean the \
         process replacement rotated the binding instead of resuming it"
    );

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "read2",
        "method": "mob/member_tool_declaration",
        "params": { "mob_id": MOB_ID, "agent_identity": ALIAS }
    }));
    let read = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "read2"),
        )
        .expect("phase-2 declaration read response");

    // Positive observables: the exact revision and the exact declaration. A
    // bare "no error" would pass against a fresh empty mob, which is precisely
    // the failure this repair exists to prevent.
    assert_eq!(read["error"], Value::Null, "phase-2 read failed: {read:#?}");
    assert_eq!(read["result"]["agent_identity"], json!(ALIAS));
    assert_eq!(
        read["result"]["desired_intent_revision"],
        json!(2),
        "the applied revision must survive the process restart: {read:#?}"
    );
    assert_eq!(
        read["result"]["declaration"], declaration,
        "the declaration content must survive the process restart: {read:#?}"
    );

    // And it is bound to the SAME session, not a freshly minted one. A
    // declaration that survived while its session did not would still be a
    // broken restart.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "status2",
        "method": "meta/status_identity",
        "params": { "identity": ALIAS }
    }));
    let status = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "status2"),
        )
        .expect("phase-2 status_identity response");
    assert_eq!(
        status["result"]["session_id"].as_str(),
        Some(session_id.as_str()),
        "the restarted process must keep the phase-1 session, not create a new one: {status}"
    );
    // The adoption left Core an identity intent row: the host manages this
    // identity, and Meta's view says so after the restart (its lifecycle verbs
    // refuse it typed).
    assert_eq!(
        status["result"]["intent_managed"],
        json!(true),
        "the adopted identity is host-managed after the restart: {status}"
    );

    // NOTE for whoever extends this: the applied declaration already outlives
    // a NEW generation in phase 1, because Core converges it by replacing the
    // member, and a restart resumes that incarnation; both are asserted above.
}

// ---------------------------------------------------------------------------
// meta/reset_all on an identity-first gateway (item R7)
// ---------------------------------------------------------------------------

/// Roster for the reset_all regression: two registered identities on the
/// `default` profile.
const RESET_ALL_IDENTITIES: [&str; 2] = ["agent:alpha", "agent:beta"];

/// Provider answerer for [`RESET_ALL_IDENTITIES`]: the roster, and a `null`
/// acknowledgement for any other host verb. The init below declares no session
/// builder, so the gateway builds the demo agents itself and
/// `callback/build_agent` never arrives.
fn answer_reset_all_host_callbacks(gateway: &mut Gateway, message: &Value) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let Some(id) = message.get("id").cloned() else {
        return;
    };
    let result = match method {
        "callback/roster_provider/roster" => Value::Array(
            RESET_ALL_IDENTITIES
                .iter()
                .map(|identity| {
                    json!({
                        "identity": identity,
                        "profile": "default",
                        "addressability": "addressable",
                        "display_name": null,
                        "labels": {},
                        "context": null,
                        "additional_instructions": []
                    })
                })
                .collect(),
        ),
        _ => Value::Null,
    };
    gateway.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

/// One HTTP exchange against the gateway's console door.
struct HttpReply {
    status: u16,
    body: String,
}

/// Minimal blocking HTTP/1.1 client over [`TcpStream`]. This suite has no
/// HTTP dependency and the console door is plain JSON over HTTP, so this is
/// enough: one request per connection, `Connection: close`, body read to EOF,
/// chunked transfer decoded if the server chose it.
fn http_request(base_url: &str, method: &str, path: &str, body: Option<&str>) -> HttpReply {
    let authority = base_url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("http base url must be plain http: {base_url}"))
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(authority).expect("connect to gateway http door");
    stream
        .set_read_timeout(Some(WEDGE_BACKSTOP))
        .expect("http read timeout");
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write http request");
    stream.flush().expect("flush http request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .expect("read http response to eof");
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| {
            panic!(
                "malformed http response (no header terminator): {}",
                String::from_utf8_lossy(&raw)
            )
        });
    let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    let payload = &raw[header_end + 4..];
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("malformed http status line: {status_line}"));
    let chunked = head.lines().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let body_bytes = if chunked {
        decode_chunked_body(payload)
    } else {
        payload.to_vec()
    };
    HttpReply {
        status,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
    }
}

fn decode_chunked_body(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size line terminator");
        let size_line = String::from_utf8_lossy(&rest[..line_end]).into_owned();
        let size =
            usize::from_str_radix(size_line.split(';').next().unwrap_or_default().trim(), 16)
                .unwrap_or_else(|_| panic!("malformed chunk size: {size_line}"));
        rest = &rest[line_end + 2..];
        if size == 0 {
            break;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
    out
}

/// Issue one console RPC over HTTP while keeping the stdin callback loop
/// alive. The handler behind `/console/rpc` may round-trip host callbacks over
/// stdout, so the POST must not run on the thread that answers them or the two
/// wait on each other.
fn console_rpc_while_answering_callbacks(
    gateway: &mut Gateway,
    base_url: &str,
    request: Value,
    mut answer: impl FnMut(&mut Gateway, &Value),
) -> Value {
    let method = request["method"].as_str().unwrap_or_default().to_string();
    let base_url = base_url.to_string();
    let body = serde_json::to_string(&request).expect("console rpc json");
    let (reply_tx, reply_rx) = mpsc::channel();
    thread::spawn(move || {
        let reply = http_request(&base_url, "POST", "/console/rpc", Some(&body));
        let _ = reply_tx.send(reply);
    });
    let start = Instant::now();
    loop {
        match reply_rx.try_recv() {
            Ok(reply) => {
                assert_eq!(
                    reply.status, 200,
                    "console rpc {method} must answer 200: {}",
                    reply.body
                );
                return serde_json::from_str(&reply.body).unwrap_or_else(|err| {
                    panic!(
                        "console rpc {method} body is not json ({err}): {}",
                        reply.body
                    )
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                panic!("http thread for console rpc {method} died without a reply")
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        assert!(
            start.elapsed() < WEDGE_BACKSTOP,
            "console rpc {method} did not answer within {WEDGE_BACKSTOP:?}"
        );
        match gateway.lines.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                    answer(gateway, &message);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("gateway stdout closed while console rpc {method} was in flight")
            }
        }
    }
}

/// `meta/status_identity` for every identity in [`RESET_ALL_IDENTITIES`],
/// keyed by identity.
fn reset_all_identity_statuses(gateway: &mut Gateway, tag: &str) -> BTreeMap<String, Value> {
    let mut statuses = BTreeMap::new();
    for identity in RESET_ALL_IDENTITIES {
        let request_id = format!("status-{tag}-{identity}");
        gateway.send(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "meta/status_identity",
            "params": { "identity": identity }
        }));
        let response = gateway
            .wait_for(WEDGE_BACKSTOP, answer_reset_all_host_callbacks, |m| {
                is_response_with_id(m, &request_id)
            })
            .unwrap_or_else(|| panic!("status_identity response for {identity} ({tag})"));
        assert_eq!(
            response["error"],
            Value::Null,
            "status_identity for {identity} ({tag}) failed: {response:#?}"
        );
        statuses.insert(identity.to_string(), response["result"].clone());
    }
    statuses
}

/// Item R7: `meta/reset_all` on an identity-first gateway RESETS every
/// registered identity and keeps the process alive.
///
/// Before this the handler chose reset-versus-retire by membership in the
/// baseline member-spec slot, which only the non-identity-first
/// `UnifiedRuntime::reconcile` populates. On every shipped gateway that slot
/// is empty, so every registered identity fell into the retire arm: the body
/// read `reset: []`, `retired_delegates: [agent:alpha, agent:beta]`, the
/// timeline carried two `identity_retired` frames and no `identity_reset`,
/// and the operator's fleet was gone (reproduced 2026-09-03 against this
/// binary). The reported process exit did not reproduce; liveness is asserted
/// here anyway because it is the other half of the report and nothing else
/// pins it.
///
/// Every assertion is a positive observable: the body lists both identities
/// under `reset`, one `identity_reset` frame per identity is on the timeline,
/// the continuity generation of each identity is 1 with a new runtime id, and
/// `/healthz` answers after the call.
#[test]
fn reset_all_resets_every_registered_identity_and_keeps_the_gateway_alive() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let (mut gateway, stderr) = Gateway::start_capturing_stderr("meerkat_meta=info");
    let captured = || {
        stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("\n")
    };

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "has_roster_provider": true,
            "runtime_options": {
                "demo_llm": true,
                // Both identities must be LIVE before the call, so the reset
                // path (not a dormant no-op) is what runs.
                "identity_bootstrap_mode": { "mode": "eager_materialize" },
                // The console door defaults to requiring an app bearer token;
                // this test speaks to it directly.
                "console_require_app_auth": false
            }
        }
    }));
    let init = gateway
        .wait_for(WEDGE_BACKSTOP, answer_reset_all_host_callbacks, |m| {
            is_response_with_id(m, "init")
        })
        .expect("init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}\nstderr:\n{}",
        captured()
    );
    let base_url = init["result"]["http_base_url"]
        .as_str()
        .unwrap_or_else(|| panic!("init result must carry http_base_url: {init}"))
        .to_string();

    let before = reset_all_identity_statuses(&mut gateway, "before");
    for identity in RESET_ALL_IDENTITIES {
        assert_eq!(
            before[identity]["state"],
            json!("active"),
            "{identity} must be active before reset_all, or the reset path is not what runs: {before:#?}"
        );
        assert_eq!(
            before[identity]["generation"],
            json!(0),
            "{identity} must start at generation 0: {before:#?}"
        );
    }

    let reset_all = console_rpc_while_answering_callbacks(
        &mut gateway,
        &base_url,
        json!({ "jsonrpc": "2.0", "id": "reset-all", "method": "meta/reset_all", "params": {} }),
        answer_reset_all_host_callbacks,
    );
    assert_eq!(
        reset_all["error"],
        Value::Null,
        "reset_all must succeed for both registered identities: {reset_all:#?}\nstderr:\n{}",
        captured()
    );
    let result = &reset_all["result"];
    assert_eq!(
        result["reset"],
        json!(RESET_ALL_IDENTITIES),
        "every registered identity must be RESET, not retired: {reset_all:#?}"
    );
    assert_eq!(
        result["retired_delegates"],
        json!([]),
        "a registered identity is never a retire target for reset_all: {reset_all:#?}"
    );
    assert_eq!(
        result["failed"],
        json!([]),
        "reset_all reported failures: {reset_all:#?}\nstderr:\n{}",
        captured()
    );
    // The bridge's successor transition retires the predecessor, so there is
    // no stale-member cleanup gap to warn about; a warning here would be a
    // claim about a gap that does not exist.
    assert_eq!(
        result["warnings"],
        json!([]),
        "reset_all must not warn about a cleanup it did not skip: {reset_all:#?}"
    );

    // The other half of the report: the process is still there and serving.
    assert!(
        gateway
            .child
            .try_wait()
            .expect("poll gateway exit")
            .is_none(),
        "rpc_gateway exited after reset_all\nstderr:\n{}",
        captured()
    );
    let health = http_request(&base_url, "GET", "/healthz", None);
    assert_eq!(
        health.status, 200,
        "/healthz must answer after reset_all: {}",
        health.body
    );
    assert_eq!(health.body.trim(), "ok", "/healthz body after reset_all");

    // The lifecycle the console shows: one identity_reset per identity, and
    // not a single identity_retired anywhere on the timeline.
    let timeline = console_rpc_while_answering_callbacks(
        &mut gateway,
        &base_url,
        json!({
            "jsonrpc": "2.0",
            "id": "timeline",
            "method": "meta/console/query_timeline",
            "params": { "limit": 400 }
        }),
        answer_reset_all_host_callbacks,
    );
    let frames = timeline["result"]["frames"]
        .as_array()
        .unwrap_or_else(|| panic!("query_timeline must return frames: {timeline:#?}"));
    for identity in RESET_ALL_IDENTITIES {
        let reset_frames = frames
            .iter()
            .filter(|frame| frame["kind"] == "identity_reset" && frame["identity"] == identity)
            .count();
        assert_eq!(
            reset_frames, 1,
            "exactly one identity_reset lifecycle frame for {identity}: {frames:#?}"
        );
    }
    let retired_frames = frames
        .iter()
        .filter(|frame| frame["kind"] == "identity_retired")
        .count();
    assert_eq!(
        retired_frames, 0,
        "reset_all must not retire any registered identity: {frames:#?}"
    );

    // The durable fact: each identity is still registered, live, one
    // generation further, under a new incarnation.
    let after = reset_all_identity_statuses(&mut gateway, "after");
    for identity in RESET_ALL_IDENTITIES {
        assert_eq!(
            after[identity]["state"],
            json!("active"),
            "{identity} must be active again after reset_all: {after:#?}"
        );
        assert_eq!(
            after[identity]["generation"],
            json!(1),
            "{identity} must have advanced to generation 1: {after:#?}"
        );
        assert_ne!(
            after[identity]["agent_runtime_id"], before[identity]["agent_runtime_id"],
            "{identity} must run under a new incarnation after reset_all: before {:#?} after {:#?}",
            before[identity], after[identity]
        );
    }

    gateway.send(json!({
        "jsonrpc": "2.0", "id": "shutdown", "method": "meta/shutdown", "params": {}
    }));
    let shutdown = gateway
        .wait_for(WEDGE_BACKSTOP, answer_reset_all_host_callbacks, |m| {
            is_response_with_id(m, "shutdown")
        })
        .expect("shutdown handshake after reset_all");
    gateway_shutdown::assert_persistent_shutdown_completed(&shutdown, "shutdown after reset_all");
    gateway.close_stdin();
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
}

/// The agent-memory recorder follows memory being enabled, not whether the
/// host has its own `AgentCustomizer`. With memory enabled, a SessionBuilder
/// and NO host customizer, the member's resolved tools carry the recorder
/// (`memory`) on a fresh create and again after a normal stop and restart of
/// the same identity and session. A profile with memory disabled never
/// carries it.
fn memory_recorder_catalogs_across_restart(profile_memory: bool) -> (Vec<String>, Vec<String>) {
    let mob_config = format!(
        r#"
[mob]
id = "gateway-memory-recorder-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
memory = {profile_memory}
"#
    );
    let init_params = |state_dir: &tempfile::TempDir| {
        let params = json!({
            "persistent_state": state_dir.path(),
            "mob_config": mob_config,
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "agent_memory": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" }
            }
        });
        // Persistent mob storage (the default, and the only one a roster
        // under `persistent_state` may use) restores the member natively at
        // mob start, BEFORE any identity customization runs; the later
        // customized resume finds the member occupied and attaches it. Only
        // the stable per-identity tool dispatcher reaches that occupant.
        params
    };
    let state_dir = tempfile::tempdir().expect("state dir");

    // Boot 1: fresh create.
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init1",
        "method": "meta/init",
        "params": init_params(&state_dir)
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "init1"),
        )
        .expect("boot-1 init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "boot-1 init failed: {init}"
    );
    let dispatch = dispatch_alpha_through_build(&mut gateway, "dispatch1");
    assert!(
        dispatch.get("result").is_some(),
        "boot-1 dispatch failed: {dispatch}"
    );
    let fresh = resolved_tools_for_alpha(&mut gateway, "tools1");
    let session_id = session_id_of_alpha(&mut gateway, "status1");
    gateway.send(
        json!({ "jsonrpc": "2.0", "id": "shutdown1", "method": "meta/shutdown", "params": {} }),
    );
    let shutdown = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "shutdown1"),
        )
        .expect("boot-1 shutdown response");
    gateway_shutdown::assert_persistent_shutdown_completed(&shutdown, "boot-1 shutdown");
    gateway.close_stdin();
    let status = gateway.wait_for_exit(WEDGE_BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");
    drop(gateway);

    // Boot 2: normal restart of the same identity and session.
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init2",
        "method": "meta/init",
        "params": init_params(&state_dir)
    }));
    // A native restore (persistent mob storage) rebuilds the member through
    // the host SessionBuilder during init: answer its build callback.
    let mut seen_callbacks = Vec::new();
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                if let Some(method) = message["method"].as_str() {
                    seen_callbacks.push(method.to_string());
                }
                if is_callback_request(message, "callback/build_agent") {
                    gateway.send(json!({
                        "jsonrpc": "2.0",
                        "id": message["id"].clone(),
                        "result": {}
                    }));
                } else {
                    answer_roster_callback_holding_builds(gateway, message);
                }
            },
            |m| is_response_with_id(m, "init2"),
        )
        .unwrap_or_else(|| panic!("boot-2 init response; callbacks seen: {seen_callbacks:?}"));
    assert!(
        init["result"]["contract_version"].is_string(),
        "boot-2 init failed: {init}"
    );
    // The member may already be live (a native restore built it during init),
    // so this dispatch answers a build callback if one comes, without
    // requiring one.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "dispatch2",
        "method": "meta/dispatch",
        "params": {
            "identity": "agent:alpha",
            "dispatch_input": { "content": "hello", "origin": "connector" }
        }
    }));
    let dispatch = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                if is_callback_request(message, "callback/build_agent") {
                    gateway.send(json!({
                        "jsonrpc": "2.0",
                        "id": message["id"].clone(),
                        "result": {}
                    }));
                } else {
                    answer_roster_callback_holding_builds(gateway, message);
                }
            },
            |m| is_response_with_id(m, "dispatch2"),
        )
        .expect("boot-2 dispatch response");
    assert!(
        dispatch.get("result").is_some(),
        "boot-2 dispatch failed: {dispatch}"
    );
    let restored = resolved_tools_for_alpha(&mut gateway, "tools2");
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "status2",
        "method": "meta/status_identity",
        "params": { "identity": "agent:alpha" }
    }));
    let status = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |gateway, message| {
                answer_roster_callback_holding_builds(gateway, message);
            },
            |m| is_response_with_id(m, "status2"),
        )
        .expect("boot-2 status_identity response");
    assert_eq!(
        status["result"]["session_id"].as_str(),
        Some(session_id.as_str()),
        "the restarted member keeps its session: {status}"
    );
    (fresh, restored)
}

#[test]
fn memory_disabled_profile_never_carries_the_recorder() {
    let (fresh, restored) = memory_recorder_catalogs_across_restart(false);
    assert!(
        !fresh.iter().any(|name| name == "memory") && !restored.iter().any(|name| name == "memory"),
        "a memory-disabled profile never carries the recorder: fresh={fresh:?} \
         restored={restored:?}"
    );
}

/// The cold-restore shape: persistent mob storage restores the member natively
/// before customization, and the customized resume attaches that occupant.
/// The recorder must still be on it after the restart.
#[test]
fn memory_recorder_survives_a_native_restore_without_a_host_customizer() {
    let (fresh, restored) = memory_recorder_catalogs_across_restart(true);
    assert!(
        fresh.iter().any(|name| name == "memory"),
        "the fresh member carries the recorder: {fresh:?}"
    );
    assert!(
        restored.iter().any(|name| name == "memory"),
        "the natively restored occupant keeps the recorder: {restored:?}"
    );
}

// ---------------------------------------------------------------------------
// Owned member MCP stdio servers at shutdown
// ---------------------------------------------------------------------------

/// A minimal stdio MCP server for a member profile. It records its pid,
/// answers `initialize`, `tools/list` and `ping`, and then on stdin EOF
/// either exits (`cooperative`) or ignores the EOF and SIGTERM until it is
/// killed (`stubborn`). `escaping` is `stubborn` plus a child in a session of
/// its own that keeps the server's stdout open and records its pid next to
/// the server's.
#[cfg(target_os = "linux")]
const MEMBER_MCP_FIXTURE: &str = r#"
import json, os, signal, sys, time

mode, pid_path = sys.argv[1], sys.argv[2]


def record(path, pid):
    with open(path + ".tmp", "w") as handle:
        handle.write(str(pid))
    os.replace(path + ".tmp", path)


if mode != "cooperative":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
if mode == "escaping" and os.fork() == 0:
    os.setsid()
    record(pid_path + ".escaped", os.getpid())
    while True:
        time.sleep(3600)
record(pid_path, os.getpid())
while True:
    line = sys.stdin.readline()
    if not line:
        break
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        version = message.get("params", {}).get("protocolVersion", "2025-06-18")
        result = {
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "settlement-fixture", "version": "1"},
        }
        reply = {"jsonrpc": "2.0", "id": message["id"], "result": result}
    elif method == "tools/list":
        reply = {"jsonrpc": "2.0", "id": message["id"], "result": {"tools": []}}
    elif method == "ping":
        reply = {"jsonrpc": "2.0", "id": message["id"], "result": {}}
    else:
        error = {"code": -32601, "message": "unsupported"}
        reply = {"jsonrpc": "2.0", "id": message["id"], "error": error}
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()
if mode != "cooperative":
    while True:
        time.sleep(3600)
"#;

/// Hang detector for the shutdown answer: the bounded member MCP close
/// (EOF grace, TERM grace and the kill backstop) fits well inside it.
#[cfg(target_os = "linux")]
const MEMBER_MCP_SHUTDOWN_BACKSTOP: Duration = Duration::from_mins(2);

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    /// No such process: it exited and was reaped.
    Gone,
    /// Exited, not reaped yet.
    Zombie,
    /// Still running, in this `/proc` state.
    Live(char),
}

#[cfg(target_os = "linux")]
fn process_state(pid: u32) -> ProcessState {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return ProcessState::Gone;
    };
    // The state is the first field after the parenthesized command name.
    match stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.trim_start().chars().next())
    {
        Some('Z') => ProcessState::Zombie,
        Some(state) => ProcessState::Live(state),
        None => ProcessState::Gone,
    }
}

#[cfg(target_os = "linux")]
fn recorded_pid(path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn wait_for_recorded_pid(path: &std::path::Path, deadline: Duration) -> Option<u32> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Some(pid) = recorded_pid(path) {
            return Some(pid);
        }
        thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Kills every recorded fixture process still alive when it is dropped, so
/// a failing run leaves no orphan behind.
#[cfg(target_os = "linux")]
struct FixtureProcesses(Vec<std::path::PathBuf>);

#[cfg(target_os = "linux")]
impl Drop for FixtureProcesses {
    fn drop(&mut self) {
        for path in &self.0 {
            if let Some(pid) = recorded_pid(path)
                && matches!(process_state(pid), ProcessState::Live(_))
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct MemberMcpShutdown {
    response: Value,
    /// The member's MCP server when the shutdown answer arrived.
    server: ProcessState,
    /// The `escaping` fixture's stdout holder when the answer arrived.
    escaped: Option<ProcessState>,
}

#[cfg(target_os = "linux")]
impl Gateway {
    /// [`Self::start`] in `workspace`, with placeholder provider secrets for
    /// natively built members (built, never called).
    fn start_in(workspace: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .current_dir(workspace)
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
            .env("XDG_STATE_HOME", workspace.join("xdg-state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (tx, rx) = mpsc::channel();
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
            lines: rx,
        }
    }
}

/// Boot a persistent gateway whose `worker` profile owns one stdio MCP
/// server (the fixture in `mode`), spawn a member, wait until its server is
/// running, then shut the gateway down. The process states are read the
/// moment the shutdown answer arrives.
#[cfg(target_os = "linux")]
fn shutdown_with_member_mcp_server(mode: &str) -> MemberMcpShutdown {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture = workspace.path().join("member_mcp_fixture.py");
    std::fs::write(&fixture, MEMBER_MCP_FIXTURE).expect("write fixture");
    let pid_path = workspace.path().join("member_mcp.pid");
    let escaped_path = workspace.path().join("member_mcp.pid.escaped");
    let _fixtures = FixtureProcesses(vec![pid_path.clone(), escaped_path.clone()]);
    let mob_config = format!(
        r#"
[mob]
id = "gateway-member-mcp-{mode}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true

[[profiles.worker.tools.mcp_servers]]
name = "settlement"
command = "python3"
args = [{fixture:?}, {mode:?}, {pid:?}]
"#,
        fixture = fixture.display().to_string(),
        pid = pid_path.display().to_string(),
    );

    let mut gateway = Gateway::start_in(workspace.path());
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": workspace.path().join("state"),
            "mob_config": mob_config,
        }
    }));
    let init = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |_, _| {},
            |m| is_response_with_id(m, "init"),
        )
        .expect("init response");
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}"
    );
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "spawn",
        "method": "meta/spawn_member",
        "params": { "profile": "worker", "meerkat_id": "worker-1" }
    }));
    let spawn = gateway
        .wait_for(
            WEDGE_BACKSTOP,
            |_, _| {},
            |m| is_response_with_id(m, "spawn"),
        )
        .expect("spawn response");
    assert!(spawn.get("result").is_some(), "spawn failed: {spawn}");
    let server = wait_for_recorded_pid(&pid_path, WEDGE_BACKSTOP)
        .expect("the member's MCP server never started");
    let escaped = (mode == "escaping").then(|| {
        wait_for_recorded_pid(&escaped_path, WEDGE_BACKSTOP)
            .expect("the escaping stdout holder never started")
    });

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "shutdown",
        "method": "meta/shutdown",
        "params": {}
    }));
    let response = gateway
        .wait_for(
            MEMBER_MCP_SHUTDOWN_BACKSTOP,
            |_, _| {},
            |m| is_response_with_id(m, "shutdown"),
        )
        .expect("shutdown response");
    let observed = MemberMcpShutdown {
        server: process_state(server),
        escaped: escaped.map(process_state),
        response,
    };
    gateway.close_stdin();
    let _ = gateway.wait_for_exit(WEDGE_BACKSTOP);
    observed
}

/// Control: a server that exits on stdin EOF settles within the member
/// teardown, so complete cleanup and a reaped server arrive together.
#[cfg(target_os = "linux")]
#[test]
fn shutdown_acknowledges_complete_cleanup_after_a_cooperative_member_mcp_server_is_reaped() {
    let observed = shutdown_with_member_mcp_server("cooperative");
    gateway_shutdown::assert_persistent_shutdown_completed(
        &observed.response,
        "cooperative member MCP server",
    );
    assert_eq!(
        observed.server,
        ProcessState::Gone,
        "complete cleanup was acknowledged while the member's MCP server was {:?}: {}",
        observed.server,
        observed.response
    );
}

/// The server ignores stdin EOF and SIGTERM, so only the close's SIGKILL ends
/// it, after the EOF and TERM graces (8 s by default): later than the 5 s
/// member teardown. Complete cleanup must not be acknowledged before the
/// server the gateway owns has been reaped.
#[cfg(target_os = "linux")]
#[test]
fn shutdown_does_not_acknowledge_complete_cleanup_before_a_stubborn_member_mcp_server_is_reaped() {
    let observed = shutdown_with_member_mcp_server("stubborn");
    gateway_shutdown::assert_persistent_shutdown_completed(
        &observed.response,
        "stubborn member MCP server",
    );
    assert_eq!(
        observed.server,
        ProcessState::Gone,
        "complete cleanup was acknowledged while the member's MCP server was {:?}: {}",
        observed.server,
        observed.response
    );
}

/// A process that left the server's group keeps its stdout open, so the
/// close cannot settle. The gateway still kills and reaps the server it
/// owns and answers within its bound, with an honest unsettled outcome: the
/// native attempt's own `stdout_open`, never a deadline.
#[cfg(target_os = "linux")]
#[test]
fn shutdown_reports_unsettled_cleanup_when_a_member_mcp_server_cannot_settle() {
    let observed = shutdown_with_member_mcp_server("escaping");
    let result = &observed.response["result"];
    assert_eq!(
        result["runtime_cleanup_completed"], false,
        "an unsettled member MCP close was acknowledged as complete cleanup: {}",
        observed.response
    );
    assert_eq!(result["shutdown"], false, "{}", observed.response);
    assert!(
        result.get("runtime_cleanup_diagnostics").is_some(),
        "an unsettled shutdown names its cause: {}",
        observed.response
    );
    let settlement = &result["runtime_cleanup_diagnostics"]["member_mcp_settlement"];
    for (field, expected) in [
        ("outcome", json!("unsettled")),
        ("servers", json!(1)),
        ("stdout_open", json!(1)),
        ("deadline", json!(0)),
        ("not_reaped", json!(0)),
        ("wait_failed", json!(0)),
    ] {
        assert_eq!(
            settlement[field], expected,
            "member_mcp_settlement.{field}: {}",
            observed.response
        );
    }
    assert_eq!(
        observed.server,
        ProcessState::Gone,
        "the close still kills and reaps the server it owns: {}",
        observed.response
    );
    assert!(
        matches!(observed.escaped, Some(ProcessState::Live(_))),
        "the stdout holder left the server's group, so no group signal reaches it: {:?}",
        observed.escaped
    );
}
