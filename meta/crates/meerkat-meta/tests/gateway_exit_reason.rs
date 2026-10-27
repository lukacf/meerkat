//! Gateway exit diagnosability: a gateway process that ends must say WHY on
//! stderr, through the tracing stream, before it runs its shutdown sequence
//! and again after. The line carries `reason=<token>` for the `select!`
//! branch that ended the run loop and `signal=<NAME>` when a signal did.
//!
//! Before this, both binaries exited silently on every designed path (stdin
//! EOF, SIGINT, SIGTERM, the SDK shutdown handshake), so a report of "the
//! gateway process exited, all endpoints 000" could not be told apart from a
//! crash, a supervisor's SIGTERM after a slow console operation, or the
//! launching process closing the pipe. Panics are covered separately by the
//! hook test in `gateway_composition`; a real gateway cannot be made to panic
//! on demand from outside.
//!
//! The tests drive the REAL binaries with `RUST_LOG` unset, so they also
//! prove the default filter surfaces the lines: an operator who never set
//! `RUST_LOG` must see them.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;
use tempfile::TempDir;

/// Hang detector, not a latency bound: every wait here guards "the line
/// arrives or the gateway is wedged". Sized like the sibling gateway
/// subprocess suites so a loaded runner does not manufacture a failure.
const BACKSTOP: Duration = Duration::from_mins(1);

/// The `meta/init` answer gets the wider bound `gateway_schedule_lease_release`
/// documents for the same boot: 45s timed out under full-suite load once
/// already, and 60s did here (four boots that never printed a line in 60s
/// while sibling builds saturated the machine). Worst case per test stays
/// under nextest's 300s kill: 90 init + 60 probe + 60 exit + 60 line.
const INIT_BACKSTOP: Duration = Duration::from_secs(90);

const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-exit-reason-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
"#;

#[derive(Clone, Copy)]
enum Binary {
    /// The SDK stdin-RPC gateway: exits on stdin EOF by design.
    Rpc,
    /// The console/HTTP gateway: outlives its stdin by design.
    Meta,
}

impl Binary {
    fn label(self) -> &'static str {
        match self {
            Self::Rpc => "rpc_gateway",
            Self::Meta => "meta_gateway",
        }
    }
}

struct Gateway {
    binary: Binary,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_lines: mpsc::Receiver<String>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    _workspace: TempDir,
    _state: TempDir,
}

impl Gateway {
    /// Spawn the real binary, send `meta/init`, and wait for its `result`.
    fn spawn(binary: Binary) -> Self {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let state = tempfile::tempdir().expect("state tempdir");
        let (bin, args, params): (&str, &[&str], Value) = match binary {
            Binary::Rpc => (
                env!("CARGO_BIN_EXE_rpc_gateway"),
                &["--persistent"],
                json!({
                    "persistent_state": state.path(),
                    "mob_config": MOB_CONFIG,
                }),
            ),
            Binary::Meta => (
                env!("CARGO_BIN_EXE_meta_gateway"),
                &[],
                json!({
                    "workspace_root": workspace.path(),
                    "store_path": state.path().join("store"),
                }),
            ),
        };
        let mut child = Command::new(bin)
            .args(args)
            .current_dir(workspace.path())
            // The exit line must pass the DEFAULT filter; a developer's
            // `RUST_LOG=warn` in the environment would hide it and make this
            // test measure the environment instead of the binary.
            .env_remove("RUST_LOG")
            // Created at bootstrap, never called: the fallback mob's agent
            // needs a secret present, so placeholders keep this CI-safe.
            .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
            .env("OPENAI_API_KEY", "sk-regression-test")
            // The default ~/.local/state/meerkat-meta is shared across
            // concurrent test processes and intermittently fails the peer-key
            // mint.
            .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("[{}] spawn: {error}", binary.label()));

        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let stderr = child.stderr.take().expect("gateway stderr");

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

