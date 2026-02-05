#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use meerkat::{Config, SessionId};
use meerkat_store::{JsonlStore, SessionStore};
use tempfile::TempDir;

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
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

fn skip_if_no_prereqs() -> bool {
    let mut missing = Vec::new();

    if rkat_binary_path().is_none() {
        missing.push("rkat binary (build with cargo build -p meerkat-cli)");
    }

    if missing.is_empty() {
        return false;
    }

    eprintln!("Skipping: missing {}", missing.join(" and "));
    true
}

#[tokio::test]
#[ignore = "integration-real: spawns rkat binary"]
async fn integration_real_cli_resume_tools() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }

    if std::env::var("RUN_TEST_CLI_RESUME_INNER").is_ok() {
        return inner_test_cli_resume_tools().await;
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;

    let status = Command::new(std::env::current_exe()?)
        .arg("e2e_cli_resume_tools")
        .env("RUN_TEST_CLI_RESUME_INNER", "1")
        .env("HOME", temp_dir.path())
        .env("XDG_DATA_HOME", &data_dir)
        .env("TEST_PROJECT_DIR", &project_dir)
        .env("TEST_DATA_DIR", &data_dir)
        .status()
        .await?;

    assert!(status.success());
    Ok(())
}

async fn inner_test_cli_resume_tools() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = std::env::var("TEST_PROJECT_DIR")?;
    let data_dir = std::env::var("TEST_DATA_DIR")?;
    let project_dir = std::path::PathBuf::from(project_dir);
    let data_dir = std::path::PathBuf::from(data_dir);

    // Change to project dir so .rkat is found
    std::env::set_current_dir(&project_dir)?;

    let mut config = Config::default();
    config.agent.max_tokens_per_turn = 128;
    let config_toml = toml::to_string_pretty(&config)?;
    tokio::fs::write(project_dir.join(".rkat/config.toml"), config_toml).await?;

    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "run",
                "Say the word 'ok' and nothing else.",
                "--output",
                "json",
                "--enable-builtins",
            ])
            .output(),
    )
    .await??;

    if !output.status.success() {
        return Err(format!(
            "rkat run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())?;
    let session_id = parsed["session_id"]
        .as_str()
        .ok_or("session_id missing in response")?
        .to_string();

    let output_find = Command::new("find")
        .arg(&std::env::var("HOME")?)
        .arg("-name")
        .arg("*.jsonl")
        .output()
        .await?;
    let find_stdout = String::from_utf8_lossy(&output_find.stdout);

    // Heuristic: pick the directory of the first found session file
    let store_dir = if let Some(first_file) = find_stdout.lines().next() {
        std::path::Path::new(first_file)
            .parent()
            .ok_or("no parent dir")?
            .to_path_buf()
    } else {
        data_dir.join("meerkat").join("sessions")
    };

    let store = JsonlStore::new(store_dir);
    store.init().await?;
    let session = store
        .load(&SessionId::parse(&session_id)?)
        .await?
        .ok_or("session not found")?;
    let metadata = session.session_metadata().ok_or("metadata missing")?;
    assert!(metadata.tooling.builtins, "builtins should be recorded");

    let original_model = metadata.model.clone();
    let original_max_tokens = metadata.max_tokens;
    let original_tooling = metadata.tooling.clone();
    let original_provider = metadata.provider;

    let mut config_alt = config.clone();
    config_alt.agent.model = "gpt-4o-mini".into();
    config_alt.agent.max_tokens_per_turn = 7;
    let config_toml = toml::to_string_pretty(&config_alt)?;
    tokio::fs::write(project_dir.join(".rkat/config.toml"), config_toml).await?;

    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args(["resume", &session_id, "Continue."])
            .output(),
    )
    .await??;

    if !output.status.success() {
        return Err(format!(
            "rkat resume failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let session = store
        .load(&SessionId::parse(&session_id)?)
        .await?
        .ok_or("session not found after resume")?;
    let metadata = session
        .session_metadata()
        .ok_or("metadata missing after resume")?;

    assert_eq!(metadata.model, original_model, "model should persist");
    assert_eq!(
        metadata.max_tokens, original_max_tokens,
        "max_tokens should persist"
    );
    assert_eq!(
        metadata.provider, original_provider,
        "provider should persist"
    );
    assert_eq!(metadata.tooling.builtins, original_tooling.builtins);
    assert_eq!(metadata.tooling.shell, original_tooling.shell);
    assert_eq!(metadata.tooling.comms, original_tooling.comms);
    assert_eq!(metadata.tooling.subagents, original_tooling.subagents);
    Ok(())
}
