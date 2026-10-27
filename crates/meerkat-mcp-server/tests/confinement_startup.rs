//! What the stock `rkat-mcp` binary reads at startup to resolve its
//! `--confinement*` flags. A server started without a flag that names a
//! relative path never reads its working directory, so it starts and serves
//! even when that directory was removed.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

/// Cargo sets `CARGO_BIN_EXE_rkat-mcp` for every integration test of the
/// package that owns the binary, and the generated Bazel test targets wire
/// the same variable. The path is canonicalized because the server is
/// spawned from another working directory.
fn rkat_mcp_binary() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("CARGO_BIN_EXE_rkat-mcp")
            .expect("CARGO_BIN_EXE_rkat-mcp is set by cargo and by the generated Bazel targets"),
    );
    path.canonicalize()
        .unwrap_or_else(|err| panic!("rkat-mcp binary {} is not reachable: {err}", path.display()))
}

#[tokio::test]
async fn rkat_mcp_starts_without_a_working_directory_when_no_flag_needs_it() {
    let binary = rkat_mcp_binary();
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    let removed = temp.path().join("removed");
    for dir in [&state_root, &user_root, &removed] {
        std::fs::create_dir_all(dir).unwrap();
    }

    // The shell removes its working directory, then becomes the server.
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("cd \"$1\" && rmdir \"$1\" && shift && exec \"$@\"")
        .arg("sh")
        .arg(&removed)
        .arg(&binary)
        .args([
            "--realm",
            "mcp-removed-cwd",
            "--realm-backend",
            "memory",
            "--state-root",
            state_root.to_str().unwrap(),
            "--user-config-root",
            user_root.to_str().unwrap(),
        ])
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rkat-mcp");

    let mut stdin = child.stdin.take().expect("child stdin");
    let mut line = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }))
    .unwrap();
    line.push('\n');
    // A server that refused to start may already have closed its stdin.
    let _ = stdin.write_all(line.as_bytes()).await;
    let _ = stdin.shutdown().await;
    drop(stdin);

    let output = timeout(Duration::from_mins(3), child.wait_with_output())
        .await
        .expect("rkat-mcp exits once stdin closes")
        .expect("rkat-mcp wait");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let answered = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|message| {
            message.get("id") == Some(&json!(1))
                && message.pointer("/result/serverInfo/name") == Some(&json!("rkat-mcp"))
        });
    assert!(
        answered,
        "a server without confinement flags serves from a removed working directory; \
         status={:?} stdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
}
