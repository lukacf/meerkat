#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use tempfile::TempDir;

fn rkat_mini_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat-mini") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        let debug = target_dir.join("debug/rkat-mini");
        if debug.exists() {
            return Some(debug);
        }
        let release = target_dir.join("release/rkat-mini");
        if release.exists() {
            return Some(release);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let debug = workspace_root.join("target/debug/rkat-mini");
    if debug.exists() {
        return Some(debug);
    }
    let release = workspace_root.join("target/release/rkat-mini");
    if release.exists() {
        return Some(release);
    }
    None
}

fn skip_if_no_prereqs() -> bool {
    if rkat_mini_binary_path().is_some() {
        return false;
    }

    eprintln!("Skipping: missing rkat-mini binary");
    true
}

#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn integration_real_rkat_mini_surface() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let rkat_mini = rkat_mini_binary_path().ok_or("rkat-mini binary not found")?;

    let help = Command::new(&rkat_mini).arg("-h").output().await?;
    assert!(help.status.success(), "rkat-mini -h failed");
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(help_stdout.contains("run"));
    assert!(help_stdout.contains("skill"));
    assert!(!help_stdout.contains("mob"));
    assert!(!help_stdout.contains("mcp"));
    assert!(help_stdout.contains("blob"));

    let mob_help = Command::new(&rkat_mini)
        .args(["mob", "--help"])
        .output()
        .await?;
    assert!(
        !mob_help.status.success(),
        "rkat-mini should not accept mob commands"
    );

    let run = timeout(
        Duration::from_secs(120),
        Command::new(&rkat_mini)
            .current_dir(&project_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "run",
                "Say the word 'ok' and nothing else.",
                "--tools",
                "workspace",
                "--output",
                "json",
            ])
            .output(),
    )
    .await??;

    assert!(
        run.status.success(),
        "rkat-mini run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())?;
    assert_eq!(parsed["text"].as_str().unwrap_or("").trim(), "ok");
    assert!(parsed["session_id"].is_string());

    Ok(())
}
