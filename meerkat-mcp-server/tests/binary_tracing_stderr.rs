//! The shipped `rkat-mcp` binary installs a stderr tracing subscriber.
//!
//! Until it did, every `tracing::warn!`/`error!` in the process was dropped:
//! an invalid realm config fell back to defaults with no output, and the
//! documented `verbose` event logging never appeared anywhere. These tests
//! drive the real binary over stdio and read what reaches each stream: trace
//! events on stderr (at the `info` default and under a `RUST_LOG` override)
//! and only JSON on stdout, which is the MCP channel.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

/// Cargo sets `CARGO_BIN_EXE_rkat-mcp` for every integration test of the
/// package that owns the binary, and the generated Bazel test targets wire the
/// same variable to `$(rootpath //meerkat-mcp-server:rkat_mcp_bin)`
/// (`scripts/generate-bazel-rust-builds.mjs`). An unset variable is a harness
/// fault in every runner and fails loudly; there is no skip arm, so a green run
/// always means the binary was exercised. The path is canonicalized because
/// Bazel hands out a runfiles-relative path and the server is spawned with a
/// different working directory.
fn rkat_mcp_binary() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("CARGO_BIN_EXE_rkat-mcp")
            .expect("CARGO_BIN_EXE_rkat-mcp is set by cargo and by the generated Bazel targets"),
    );
    path.canonicalize()
        .unwrap_or_else(|err| panic!("rkat-mcp binary {} is not reachable: {err}", path.display()))
}

struct ServerRun {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

/// Start the binary against a private state root, send one `initialize`
/// request, close stdin so the server exits on its own, and collect both
/// streams. `rust_log` is applied as the process's `RUST_LOG`; `None` removes
/// any value inherited from the test runner so the default filter is measured.
async fn run_initialize(
    binary: &Path,
    home: &Path,
    realm: &str,
    rust_log: Option<&str>,
) -> ServerRun {
    let state_root = home.join("state");
    let project = home.join("project");
    tokio::fs::create_dir_all(&state_root).await.unwrap();
    tokio::fs::create_dir_all(&project).await.unwrap();

    let mut command = Command::new(binary);
    command
        .current_dir(&project)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("RUST_LOG")
        .args([
            "--realm",
            realm,
            "--realm-backend",
            "memory",
            "--state-root",
            state_root.to_str().unwrap(),
            "--context-root",
            project.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(rust_log) = rust_log {
        command.env("RUST_LOG", rust_log);
    }
    let mut child = command.spawn().expect("spawn rkat-mcp");

    let mut stdin = child.stdin.take().expect("child stdin");
    let mut line = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }))
    .unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.unwrap();
    stdin.shutdown().await.unwrap();
    drop(stdin);

    let output = timeout(Duration::from_secs(180), child.wait_with_output())
        .await
        .expect("rkat-mcp exits once stdin closes")
        .expect("rkat-mcp wait");
    ServerRun {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// stdout is the MCP channel: every line must be JSON, the `initialize`
/// response must be among them, and no trace event may leak into it.
fn assert_stdout_is_only_json(run: &ServerRun) {
    let mut initialize_responses = 0;
    for line in run.stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("non-JSON line on stdout ({err}): {line:?}"));
        if value.get("id") == Some(&json!(1)) {
            assert_eq!(
                value.pointer("/result/serverInfo/name"),
                Some(&json!("rkat-mcp")),
                "initialize response: {value}"
            );
            initialize_responses += 1;
        }
    }
    assert_eq!(
        initialize_responses, 1,
        "exactly one initialize response on stdout:\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("resolved realm storage"),
        "trace event leaked onto the MCP stdout channel:\n{}",
        run.stdout
    );
}

/// With `RUST_LOG` unset the binary logs at `info`: the server's own startup
/// event (emitted by the library at INFO) reaches stderr, and stdout stays
/// pure JSON.
#[tokio::test]
async fn default_filter_writes_info_events_to_stderr_and_keeps_stdout_json() {
    let binary = rkat_mcp_binary();
    let temp = TempDir::new().unwrap();
    let realm = "tracing_default_realm";

    let run = run_initialize(&binary, temp.path(), realm, None).await;

    assert!(
        run.status.success(),
        "rkat-mcp exited {:?}; stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("resolved realm storage"),
        "startup INFO event must reach stderr under the default filter; stderr:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("INFO") && run.stderr.contains(&format!("realm={realm}")),
        "stderr must carry the formatted event with its level and fields; stderr:\n{}",
        run.stderr
    );
    assert_stdout_is_only_json(&run);
}

/// `RUST_LOG` replaces the default instead of layering on it: at `warn` the
/// startup INFO event is gone from stderr while stdout stays pure JSON. (An
/// invalid realm config is no longer a usable WARN trigger here: since #1092
/// the server refuses to start on an unloadable head config instead of falling
/// back to defaults, so this test keeps the realm valid.)
#[tokio::test]
async fn rust_log_overrides_the_default_filter() {
    let binary = rkat_mcp_binary();
    let temp = TempDir::new().unwrap();
    let realm = "tracing_override_realm";

    let run = run_initialize(&binary, temp.path(), realm, Some("warn")).await;

    assert!(
        run.status.success(),
        "rkat-mcp exited {:?}; stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        !run.stderr.contains("resolved realm storage"),
        "RUST_LOG=warn must suppress the INFO default; stderr:\n{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("ignoring RUST_LOG"),
        "a parsable RUST_LOG must not be reported as ignored; stderr:\n{}",
        run.stderr
    );
    assert_stdout_is_only_json(&run);
}

/// An unparsable `RUST_LOG` is named on stderr and the `info` default applies,
/// so an operator's typo is neither honoured nor silently dropped: the startup
/// INFO event is present again and stdout stays pure JSON.
#[tokio::test]
async fn unparsable_rust_log_is_named_and_the_default_applies() {
    let binary = rkat_mcp_binary();
    let temp = TempDir::new().unwrap();
    let realm = "tracing_unparsable_realm";

    let run = run_initialize(&binary, temp.path(), realm, Some("meerkat_mcp_server=loud")).await;

    assert!(
        run.status.success(),
        "rkat-mcp exited {:?}; stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("rkat-mcp: ignoring RUST_LOG=")
            && run.stderr.contains("meerkat_mcp_server=loud"),
        "the rejected RUST_LOG value must be named on stderr; stderr:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("resolved realm storage"),
        "the info default must apply after an unparsable RUST_LOG; stderr:\n{}",
        run.stderr
    );
    assert_stdout_is_only_json(&run);
}