        let mut gateway = Self {
            binary,
            child,
            stdin: Some(stdin),
            stdout_lines,
            stderr_lines,
            _workspace: workspace,
            _state: state,
        };
        gateway.send(json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "meta/init",
            "params": params,
        }));
        let init = gateway
            .wait_for_response("init", INIT_BACKSTOP)
            .unwrap_or_else(|| {
                panic!(
                    "[{}] no meta/init answer within {INIT_BACKSTOP:?}\nstderr:\n{}",
                    binary.label(),
                    gateway.stderr_snapshot()
                )
            });
        assert!(
            init.get("result").is_some(),
            "[{}] meta/init failed: {init}",
            binary.label()
        );
        gateway
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        writeln!(stdin, "{}", serde_json::to_string(&value).expect("json")).expect("write");
        stdin.flush().expect("flush");
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    /// Deliver `kill -<name>` (INT or TERM) to the gateway.
    #[cfg(unix)]
    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .args([format!("-{name}"), self.child.id().to_string()])
            .status()
            .expect("run kill");
        assert!(
            status.success(),
            "[{}] kill -{name} failed: {status}",
            self.binary.label()
        );
    }

    fn wait_for_response(&mut self, id: &str, deadline: Duration) -> Option<Value> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            let remaining = deadline.saturating_sub(start.elapsed());
            let Ok(line) = self.stdout_lines.recv_timeout(remaining) else {
                return None;
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_str) == Some(id)
            {
                return Some(message);
            }
        }
        None
    }

    /// Prove the run loop is serving before signalling, so a row reads the
    /// exit of a gateway that was fully up. Both gateways listen for SIGINT
    /// and SIGTERM from before their init response, so a signal sent right
    /// after it is no longer lost to the default disposition (it once killed
    /// a gateway here on a loaded runner); the `..._right_after_init` rows
    /// send theirs without this probe. For rpc_gateway a request answered by
    /// the dispatch loop proves the loop; for meta_gateway the stdin guard's
    /// -32601 refusal of any post-init line does.
    fn probe_select_loop(&mut self) {
        let method = match self.binary {
            Binary::Rpc => "meta/status",
            Binary::Meta => "meta/probe",
        };
        self.send(json!({
            "jsonrpc": "2.0",
            "id": "probe",
            "method": method,
            "params": {}
        }));
        self.wait_for_response("probe", BACKSTOP)
            .unwrap_or_else(|| {
                panic!(
                    "[{}] no answer to the select-loop probe within {BACKSTOP:?}\nstderr:\n{}",
                    self.binary.label(),
                    self.stderr_snapshot()
                )
            });
    }

    fn is_running(&mut self) -> bool {
        self.child.try_wait().expect("poll gateway").is_none()
    }

    fn wait_for_exit(&mut self, deadline: Duration) -> ExitStatus {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                return status;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        panic!(
            "[{}] did not exit within {deadline:?}\nstderr:\n{}",
            self.binary.label(),
            self.stderr_snapshot()
        );
    }

    fn stderr_snapshot(&self) -> String {
        self.stderr_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("\n")
    }

    /// The first stderr line containing `needle`, waiting up to `deadline`
    /// for it: the pipe drains on a separate thread, so a line the gateway
    /// wrote just before exiting may land a moment after `try_wait` says it
    /// is gone.
    fn wait_for_stderr(&self, needle: &str, deadline: Duration) -> Option<String> {
        let start = Instant::now();
        loop {
            let found = self
                .stderr_lines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|line| line.contains(needle))
                .cloned();
            if found.is_some() || start.elapsed() >= deadline {
                return found;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Assert the run-loop exit line and its closing bookend, and return the
    /// exit line for reason/signal assertions.
    fn assert_exit_lines(&self) -> String {
        let label = self.binary.label();
        let (ended, complete) = match self.binary {
            Binary::Rpc => (
                "rpc_gateway dispatch loop ended; running graceful shutdown",
                "rpc_gateway shutdown complete; exiting",
            ),
            Binary::Meta => (
                "meta_gateway run loop ended; running graceful shutdown",
                "meta_gateway shutdown complete; exiting",
            ),
        };
        let exit_line = self.wait_for_stderr(ended, BACKSTOP).unwrap_or_else(|| {
            panic!(
                "[{label}] no exit-reason line ({ended:?}) on stderr\nstderr:\n{}",
                self.stderr_snapshot()
            )
        });
        assert!(
            exit_line.contains(" INFO "),
            "[{label}] the exit line must be at INFO so the default filter shows it: {exit_line}"
        );
        assert!(
            self.wait_for_stderr(complete, BACKSTOP).is_some(),
            "[{label}] no shutdown-complete bookend ({complete:?}) on stderr\nstderr:\n{}",
            self.stderr_snapshot()
        );
        exit_line
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_reason(line: &str, reason: &str) {
    assert!(
        line.contains(&format!("reason={reason}")),
        "exit line does not carry reason={reason}: {line}"
    );
}

fn assert_signal(line: &str, name: &str) {
    assert!(
        line.contains(&format!("signal={name}")),
        "exit line does not name the signal {name}: {line}"
    );
}

#[test]
fn rpc_gateway_names_stdin_eof_as_its_exit_reason() {
    let mut gateway = Gateway::spawn(Binary::Rpc);
    gateway.close_stdin();
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");

    assert!(
        gateway
            .wait_for_stderr("stdin reached EOF", BACKSTOP)
            .is_some(),
        "the stdin reader must name the EOF before the loop's exit line\nstderr:\n{}",
        gateway.stderr_snapshot()
    );
    let line = gateway.assert_exit_lines();
    assert_reason(&line, "stdin_closed");
    assert!(
        !line.contains("signal="),
        "an EOF exit must not claim a signal: {line}"
    );
}

#[cfg(unix)]
#[test]
fn rpc_gateway_names_sigint_as_its_exit_reason() {
    let mut gateway = Gateway::spawn(Binary::Rpc);
    gateway.probe_select_loop();
    gateway.signal("INT");
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "signal");
    assert_signal(&line, "SIGINT");
}

#[cfg(unix)]
#[test]
fn rpc_gateway_names_sigterm_as_its_exit_reason() {
    let mut gateway = Gateway::spawn(Binary::Rpc);
    gateway.probe_select_loop();
    gateway.signal("TERM");
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "signal");
    assert_signal(&line, "SIGTERM");
}

/// A SIGTERM sent the moment the init response arrives, before the
/// dispatch loop has waited even once, still runs the graceful shutdown:
/// the gateway listens for it from before it answers init.
#[cfg(unix)]
#[test]
fn rpc_gateway_shuts_down_gracefully_on_a_sigterm_right_after_init() {
    let mut gateway = Gateway::spawn(Binary::Rpc);
    gateway.signal("TERM");
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "signal");
    assert_signal(&line, "SIGTERM");
}

