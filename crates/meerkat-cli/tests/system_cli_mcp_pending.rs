#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Real-binary journey regression for the 0.8.21 resume wedge: a first turn
//! run beside a slow-connecting MCP server commits the synthetic
//! `[MCP_PENDING]` notice inside the HeadCanonical boundary prefix; resumed
//! turns then re-materialize that notice differently. Before the
//! committed-notice durability fix, the intra-turn checkpoint rejected the
//! live transcript ("failed to prepare the preflighted head-canonical
//! intra-turn checkpoint route ... live actor reload required") and the CLI
//! surfaced an Internal error after the resumed turn completed. This journey
//! drives the shipped `rkat` binary through both resumed shapes (server still
//! pending, server gone) and requires clean exits, no wedge markers, and a
//! coherent growing session in the reopened realm.

use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use meerkat::{Config, SessionId, open_realm_persistence_in};
use meerkat_store::RealmOrigin;
use tempfile::TempDir;

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path.canonicalize().unwrap_or(path));
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

fn skip_if_no_prereqs() -> bool {
    if cfg!(windows) {
        eprintln!("Skipping: the slow-MCP stub uses /bin/sh");
        return true;
    }
    if rkat_binary_path().is_none() {
        eprintln!("Skipping: rkat binary not found (build with `cargo build -p meerkat-cli`)");
        return true;
    }
    false
}

/// Output fragments that identify the pre-fix checkpoint wedge. A journey turn
/// that prints any of these has reproduced the defect even if it exits zero.
const CHECKPOINT_WEDGE_MARKERS: &[&str] = &[
    "Internal error",
    "failed to prepare the preflighted head-canonical intra-turn checkpoint route",
    "live actor reload required",
    "does not retain the committed boundary row prefix",
];

