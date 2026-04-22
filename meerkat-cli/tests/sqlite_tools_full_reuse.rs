#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;

use meerkat::Config;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        let debug = target_dir.join("debug/rkat");
        if debug.exists() {
            return Some(debug);
        }
        let release = target_dir.join("release/rkat");
        if release.exists() {
            return Some(release);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let codex_debug = workspace_root.join("target-codex/debug/rkat");
    if codex_debug.exists() {
        return Some(codex_debug);
    }
    let codex_release = workspace_root.join("target-codex/release/rkat");
    if codex_release.exists() {
        return Some(codex_release);
    }
    let debug = workspace_root.join("target/debug/rkat");
    if debug.exists() {
        return Some(debug);
    }
    let release = workspace_root.join("target/release/rkat");
    if release.exists() {
        return Some(release);
    }
    None
}

async fn write_test_config(
    project_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let rkat_dir = project_dir.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;

    let mut config = Config::default();
    config.agent.max_tokens_per_turn = 128;
    let config_toml = toml::to_string_pretty(&config)?;
    tokio::fs::write(rkat_dir.join("config.toml"), config_toml).await?;
    Ok(())
}

#[tokio::test]
async fn run_tools_full_sqlite_reuses_open_realm() -> Result<(), Box<dyn std::error::Error>> {
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(&project_dir).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;

    write_test_config(&project_dir).await?;

    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("HOME", temp_dir.path())
            .env("XDG_DATA_HOME", &data_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "--realm-backend",
                "sqlite",
                "run",
                "hello",
                "--tools",
                "full",
                "--output",
                "json",
            ])
            .output(),
    )
    .await??;

    assert!(
        output.status.success(),
        "rkat run with --tools full and sqlite backend failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("failed to parse JSON output: {e}\nstdout: {stdout}"))?;
    assert!(
        parsed["session_id"].as_str().is_some(),
        "expected session_id in CLI JSON output, got: {parsed}"
    );

    Ok(())
}