#[test]
fn rpc_gateway_names_the_sdk_shutdown_handshake_as_its_exit_reason() {
    let mut gateway = Gateway::spawn(Binary::Rpc);
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "shutdown",
        "method": "meta/shutdown",
        "params": {}
    }));
    let response = gateway
        .wait_for_response("shutdown", BACKSTOP)
        .expect("shutdown handshake response");
    // Persistent stock gateway: its durable child root is released, so
    // cleanup is complete.
    gateway_shutdown::assert_persistent_shutdown_completed(&response, "sdk shutdown handshake");
    // Match SDK behaviour: close stdin only after the gateway confirms.
    gateway.close_stdin();
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "rpc_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "sdk_shutdown_handshake");
    assert!(
        !line.contains("signal="),
        "a handshake exit must not claim a signal: {line}"
    );
}

/// `meta_gateway` outlives its stdin by design; the log must say so at the
/// EOF, and a later SIGTERM must be attributed to the signal, not the EOF.
#[cfg(unix)]
#[test]
fn meta_gateway_survives_stdin_eof_and_names_sigterm_as_its_exit_reason() {
    let mut gateway = Gateway::spawn(Binary::Meta);
    gateway.close_stdin();
    assert!(
        gateway
            .wait_for_stderr("stdin closed by the launching process", BACKSTOP)
            .is_some(),
        "meta_gateway must log the stdin close it survives\nstderr:\n{}",
        gateway.stderr_snapshot()
    );
    assert!(
        gateway.is_running(),
        "meta_gateway must keep serving after stdin EOF"
    );

    gateway.signal("TERM");
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "meta_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "signal");
    assert_signal(&line, "SIGTERM");
}

/// A SIGTERM sent the moment the init response arrives, before the run
/// loop has waited even once, still runs the graceful shutdown: the gateway
/// listens for it from before it prints the init response.
#[cfg(unix)]
#[test]
fn meta_gateway_shuts_down_gracefully_on_a_sigterm_right_after_init() {
    let mut gateway = Gateway::spawn(Binary::Meta);
    gateway.signal("TERM");
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "meta_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "signal");
    assert_signal(&line, "SIGTERM");
}

#[cfg(unix)]
#[test]
fn meta_gateway_names_sigint_as_its_exit_reason() {
    // stdin stays OPEN throughout: the operator ctrl-c shape. Before the
    // explicit exit on the signal path, the binary logged "exiting" and then
    // hung in the runtime destructor on the blocking stdin read forever.
    let mut gateway = Gateway::spawn(Binary::Meta);
    gateway.probe_select_loop();
    gateway.signal("INT");
    let status = gateway.wait_for_exit(BACKSTOP);
    assert!(status.success(), "meta_gateway exit status: {status}");

    let line = gateway.assert_exit_lines();
    assert_reason(&line, "signal");
    assert_signal(&line, "SIGINT");
}