fn assert_clean_cli_output(step: &str, output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{step} failed (exit {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );
    for marker in CHECKPOINT_WEDGE_MARKERS {
        assert!(
            !stdout.contains(marker) && !stderr.contains(marker),
            "{step} surfaced the checkpoint wedge marker '{marker}'\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

async fn write_slow_mcp_config(
    project_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // The stub never completes the MCP handshake inside the connect budget,
    // so the first turn always runs with the server pending and commits the
    // synthetic [MCP_PENDING] notice into the boundary prefix. The stderr
    // detach is load-bearing: the spawned stub inherits rkat's stderr, and a
    // held pipe keeps the test's `.output()` from seeing EOF after rkat
    // exits.
    let stub_path = project_dir.join("slow-mcp-server.sh");
    tokio::fs::write(&stub_path, "#!/bin/sh\nexec 2>/dev/null\nsleep 60\n").await?;
    let mcp_toml = format!(
        "[[servers]]\nname = \"king-search\"\ncommand = \"/bin/sh\"\nargs = [\"{}\"]\nconnect_timeout_secs = 2\n",
        stub_path.display()
    );
    tokio::fs::write(project_dir.join(".rkat/mcp.toml"), mcp_toml).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_mcp_pending_resume_journey() -> Result<(), Box<dyn std::error::Error>>
{
    if skip_if_no_prereqs() {
        return Ok(());
    }

    if std::env::var("RUN_TEST_CLI_MCP_PENDING_INNER").is_ok() {
        return inner_test_cli_mcp_pending_resume_journey().await;
    }

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;

    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let status = Command::new(std::env::current_exe()?)
        .arg("integration_real_cli_mcp_pending_resume_journey")
        .arg("--ignored")
        .env("RUN_TEST_CLI_MCP_PENDING_INNER", "1")
        .env("CARGO_BIN_EXE_rkat", &rkat)
        .env("HOME", temp_dir.path())
        .env("XDG_DATA_HOME", &data_dir)
        .env("TEST_PROJECT_DIR", &project_dir)
        .env("TEST_DATA_DIR", &data_dir)
        .status()
        .await?;

    assert!(status.success(), "inner test failed");
    Ok(())
}

async fn inner_test_cli_mcp_pending_resume_journey() -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = PathBuf::from(std::env::var("TEST_PROJECT_DIR")?);
    let data_dir = PathBuf::from(std::env::var("TEST_DATA_DIR")?);
    let home_dir = PathBuf::from(std::env::var("HOME")?);

    std::env::set_current_dir(&project_dir)?;

    let mut config = Config::default();
    config.agent.max_tokens_per_turn = Some(128);
    let config_toml = toml::to_string_pretty(&config)?;
    tokio::fs::write(project_dir.join(".rkat/config.toml"), config_toml).await?;
    write_slow_mcp_config(&project_dir).await?;

    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    // --- Turn one: `rkat run -m <model> hello` beside the pending server ---
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "run",
                "-m",
                "claude-sonnet-4-5",
                "hello",
                "--yolo",
                "--output",
                "json",
            ])
            .output(),
    )
    .await??;
    assert_clean_cli_output("turn one (run with pending MCP server)", &output);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("failed to parse turn-one JSON output: {error}\n{stdout}"))?;
    let session_id = parsed["session_id"]
        .as_str()
        .ok_or("session_id missing in turn-one response")?
        .to_string();
    let session_ref = parsed["session_ref"]
        .as_str()
        .ok_or("session_ref missing in turn-one response")?;
    let realm_id = session_ref
        .split_once(':')
        .map(|(realm_id, _)| realm_id)
        .ok_or("session_ref missing realm prefix")?
        .to_string();

    // --- Realm creation: the first run minted a workspace realm ---
    let realm_show = Command::new(&rkat)
        .current_dir(&project_dir)
        .env("HOME", &home_dir)
        .env("XDG_DATA_HOME", &data_dir)
        .args(["realm", "show", &realm_id])
        .output()
        .await?;
    assert_clean_cli_output("realm show after turn one", &realm_show);
    let realm_show_stdout = String::from_utf8_lossy(&realm_show.stdout);
    let realms_root = realm_show_stdout
        .lines()
        .find_map(|line| line.strip_prefix("state_root: "))
        .map(PathBuf::from)
        .ok_or_else(|| format!("state_root missing in realm show output: {realm_show_stdout}"))?;
    let (_manifest, persistence) =
        open_realm_persistence_in(&realms_root, &realm_id, None, Some(RealmOrigin::Workspace))
            .await?;
    let store = persistence.session_store();

    // Defect precondition: the committed turn-one transcript contains the
    // synthetic MCP pending notice. Without this the journey is vacuous.
    let session = store
        .load(&SessionId::parse(&session_id)?)
        .await?
        .ok_or("session not found after turn one")?;
    let turn_one_len = session.messages().len();
    let pending_notices = session
        .messages()
        .iter()
        .filter(|message| {
            matches!(
                message,
                meerkat_core::Message::SystemNotice(notice)
                    if notice.kind == meerkat_core::SystemNoticeKind::McpPending
            )
        })
        .count();
    assert!(
        pending_notices >= 1,
        "turn one must commit the [MCP_PENDING] notice into the boundary prefix; \
         the slow-MCP stub did not register as pending (transcript rows: {turn_one_len})"
    );

    // --- Turn two: resume while the server is STILL pending. Before the fix
    // this re-materialized the committed notice differently and wedged the
    // intra-turn checkpoint after the turn completed. ---
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args(["run", "--resume", &session_id, "--yolo", "Say ok again."])
            .output(),
    )
    .await??;
    assert_clean_cli_output("turn two (resume with server still pending)", &output);

    let session = store
        .load(&SessionId::parse(&session_id)?)
        .await?
        .ok_or("session not found after turn two: realm did not reopen")?;
    let turn_two_len = session.messages().len();
    assert!(
        turn_two_len > turn_one_len,
        "turn two must extend the reopened session ({turn_one_len} -> {turn_two_len})"
    );
    assert!(
        session.messages().iter().any(|message| {
            matches!(
                message,
                meerkat_core::Message::User(user)
                    if user.text_content().contains("Say ok again")
            )
        }),
        "turn two's user prompt must be visible in the persisted session"
    );

    // --- Turn three: resume after the slow server is gone (the strip shape
    // of the original repro). ---
    tokio::fs::remove_file(project_dir.join(".rkat/mcp.toml")).await?;
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args(["run", "--resume", &session_id, "--yolo", "And once more."])
            .output(),
    )
    .await??;
    assert_clean_cli_output("turn three (resume with server removed)", &output);

    let session = store
        .load(&SessionId::parse(&session_id)?)
        .await?
        .ok_or("session not found after turn three")?;
    assert!(
        session.messages().len() > turn_two_len,
        "turn three must extend the session ({turn_two_len} -> {})",
        session.messages().len()
    );
    // Committed synthetic notices are durable: the retained notice proves the
    // resumes were exact appends over the committed boundary, not a rewrite.
    assert!(
        session.messages().iter().any(|message| {
            matches!(
                message,
                meerkat_core::Message::SystemNotice(notice)
                    if notice.kind == meerkat_core::SystemNoticeKind::McpPending
            )
        }),
        "the committed [MCP_PENDING] notice must remain in the transcript after both resumes"
    );

    Ok(())
}
