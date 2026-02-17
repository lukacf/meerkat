#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use meerkat::Config;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

/// Returns `true` if prerequisites are missing and the test should be skipped.
fn skip_if_no_prereqs() -> bool {
    if rkat_binary_path().is_none() {
        eprintln!("Skipping: rkat binary not found (build with `cargo build -p meerkat-cli`)");
        return true;
    }
    false
}

/// Returns `true` if API prereqs (binary + key) are missing.
fn skip_if_no_api_prereqs() -> bool {
    if skip_if_no_prereqs() {
        return true;
    }
    if anthropic_api_key().is_none() {
        eprintln!(
            "Skipping: no Anthropic API key (set ANTHROPIC_API_KEY or RKAT_ANTHROPIC_API_KEY)"
        );
        return true;
    }
    false
}

/// Write a minimal config.toml into the `.rkat/` directory inside `project_dir`.
async fn write_smoke_config(
    project_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let rkat_dir = project_dir.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;

    let mut config = Config::default();
    config.agent.max_tokens_per_turn = 256;
    config.agent.model = smoke_model();
    let config_toml = toml::to_string_pretty(&config)?;
    tokio::fs::write(rkat_dir.join("config.toml"), config_toml).await?;
    Ok(())
}

// ===========================================================================
// Scenario 11: CLI run + resume + verify persistence
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_cli_run_resume_persistence() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    // Outer test: spawn inner with isolated HOME / XDG dirs
    if std::env::var("RUN_TEST_E2E_SMOKE_11_INNER").is_ok() {
        return inner_e2e_cli_run_resume_persistence().await;
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;

    let status = Command::new(std::env::current_exe()?)
        .arg("e2e_cli_run_resume_persistence")
        .arg("--ignored")
        .env("RUN_TEST_E2E_SMOKE_11_INNER", "1")
        .env("HOME", temp_dir.path())
        .env("XDG_DATA_HOME", &data_dir)
        .env("TEST_PROJECT_DIR", &project_dir)
        .env("TEST_DATA_DIR", &data_dir)
        .status()
        .await?;

    assert!(status.success(), "inner test failed");
    Ok(())
}

async fn inner_e2e_cli_run_resume_persistence() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = PathBuf::from(std::env::var("TEST_PROJECT_DIR")?);
    let _data_dir = PathBuf::from(std::env::var("TEST_DATA_DIR")?);

    std::env::set_current_dir(&project_dir)?;
    write_smoke_config(&project_dir).await?;

    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    // --- Step 1: Initial run ---
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .args([
                "run",
                "My name is RkatBot. Remember my name.",
                "--output",
                "json",
                "--enable-builtins",
            ])
            .output(),
    )
    .await??;

    if !output.status.success() {
        return Err(format!(
            "rkat run failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Failed to parse JSON output: {e}\nstdout: {stdout}"))?;

    let session_id = parsed["session_id"]
        .as_str()
        .ok_or("session_id missing in initial run response")?
        .to_string();

    assert!(!session_id.is_empty(), "session_id should be non-empty");

    // --- Step 2: Resume and ask about name ---
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .args([
                "resume",
                &session_id,
                "What is my name? Reply with just the name.",
            ])
            .output(),
    )
    .await??;

    if !output.status.success() {
        return Err(format!(
            "rkat resume failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    // Resume outputs plain text (no --output json on resume command)
    let resume_stdout = String::from_utf8_lossy(&output.stdout);
    let resume_text = resume_stdout.trim().to_lowercase();

    assert!(
        resume_text.contains("rkatbot"),
        "Resume response should mention 'RkatBot', got: {resume_stdout}"
    );

    Ok(())
}

// ===========================================================================
// Scenario 12: CLI shell tool
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_cli_shell_tool() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    // Outer test
    if std::env::var("RUN_TEST_E2E_SMOKE_12_INNER").is_ok() {
        return inner_e2e_cli_shell_tool().await;
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;

    let status = Command::new(std::env::current_exe()?)
        .arg("e2e_cli_shell_tool")
        .arg("--ignored")
        .env("RUN_TEST_E2E_SMOKE_12_INNER", "1")
        .env("HOME", temp_dir.path())
        .env("XDG_DATA_HOME", &data_dir)
        .env("TEST_PROJECT_DIR", &project_dir)
        .env("TEST_DATA_DIR", &data_dir)
        .status()
        .await?;

    assert!(status.success(), "inner test failed");
    Ok(())
}

async fn inner_e2e_cli_shell_tool() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = PathBuf::from(std::env::var("TEST_PROJECT_DIR")?);

    std::env::set_current_dir(&project_dir)?;
    write_smoke_config(&project_dir).await?;

    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .args([
                "run",
                "Use the shell to run 'echo SMOKE_OK_42' and tell me the output",
                "--enable-builtins",
                "--enable-shell",
                "--output",
                "json",
            ])
            .output(),
    )
    .await??;

    assert!(
        output.status.success(),
        "rkat run with shell failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Failed to parse JSON output: {e}\nstdout: {stdout}"))?;

    let tool_calls = parsed["tool_calls"].as_u64().unwrap_or(0);
    assert!(
        tool_calls > 0,
        "Expected at least one tool call, got {tool_calls}"
    );

    let text = parsed["text"].as_str().unwrap_or("");
    assert!(
        text.contains("SMOKE_OK_42"),
        "Response text should contain 'SMOKE_OK_42', got: {text}"
    );

    Ok(())
}

