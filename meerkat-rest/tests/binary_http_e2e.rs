#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{Duration, Instant, sleep, timeout};

fn rest_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat-rest") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let debug = workspace_root.join("target/debug/rkat-rest");
    if debug.exists() {
        return Some(debug);
    }
    let release = workspace_root.join("target/release/rkat-rest");
    if release.exists() {
        return Some(release);
    }
    None
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

#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn http_e2e_create_and_continue_with_test_client() -> Result<(), Box<dyn std::error::Error>> {
    let Some(binary) = rest_binary_path() else {
        eprintln!("Skipping: missing rkat-rest binary");
        return Ok(());
    };

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
                "--context-root",
                project_dir.to_str().ok_or("invalid project dir")?,
            ])
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
    assert!(created.status().is_success(), "create failed: {created:?}");
    let created_json: serde_json::Value = created.json().await?;
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
