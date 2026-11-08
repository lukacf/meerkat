#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! The stdio gateway's launch option for scoped tool grants
//! (`runtime_options.tool_grants = {max_lifetime_ms}`): the host selects the
//! longest window a grant may have, set as the runtime's grant policy at
//! init. Without it the default maximum (24 hours) stands; a value that is
//! not a positive whole number of milliseconds within 2^53 - 1 refuses init.
//! Each case is a `rpc_gateway` process on a pipe, with an identity roster
//! the test answers as the SDK host.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const IDENTITY: &str = "agent-a";
const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-tool-grants-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
builtins = true
grant_gated = { tools = ["datetime"] }
"#;
const LINE_WAIT: Duration = Duration::from_mins(3);
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// One `rpc_gateway` process on a pipe. Requests go to its stdin; a reader
/// thread hands every stdout line to the test, and the test answers the
/// gateway's roster callback as the SDK host.
struct Gateway {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    next_id: u64,
}

impl Gateway {
    fn start(scratch: &Path) -> Self {
        let stderr = File::create(scratch.join(format!(
            "gateway-{}.stderr",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        )))
        .expect("gateway stderr file");
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", scratch.join("xdg-state"))
            .current_dir(scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write");
        self.stdin.flush().expect("flush");
    }

    /// Send `method` and read stdout until its answer, answering the
    /// gateway's roster callback on the way.
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = format!("grants-{}", self.next_id);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(LINE_WAIT)
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let Some(message) = line
                .find('{')
                .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
            else {
                continue;
            };
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return message;
            }
            if let (Some(callback), Some(callback_id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let reply = match callback {
                    "callback/roster_provider/roster" => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "result": [{"identity": IDENTITY, "profile": "worker"}],
                    }),
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "error": {"code": -32601, "message": format!("no handler for {other}")},
                    }),
                };
                self.send(&reply);
            }
        }
    }

    /// `meta/init` on `state` with the identity roster and `runtime_options`.
    fn init(&mut self, state: &Path, runtime_options: Value) -> Value {
        self.rpc(
            "meta/init",
            json!({
                "mob_config": MOB_CONFIG,
                "persistent_state": state,
                "has_roster_provider": true,
                "runtime_options": runtime_options,
            }),
        )
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn grant(gateway: &mut Gateway, duration_ms: u64) -> Value {
    gateway.rpc(
        "meta/tool_grants/grant",
        json!({
            "identity": IDENTITY,
            "target": {"kind": "tool", "name": "datetime"},
            "duration_ms": duration_ms,
        }),
    )
}

/// A launch selecting a 400-day maximum admits a 365-day grant.
#[test]
fn a_launch_maximum_lifetime_admits_a_grant_longer_than_the_default() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path());
    let init = gateway.init(
        &scratch.path().join("state"),
        json!({"demo_llm": true, "tool_grants": {"max_lifetime_ms": 400 * DAY_MS}}),
    );
    assert!(
        init["error"].is_null() && init["result"]["contract_version"].is_string(),
        "the launch option is accepted: {init:#}"
    );
    let granted = grant(&mut gateway, 365 * DAY_MS);
    assert!(
        granted["error"].is_null(),
        "a 365-day grant fits the launch maximum: {granted:#}"
    );
    assert_eq!(granted["result"]["grant"]["status"], json!("active"));
}

/// Without the option the default maximum (24 hours) stands: a 25-hour grant
/// is `invalid_window`.
#[test]
fn without_the_option_a_grant_over_a_day_is_an_invalid_window() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path());
    let init = gateway.init(&scratch.path().join("state"), json!({"demo_llm": true}));
    assert!(init["error"].is_null(), "init: {init:#}");
    let refused = grant(&mut gateway, 25 * HOUR_MS);
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused:#}");
    assert_eq!(
        refused["error"]["data"],
        json!({
            "kind": "tool_grant_request_refused",
            "tool_grant_request_refusal": {"kind": "invalid_window"},
        }),
        "{refused:#}"
    );
}

/// A maximum that is not a positive whole number of milliseconds within
/// 2^53 - 1 refuses init, typed.
#[test]
fn an_invalid_maximum_lifetime_refuses_init() {
    for max_lifetime_ms in [json!(0), json!(9_007_199_254_740_992_u64), json!(1.5)] {
        let scratch = tempfile::tempdir().unwrap();
        let mut gateway = Gateway::start(scratch.path());
        let init = gateway.init(
            &scratch.path().join("state"),
            json!({"demo_llm": true, "tool_grants": {"max_lifetime_ms": max_lifetime_ms}}),
        );
        assert_eq!(init["error"]["code"], json!(-32602), "{init:#}");
        assert_eq!(
            init["error"]["data"]["kind"],
            json!("tool_grant_lifetime_invalid"),
            "{max_lifetime_ms}: {init:#}"
        );
    }
}
