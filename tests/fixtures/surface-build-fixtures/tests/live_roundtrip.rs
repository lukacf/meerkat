#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

fn binary_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(format!("CARGO_BIN_EXE_{name}")).map(PathBuf::from)
}

async fn run_fixture(binary: &str) -> Result<String, Box<dyn std::error::Error>> {
    let Some(path) = binary_path(binary) else {
        return Err(format!("missing fixture binary {binary}").into());
    };
    let output = timeout(
        Duration::from_secs(180),
        Command::new(path)
            .arg("basic-roundtrip")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await??;
    if !output.status.success() {
        return Err(format!(
            "{binary} failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_embedded_min_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = run_fixture("embedded_min").await?;
    let payload: serde_json::Value = serde_json::from_str(stdout.trim())?;
    assert_eq!(payload["fixture"], "embedded_min");
    assert!(payload["session_id"].is_string());
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_runtime_backed_min_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = run_fixture("runtime_backed_min").await?;
    let payload: serde_json::Value = serde_json::from_str(stdout.trim())?;
    assert_eq!(payload["fixture"], "runtime_backed_min");
    assert!(payload["session_id"].is_string());
    Ok(())
}