// ===========================================================================
// Scenario 13: CLI capabilities + config (no API key needed)
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: spawns rkat binary"]
async fn e2e_cli_capabilities_and_config() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_prereqs() {
        return Ok(());
    }

    // Outer test
    if std::env::var("RUN_TEST_E2E_SMOKE_13_INNER").is_ok() {
        return inner_e2e_cli_capabilities_and_config().await;
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;

    let status = Command::new(std::env::current_exe()?)
        .arg("e2e_cli_capabilities_and_config")
        .arg("--ignored")
        .env("RUN_TEST_E2E_SMOKE_13_INNER", "1")
        .env("HOME", temp_dir.path())
        .env("XDG_DATA_HOME", &data_dir)
        .env("TEST_PROJECT_DIR", &project_dir)
        .env("TEST_DATA_DIR", &data_dir)
        .status()
        .await?;

    assert!(status.success(), "inner test failed");
    Ok(())
}

async fn inner_e2e_cli_capabilities_and_config() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = PathBuf::from(std::env::var("TEST_PROJECT_DIR")?);

    std::env::set_current_dir(&project_dir)?;
    write_smoke_config(&project_dir).await?;

    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    // --- Step 1: capabilities ---
    let output = timeout(
        Duration::from_secs(30),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .args(["capabilities"])
            .output(),
    )
    .await??;

    assert!(
        output.status.success(),
        "rkat capabilities failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let caps: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Failed to parse capabilities JSON: {e}\nstdout: {stdout}"))?;

    // Verify contract_version is present
    assert!(
        caps.get("contract_version").is_some(),
        "capabilities response should have contract_version, got: {caps}"
    );
    let contract_version = &caps["contract_version"];
    assert!(
        contract_version.get("major").is_some(),
        "contract_version should have major field"
    );

    // Verify "sessions" capability is present
    let capabilities = caps["capabilities"]
        .as_array()
        .ok_or("capabilities should be an array")?;
    let has_sessions = capabilities
        .iter()
        .any(|c| c["id"].as_str() == Some("sessions"));
    assert!(
        has_sessions,
        "capabilities should include 'sessions', got: {:?}",
        capabilities
            .iter()
            .filter_map(|c| c["id"].as_str())
            .collect::<Vec<_>>()
    );

    // --- Step 2: config get ---
    let output = timeout(
        Duration::from_secs(30),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .args(["config", "get"])
            .output(),
    )
    .await??;

    assert!(
        output.status.success(),
        "rkat config get failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let config_stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !config_stdout.trim().is_empty(),
        "config get output should be non-empty"
    );

    // Default format is TOML; verify it looks like TOML (has section headers or key=value)
    assert!(
        config_stdout.contains('[') || config_stdout.contains('='),
        "config get output should look like TOML, got: {config_stdout}"
    );

    Ok(())
}

// ===========================================================================
// Scenario 14: CLI structured output
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_cli_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    // Outer test
    if std::env::var("RUN_TEST_E2E_SMOKE_14_INNER").is_ok() {
        return inner_e2e_cli_structured_output().await;
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;

    let status = Command::new(std::env::current_exe()?)
        .arg("e2e_cli_structured_output")
        .arg("--ignored")
        .env("RUN_TEST_E2E_SMOKE_14_INNER", "1")
        .env("HOME", temp_dir.path())
        .env("XDG_DATA_HOME", &data_dir)
        .env("TEST_PROJECT_DIR", &project_dir)
        .env("TEST_DATA_DIR", &data_dir)
        .status()
        .await?;

    assert!(status.success(), "inner test failed");
    Ok(())
}

async fn inner_e2e_cli_structured_output() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = PathBuf::from(std::env::var("TEST_PROJECT_DIR")?);

    std::env::set_current_dir(&project_dir)?;
    write_smoke_config(&project_dir).await?;

    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let schema =
        r#"{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]}"#;

    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .args([
                "run",
                "What is 6 times 7? Give just the number.",
                "--output-schema",
                schema,
                "--output",
                "json",
            ])
            .output(),
    )
    .await??;

    assert!(
        output.status.success(),
        "rkat run with structured output failed (exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Failed to parse JSON output: {e}\nstdout: {stdout}"))?;

    // Verify structured_output is present and has the answer field
    let structured_output = &parsed["structured_output"];
    assert!(
        !structured_output.is_null(),
        "structured_output should be present in response, got: {parsed}"
    );

    let answer = structured_output.get("answer");
    assert!(
        answer.is_some(),
        "structured_output should have 'answer' field, got: {structured_output}"
    );

    // The answer should be a number (the LLM should return 42)
    let answer_val = answer.unwrap();
    assert!(
        answer_val.is_number(),
        "answer should be a number, got: {answer_val}"
    );

    Ok(())
}
