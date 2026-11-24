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
/// and once with a context root the operator names. The binary as composed
/// also answers callers by the origin they name
/// (`answers_no_origin_that_is_not_allowed`).
#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn http_e2e_create_and_continue_with_test_client() -> Result<(), Box<dyn std::error::Error>> {
    create_and_continue(ContextRoot::NotPassed)
        .await
        .map_err(|err| format!("without --context-root: {err}"))?;
    create_and_continue(ContextRoot::Passed)
        .await
        .map_err(|err| format!("with --context-root: {err}"))?;
    answers_no_origin_that_is_not_allowed()
        .await
        .map_err(|err| format!("the server's answers by origin:\n{err}"))?;
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

/// The built binary, started and healthy on a loopback port.
struct RunningServer {
    child: tokio::process::Child,
    stderr_task: tokio::task::JoinHandle<String>,
    port: u16,
}

impl RunningServer {
    async fn stop(mut self) {
        let _ = self.child.kill().await;
        let _ = self.stderr_task.await;
    }
}

/// Start the built binary on `realm_id` under `temp`, on a free loopback
/// port, and wait until it is healthy.
async fn start_rest_server(
    temp: &TempDir,
    realm_id: &str,
    context_root: ContextRoot,
) -> Result<RunningServer, Box<dyn std::error::Error>> {
    let binary = rest_binary_path();

    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    let realm_root = state_root.join(realm_id);
    tokio::fs::create_dir_all(&realm_root).await?;
    for _attempt in 0..5 {
        let port = allocate_port();
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
                return Ok(RunningServer {
                    child,
                    stderr_task,
                    port,
                });
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
    Err("failed to start rkat-rest after retrying address allocation".into())
}

async fn create_and_continue(context_root: ContextRoot) -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let server = start_rest_server(&temp, "rest-binary-e2e", context_root).await?;
    let port = server.port;
    let RunningServer {
        mut child,
        stderr_task,
        ..
    } = server;

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

/// The built binary answers a browser page by its origin. A page of an
/// origin the realm does not allow gets no cross-origin allowance, on a read
/// and on its preflight, and its write is not applied. A caller that names no
/// origin is served as before, and its answer carries no allowance either.
async fn answers_no_origin_that_is_not_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let server = start_rest_server(&temp, "rest-binary-origins", ContextRoot::Passed).await?;
    let observed = timeout(
        Duration::from_mins(1),
        answers_by_origin(format!("http://127.0.0.1:{}", server.port)),
    )
    .await;
    server.stop().await;
    let unexpected = observed??;
    if unexpected.is_empty() {
        Ok(())
    } else {
        Err(unexpected.join("\n").into())
    }
}

/// The names of the cross-origin response headers an answer carries.
fn cross_origin_headers(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .keys()
        .map(|name| name.as_str().to_string())
        .filter(|name| name.starts_with("access-control-"))
        .collect()
}

async fn config_generation(
    client: &reqwest::Client,
    base: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let envelope: serde_json::Value = client
        .get(format!("{base}/config"))
        .send()
        .await?
        .json()
        .await?;
    envelope["generation"]
        .as_u64()
        .ok_or_else(|| format!("no generation in {envelope}").into())
}

/// Each answer of the server at `base` that differs from the contract, as
/// one line.
async fn answers_by_origin(base: String) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // A page served by another local server: another origin.
    const OTHER_ORIGIN: &str = "http://localhost:5173";
    let client = reqwest::Client::new();
    let patch = json!({"agent": {"max_tokens_per_turn": 129}});
    let mut unexpected = Vec::new();

    let read = client
        .get(format!("{base}/config"))
        .header("origin", OTHER_ORIGIN)
        .send()
        .await?;
    if read.status() != reqwest::StatusCode::FORBIDDEN || !cross_origin_headers(&read).is_empty() {
        unexpected.push(format!(
            "a read from an origin that is not allowed: {} with {:?}",
            read.status(),
            cross_origin_headers(&read)
        ));
    }

    let preflight = client
        .request(reqwest::Method::OPTIONS, format!("{base}/config"))
        .header("origin", OTHER_ORIGIN)
        .header("access-control-request-method", "PATCH")
        .header("access-control-request-headers", "content-type")
        .send()
        .await?;
    if preflight.status() != reqwest::StatusCode::FORBIDDEN
        || !cross_origin_headers(&preflight).is_empty()
    {
        unexpected.push(format!(
            "the preflight of an origin that is not allowed: {} with {:?}",
            preflight.status(),
            cross_origin_headers(&preflight)
        ));
    }

    let before = config_generation(&client, &base).await?;
    let write = client
        .patch(format!("{base}/config"))
        .header("origin", OTHER_ORIGIN)
        .json(&patch)
        .send()
        .await?;
    let write_status = write.status();
    let after = config_generation(&client, &base).await?;
    if write_status != reqwest::StatusCode::FORBIDDEN || after != before {
        unexpected.push(format!(
            "a write from an origin that is not allowed: {write_status}, the config generation \
             went from {before} to {after}"
        ));
    }

    let create = client
        .post(format!("{base}/sessions"))
        .header("origin", OTHER_ORIGIN)
        .json(&json!({
            "prompt": "Reply with ok.",
            "model": "claude-sonnet-4-5",
            "max_tokens": 64,
            "enable_builtins": true,
            "enable_shell": true
        }))
        .send()
        .await?;
    let create_status = create.status();
    let listed: serde_json::Value = client
        .get(format!("{base}/sessions"))
        .send()
        .await?
        .json()
        .await?;
    if create_status != reqwest::StatusCode::FORBIDDEN || listed["sessions"] != json!([]) {
        unexpected.push(format!(
            "a session create from an origin that is not allowed: {create_status}, the \
             sessions are {}",
            listed["sessions"]
        ));
    }

    // The control: a caller that names no origin.
    let plain_read = client.get(format!("{base}/config")).send().await?;
    if !plain_read.status().is_success() || !cross_origin_headers(&plain_read).is_empty() {
        unexpected.push(format!(
            "a read that names no origin: {} with {:?}",
            plain_read.status(),
            cross_origin_headers(&plain_read)
        ));
    }
    let plain_write = client
        .patch(format!("{base}/config"))
        .json(&patch)
        .send()
        .await?;
    let plain_write_status = plain_write.status();
    let written = config_generation(&client, &base).await?;
    if !plain_write_status.is_success() || written <= after {
        unexpected.push(format!(
            "a write that names no origin: {plain_write_status}, the config generation went \
             from {after} to {written}"
        ));
    }

    Ok(unexpected)
}
