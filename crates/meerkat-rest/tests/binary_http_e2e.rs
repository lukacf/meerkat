#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, Instant, sleep, timeout};

/// Cargo, nextest, the e2e lane harness and the Bazel graph advertise the
/// crate's own binary through `CARGO_BIN_EXE_rkat-rest`, so a missing binary
/// fails the test instead of skipping it.
fn rest_binary_path() -> PathBuf {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rest")
        .map(PathBuf::from)
        .expect("cargo advertises CARGO_BIN_EXE_rkat-rest to the crate's integration tests");
    assert!(
        advertised.exists(),
        "CARGO_BIN_EXE_rkat-rest names a missing binary: {}",
        advertised.display()
    );
    advertised
}

async fn wait_for_rest_server(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            && response.status().is_success()
        {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err(format!("rkat-rest did not become healthy on port {port}").into())
}

fn allocate_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// The built binary creates a session and continues it, with the binary's
/// test client in the place of a provider: once started as a first start is
/// (no `--context-root`, on a home that holds nothing of the server's yet),
/// and once with a context root the operator names.
#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn http_e2e_create_and_continue_with_test_client() -> Result<(), Box<dyn std::error::Error>> {
    create_and_continue(ContextRoot::NotPassed)
        .await
        .map_err(|err| format!("without --context-root: {err}"))?;
    create_and_continue(ContextRoot::Passed)
        .await
        .map_err(|err| format!("with --context-root: {err}"))?;
    Ok(())
}

/// Whether the server is started with `--context-root`.
#[derive(Clone, Copy)]
enum ContextRoot {
    /// The flag names the project directory.
    Passed,
    /// No flag: the server uses its own instance directory under the data
    /// directory, which a first start does not find.
    NotPassed,
}

async fn create_and_continue(context_root: ContextRoot) -> Result<(), Box<dyn std::error::Error>> {
    let binary = rest_binary_path();

    let realm_id = "rest-binary-e2e";
    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let realm_root = state_root.join(realm_id);
    tokio::fs::create_dir_all(&realm_root).await?;
    let mut child_and_stderr = None;
    let mut port = 0;
    for _attempt in 0..5 {
        port = allocate_port();
        tokio::fs::write(
            realm_root.join("config.toml"),
            format!(
                "[agent]\nmodel = \"claude-sonnet-4-5\"\nmax_tokens_per_turn = 128\n[rest]\nhost = \"127.0.0.1\"\nport = {port}\n"
            ),
        )
        .await?;

        let mut child = Command::new(&binary)
            .current_dir(&project_dir)
            .env("HOME", &project_dir)
            .env("XDG_DATA_HOME", project_dir.join("data"))
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "--realm",
                realm_id,
                "--state-root",
                state_root.to_str().ok_or("invalid state root")?,
            ])
            .args(match context_root {
                ContextRoot::Passed => vec![
                    "--context-root",
                    project_dir.to_str().ok_or("invalid project dir")?,
                ],
                ContextRoot::NotPassed => Vec::new(),
            })
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        let stderr = child.stderr.take().ok_or("missing stderr")?;
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buffer = Vec::new();
            let _ = reader.read_to_end(&mut buffer).await;
            String::from_utf8_lossy(&buffer).to_string()
        });

        match wait_for_rest_server(port).await {
            Ok(()) => {
                child_and_stderr = Some((child, stderr_task));
                break;
            }
            Err(err) => {
                let _ = child.kill().await;
                let stderr = stderr_task.await.unwrap_or_else(|_| String::new());
                if stderr.contains("Address already in use") {
                    continue;
                }
                return Err(format!("{err}; stderr:\n{stderr}").into());
            }
        }
    }

    let Some((mut child, stderr_task)) = child_and_stderr else {
        return Err("failed to start rkat-rest after retrying address allocation".into());
    };

    let client = reqwest::Client::new();
    let created = timeout(
        Duration::from_secs(30),
        client
            .post(format!("http://127.0.0.1:{port}/sessions"))
            .json(&json!({
                "prompt": "Remember REST_BINARY_42 and reply with ok.",
                "model": "claude-sonnet-4-5",
                "max_tokens": 64
            }))
            .send(),
    )
    .await??;
    let created_status = created.status();
    let created_body = created.text().await?;
    if !created_status.is_success() {
        let _ = child.kill().await;
        return Err(format!("create answered {created_status}: {created_body}").into());
    }
    let created_json: serde_json::Value = serde_json::from_str(&created_body)?;
    let session_id = created_json["session_id"]
        .as_str()
        .ok_or("missing session_id")?
        .to_string();

    let continued = timeout(
        Duration::from_secs(30),
        client
            .post(format!(
                "http://127.0.0.1:{port}/sessions/{session_id}/messages"
            ))
            .json(&json!({
                "session_id": session_id,
                "prompt": "Continue the same session and reply with ok."
            }))
            .send(),
    )
    .await??;
    assert!(
        continued.status().is_success(),
        "continue failed: {continued:?}"
    );
    let continued_json: serde_json::Value = continued.json().await?;
    assert_eq!(
        continued_json["session_id"].as_str(),
        Some(session_id.as_str())
    );

    let _ = child.kill().await;
    let _ = stderr_task.await;
    Ok(())
}
