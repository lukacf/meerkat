//! The shell tool family of a mob member on the stock stdio gateway.
//!
//! `shell = true` turns on the whole family: `shell`, the three job tools
//! and `monitor_start`. A profile narrows it as it narrows any tool, with
//! `deny` or the grant gate. Each case is a real `rpc_gateway` process on a
//! pipe with a mob definition as an operator writes it, and reads the
//! member's live tool list through `meta/identity/resolved_tools`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

/// A hang detector for every wait here, never a latency measurement.
const WEDGE_BACKSTOP: Duration = Duration::from_mins(1);

/// The tools of the shell family.
const SHELL_FAMILY: [&str; 5] = [
    "shell",
    "shell_job_status",
    "shell_jobs",
    "shell_job_cancel",
    "monitor_start",
];

const MONITOR: &str = "monitor_start";

/// The mob of every row: one profile, `default`, whose `tools` table carries
/// `tools` beside `comms`. The profile names no runtime mode, so the roster
/// pass at `meta/init` builds `agent:alpha`.
fn mob_config(tools: &str) -> String {
    format!(
        r#"
[mob]
id = "gateway-member-shell-family-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
{tools}
"#
    )
}

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

    /// Send `method` and read until its answer, answering the gateway's
    /// roster and build callbacks on the way.
    fn rpc(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let start = Instant::now();
        while start.elapsed() < WEDGE_BACKSTOP {
            let remaining = WEDGE_BACKSTOP.saturating_sub(start.elapsed());
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                break;
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_str) == Some(id)
            {
                return message;
            }
            self.answer_callback(&message);
        }
        panic!("no answer to {method} within {WEDGE_BACKSTOP:?}");
    }

    /// Answer the roster callback with `agent:alpha`, a build callback with
    /// an empty draft and every other host callback with `null`.
    fn answer_callback(&mut self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
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
            "callback/build_agent" => json!({}),
            _ => Value::Null,
        };
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// `meta/init` on `state` for `mob_config`, as an identity-first host
    /// with the demo model.
    fn init(&mut self, state: &Path, mob_config: &str) {
        let init = self.rpc(
            "init",
            "meta/init",
            json!({
                "persistent_state": state,
                "mob_config": mob_config,
                "has_roster_provider": true,
                "has_session_builder": true,
                "runtime_options": {
                    "demo_llm": true,
                    "identity_bootstrap_mode": { "mode": "lazy_materialize" }
                }
            }),
        );
        assert!(
            init["result"]["contract_version"].is_string(),
            "init failed: {init}"
        );
    }

    /// Deliver one input to `agent:alpha`, so the member is live.
    fn dispatch(&mut self, id: &str) {
        let dispatch = self.rpc(
            id,
            "meta/dispatch",
            json!({
                "identity": "agent:alpha",
                "dispatch_input": { "content": "hello", "origin": "connector" }
            }),
        );
        assert!(
            dispatch.get("result").is_some(),
            "the dispatch is admitted so the member is live: {dispatch}"
        );
    }

    /// The names on `agent:alpha`'s live tool list.
    fn resolved_tools(&mut self, id: &str) -> Vec<String> {
        let resolved = self.rpc(
            id,
            "meta/identity/resolved_tools",
            json!({ "identity": "agent:alpha" }),
        );
        resolved["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("resolved_tools answers a tools array: {resolved}"))
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }

    /// Grant `agent:alpha` the shell family, deliver one more input and
    /// answer the grant with the member's tool list once it carries `shell`
    /// (a grant opens its names at the member's next model request), or the
    /// last list read when it never does.
    fn grant_shell_family(&mut self) -> (Value, Vec<String>) {
        self.grant(json!({"kind": "family", "family": "shell"}))
    }

    /// Grant `agent:alpha` the one tool `shell`, as
    /// [`Self::grant_shell_family`] grants the family.
    fn grant_the_tool_shell(&mut self) -> (Value, Vec<String>) {
        self.grant(json!({"kind": "tool", "name": "shell"}))
    }

    fn grant(&mut self, target: Value) -> (Value, Vec<String>) {
        let granted = self.rpc(
            "grant",
            "meta/tool_grants/grant",
            json!({
                "identity": "agent:alpha",
                "target": target,
                "duration_ms": 600_000,
            }),
        );
        assert!(granted["error"].is_null(), "the grant is issued: {granted}");
        self.dispatch("dispatch-after-grant");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut read = 0_u32;
        loop {
            read += 1;
            let names = self.resolved_tools(&format!("tools-after-grant-{read}"));
            if has(&names, "shell") || Instant::now() >= deadline {
                return (granted, names);
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// The graceful end of a persistent gateway: the shutdown handshake, then
    /// stdin EOF and a clean exit.
    fn shutdown(mut self) {
        let shutdown = self.rpc("shutdown", "meta/shutdown", json!({}));
        gateway_shutdown::assert_persistent_shutdown_completed(&shutdown, "shutdown");
        drop(self.stdin.take());
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                assert!(status.success(), "rpc_gateway exit status: {status}");
                return;
            }
            assert!(
                start.elapsed() < WEDGE_BACKSTOP,
                "rpc_gateway did not exit within {WEDGE_BACKSTOP:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Boot a gateway on `state` for a profile whose tools table carries `tools`,
/// make `agent:alpha` live and answer its tool list.
fn live_tools(state: &Path, tools: &str) -> (Gateway, Vec<String>) {
    let mut gateway = Gateway::start();
    gateway.init(state, &mob_config(tools));
    gateway.dispatch("dispatch");
    let names = gateway.resolved_tools("tools");
    (gateway, names)
}

fn has(names: &[String], name: &str) -> bool {
    names.iter().any(|listed| listed == name)
}

fn assert_lists(names: &[String], tools: &[&str], why: &str) {
    for tool in tools {
        assert!(has(names, tool), "{why}: `{tool}` is listed: {names:?}");
    }
}

fn assert_lists_none(names: &[String], tools: &[&str], why: &str) {
    for tool in tools {
        assert!(
            !has(names, tool),
            "{why}: `{tool}` is not listed: {names:?}"
        );
    }
}

/// Boot a gateway on `state`, whose pinned mob definition the operator
/// changed since the last run and declares as an update, make `agent:alpha`
/// live and answer its tool list.
fn live_tools_after_declared_update(state: &Path, tools: &str) -> (Gateway, Vec<String>) {
    let mut gateway = Gateway::start();
    let init = gateway.rpc(
        "init",
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": mob_config(tools),
            "has_roster_provider": true,
            "has_session_builder": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "lazy_materialize" },
                "declare_spec_update": { "expected_revision": 1 }
            }
        }),
    );
    assert!(
        init["result"]["contract_version"].is_string(),
        "init with a declared definition update failed: {init}"
    );
    gateway.dispatch("dispatch");
    let names = gateway.resolved_tools("tools");
    (gateway, names)
}

/// `shell = true` alone, with no other key, lists the whole family on the
/// member's live tool list, `monitor_start` included.
#[test]
fn a_shell_member_lists_the_whole_shell_family() {
    let state = tempfile::tempdir().expect("state dir");
    let (_gateway, names) = live_tools(state.path(), "shell = true");
    assert_lists(&names, &SHELL_FAMILY, "a shell profile");
}

/// The family is the profile's at every build: after the gateway stopped and
/// another process opened the same state, the resumed member lists it whole.
#[test]
fn a_resumed_shell_member_still_lists_the_whole_shell_family() {
    let state = tempfile::tempdir().expect("state dir");
    let (gateway, _) = live_tools(state.path(), "shell = true");
    gateway.shutdown();

    let (_gateway, names) = live_tools(state.path(), "shell = true");
    assert_lists(&names, &SHELL_FAMILY, "a resumed shell profile");
}

/// `deny = ["monitor_start"]` keeps the tool list a shell member had before
/// the family carried `monitor_start`: the four other tools, and no monitor.
#[test]
fn a_profile_that_denies_monitor_start_lists_the_four_other_shell_tools() {
    let state = tempfile::tempdir().expect("state dir");
    let (_gateway, names) = live_tools(state.path(), "shell = true\ndeny = [\"monitor_start\"]");
    assert_lists(&names, &SHELL_FAMILY[..4], "the deny names one tool");
    assert_lists_none(&names, &[MONITOR], "a denied tool");
}

/// The deny is the profile's at every build too: a member that had the whole
/// family lists no `monitor_start` once its profile denies it and the gateway
/// opened the same state again.
#[test]
fn a_deny_added_later_removes_monitor_start_from_a_resumed_member() {
    let state = tempfile::tempdir().expect("state dir");
    let (gateway, before) = live_tools(state.path(), "shell = true");
    assert_lists(&before, &SHELL_FAMILY, "a shell profile");
    gateway.shutdown();

    let (_gateway, names) =
        live_tools_after_declared_update(state.path(), "shell = true\ndeny = [\"monitor_start\"]");
    assert_lists(&names, &SHELL_FAMILY[..4], "the deny names one tool");
    assert_lists_none(&names, &[MONITOR], "a tool denied since the last run");
}

/// A grant-gated shell family is mounted closed, `monitor_start` like its
/// other tools, and a grant of the family opens all five.
#[test]
fn a_shell_family_grant_opens_all_five_shell_tools() {
    let state = tempfile::tempdir().expect("state dir");
    let (mut gateway, closed) =
        live_tools(state.path(), "grant_gated = { families = [\"shell\"] }");
    assert_lists_none(&closed, &SHELL_FAMILY, "a gated family before its grant");

    let (granted, open) = gateway.grant_shell_family();
    assert!(
        has(&open, "shell"),
        "the family grant opens the shell tools: {open:?}; grant {granted}"
    );
    assert_lists(&open, &SHELL_FAMILY, "a granted shell family");
}

/// A read-only member lists `monitor_start` as it lists `shell`: read-only
/// hides no tool, it refuses each call of a tool that is not read-only.
#[test]
fn a_read_only_shell_member_lists_monitor_start_as_it_lists_shell() {
    let state = tempfile::tempdir().expect("state dir");
    let (_gateway, names) = live_tools(state.path(), "shell = true\nread_only = true");
    assert_lists(&names, &SHELL_FAMILY, "a read-only shell profile");
}

/// A member without `shell` has no tool of the family.
#[test]
fn a_member_without_shell_lists_no_shell_tool() {
    let state = tempfile::tempdir().expect("state dir");
    let (_gateway, names) = live_tools(state.path(), "");
    assert_lists_none(&names, &SHELL_FAMILY, "a profile without shell");
}

/// `monitor_start` is never wider than `shell`: a profile whose deny list
/// names only `shell` lists the family's three job tools and neither `shell`
/// nor `monitor_start`.
#[test]
fn a_profile_that_denies_shell_lists_no_monitor_start() {
    let state = tempfile::tempdir().expect("state dir");
    let (_gateway, names) = live_tools(state.path(), "shell = true\ndeny = [\"shell\"]");
    assert_lists(&names, &SHELL_FAMILY[1..4], "the deny names `shell`");
    assert_lists_none(&names, &["shell", MONITOR], "a denied `shell`");
}

/// A restart does not bring it back: the resumed member lists the same.
#[test]
fn a_resumed_member_whose_profile_denies_shell_lists_no_monitor_start() {
    let state = tempfile::tempdir().expect("state dir");
    let tools = "shell = true\ndeny = [\"shell\"]";
    let (gateway, _) = live_tools(state.path(), tools);
    gateway.shutdown();

    let (_gateway, names) = live_tools(state.path(), tools);
    assert_lists(
        &names,
        &SHELL_FAMILY[1..4],
        "the deny names `shell`, resumed",
    );
    assert_lists_none(&names, &["shell", MONITOR], "a denied `shell`, resumed");
}

/// A grant gate on the name `shell` (not on the family) gates
/// `monitor_start` with it: neither is listed before a grant, and the grant
/// of the tool `shell` opens both.
#[test]
fn a_grant_gate_on_the_name_shell_gates_monitor_start_with_it() {
    let state = tempfile::tempdir().expect("state dir");
    let (mut gateway, closed) = live_tools(
        state.path(),
        "shell = true\ngrant_gated = { tools = [\"shell\"] }",
    );
    assert_lists_none(
        &closed,
        &["shell", MONITOR],
        "a gate on the name `shell` before its grant",
    );

    let (granted, open) = gateway.grant_the_tool_shell();
    assert!(
        has(&open, "shell"),
        "the grant of the tool opens `shell`: {open:?}; grant {granted}"
    );
    assert_lists(&open, &SHELL_FAMILY, "the tool `shell` granted");
}
