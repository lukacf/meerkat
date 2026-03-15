#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep, timeout};

fn sqlite_shared_realm_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn binary_path(name: &str) -> PathBuf {
    let root = workspace_root();
    [
        root.join(format!("target/debug/{name}")),
        root.join(format!("target/release/{name}")),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
    .unwrap_or_else(|| panic!("binary '{name}' not found; build it before running this test"))
}

async fn write_project_config(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let rkat_dir = project_dir.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;
    tokio::fs::write(
        rkat_dir.join("config.toml"),
        "[agent]\nmodel = \"claude-sonnet-4-5\"\nmax_tokens_per_turn = 128\n",
    )
    .await?;
    Ok(())
}

fn command_base(cmd: &mut Command, cwd: &Path) {
    cmd.current_dir(cwd)
        .env("HOME", cwd)
        .env("XDG_DATA_HOME", cwd.join("data"))
        .env("RKAT_TEST_CLIENT", "1");
}

async fn run_binary(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(binary);
    command_base(&mut cmd, cwd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(args);
    Ok(timeout(Duration::from_secs(180), cmd.output()).await??)
}

fn output_ok_or_err(
    output: std::process::Output,
    binary: &str,
    args: &[&str],
) -> Result<String, String> {
    if !output.status.success() {
        return Err(format!(
            "command failed (exit {:?}): {} {}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            binary,
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn extract_json_string_field(payload: &str, field: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    find_first_string_field(&value, field)
}

fn find_first_string_field(value: &Value, field: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(field).and_then(Value::as_str) {
                return Some(found.to_string());
            }
            map.values()
                .find_map(|child| find_first_string_field(child, field))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_first_string_field(child, field)),
        _ => None,
    }
}

struct RpcProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
}

async fn spawn_rpc(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
) -> Result<RpcProcess, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(binary);
    command_base(&mut cmd, cwd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(args);
    let mut child = cmd.spawn()?;
    Ok(RpcProcess {
        stdin: child.stdin.take().expect("stdin"),
        stdout: BufReader::new(child.stdout.take().expect("stdout")),
        stderr: BufReader::new(child.stderr.take().expect("stderr")),
        child,
    })
}

async fn rpc_send_line(
    process: &mut RpcProcess,
    line: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    process.stdin.write_all(line.as_bytes()).await?;
    process.stdin.write_all(b"\n").await?;
    process.stdin.flush().await?;
    Ok(())
}

async fn rpc_call(
    process: &mut RpcProcess,
    id: u64,
    method: &str,
    params: Value,
    timeout_secs: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    rpc_send_line(
        process,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string(),
    )
    .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let mut line = String::new();
        timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            process.stdout.read_line(&mut line),
        )
        .await??;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed)?;
        let Some(response_id) = value.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if response_id != id {
            continue;
        }
        if !value["error"].is_null() {
            return Err(format!("rpc {method} failed: {value}").into());
        }
        return Ok(value["result"].clone());
    }
}

async fn shutdown_rpc(mut process: RpcProcess) -> Result<(), Box<dyn std::error::Error>> {
    let _ = process.stdin.shutdown().await;
    let _ = process.child.kill().await;
    let _ = timeout(Duration::from_secs(5), process.child.wait()).await;
    // Drain stderr with a timeout — grandchild processes may hold the pipe
    // open after kill(), causing read_to_string to block forever.
    let mut stderr = String::new();
    let _ = timeout(
        Duration::from_secs(2),
        process.stderr.read_to_string(&mut stderr),
    )
    .await;
    Ok(())
}

async fn spawn_rest(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
    port: u16,
) -> Result<Child, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(binary);
    command_base(&mut cmd, cwd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(args);
    let mut child = cmd.spawn()?;
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{port}/health");

    for _ in 0..50 {
        if let Some(status) = child.try_wait()? {
            let stderr = child
                .stderr
                .take()
                .map(BufReader::new)
                .map(|mut reader| async move {
                    let mut buf = String::new();
                    let _ = reader.read_to_string(&mut buf).await;
                    buf
                });
            let stderr = match stderr {
                Some(fut) => fut.await,
                None => String::new(),
            };
            return Err(format!("REST exited early with {status}: {stderr}").into());
        }

        if let Ok(response) = client.get(&health_url).send().await
            && response.status().is_success()
        {
            return Ok(child);
        }
        sleep(Duration::from_millis(200)).await;
    }

    Err(format!("REST server did not become healthy on port {port}").into())
}

async fn shutdown_child(mut child: Child) -> Result<(), Box<dyn std::error::Error>> {
    let _ = child.kill().await;
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
    Ok(())
}

fn allocate_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn read_manifest(
    state_root: &Path,
    realm_id: &str,
) -> Result<meerkat_store::RealmManifest, Box<dyn std::error::Error>> {
    let paths = meerkat_store::realm_paths_in(state_root, realm_id);
    let manifest = tokio::fs::read_to_string(paths.manifest_path).await?;
    Ok(serde_json::from_str(&manifest)?)
}

async fn load_session(
    state_root: &Path,
    realm_id: &str,
    session_id: &meerkat_core::SessionId,
) -> Result<meerkat_core::Session, Box<dyn std::error::Error>> {
    let (_manifest, store) =
        meerkat_store::open_realm_session_store_in(state_root, realm_id, None, None).await?;
    Ok(store.load(session_id).await?.expect("session exists"))
}

async fn session_message_count(
    state_root: &Path,
    realm_id: &str,
    session_id: &meerkat_core::SessionId,
) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(load_session(state_root, realm_id, session_id)
        .await?
        .messages()
        .len())
}

async fn write_rest_config(
    state_root: &Path,
    realm_id: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let paths = meerkat_store::realm_paths_in(state_root, realm_id);
    tokio::fs::create_dir_all(paths.root.clone()).await?;
    tokio::fs::write(
        paths.config_path,
        format!(
            "[agent]\nmodel = \"claude-sonnet-4-5\"\nmax_tokens_per_turn = 128\n\n[rest]\nhost = \"127.0.0.1\"\nport = {port}\n"
        ),
    )
    .await?;
    Ok(())
}

async fn assert_default_sqlite_realm(
    state_root: &Path,
    realm_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = read_manifest(state_root, realm_id).await?;
    assert_eq!(manifest.backend, meerkat_store::RealmBackend::Sqlite);
    let paths = meerkat_store::realm_paths_in(state_root, realm_id);
    assert!(paths.sessions_sqlite_path.exists());
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: spawns rkat-rpc/rkat-rest binaries"]
async fn rpc_rest_rpc_default_sqlite_shared_realm_roundtrip()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = sqlite_shared_realm_test_lock().lock().await;
    let rkat_rpc = binary_path("rkat-rpc");
    let rkat_rest = binary_path("rkat-rest");

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "sqlite-shared-rpc-rest";
    let mut rpc = spawn_rpc(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--context-root",
            project_dir.to_str().unwrap(),
            "--instance",
            "rpc-a",
        ],
    )
    .await?;

    let initialize = rpc_call(&mut rpc, 1, "initialize", json!({}), 20).await?;
    assert!(initialize["methods"].as_array().is_some());

    let created = rpc_call(
        &mut rpc,
        2,
        "session/create",
        json!({
            "prompt": "Create a shared sqlite session.",
            "model": "claude-sonnet-4-5",
        }),
        60,
    )
    .await?;
    let session_id = find_first_string_field(&created, "session_id").expect("session_id");
    let session_id = meerkat_core::SessionId(uuid::Uuid::parse_str(&session_id)?);
    let session_id_str = session_id.to_string();
    assert_default_sqlite_realm(&state_root, realm_id).await?;
    let count_before = session_message_count(&state_root, realm_id, &session_id).await?;

    let port = allocate_port();
    write_rest_config(&state_root, realm_id, port).await?;
    let rest = spawn_rest(
        &rkat_rest,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--context-root",
            project_dir.to_str().unwrap(),
            "--instance",
            "rest-b",
        ],
        port,
    )
    .await?;

    let leases = meerkat_store::inspect_realm_leases_in(&state_root, realm_id, false).await?;
    assert!(
        leases.active.len() >= 2,
        "expected overlapping leases, got {:?}",
        leases.active
    );

    let client = reqwest::Client::new();
    let get_response = client
        .get(format!("http://127.0.0.1:{port}/sessions/{session_id_str}"))
        .send()
        .await?;
    assert!(get_response.status().is_success());
    let read_json: Value = get_response.json().await?;
    assert_eq!(
        read_json["session_id"].as_str(),
        Some(session_id_str.as_str())
    );

    let continue_response = client
        .post(format!(
            "http://127.0.0.1:{port}/sessions/{session_id_str}/messages"
        ))
        .json(&json!({
            "session_id": session_id_str.clone(),
            "prompt": "Continue the shared sqlite session."
        }))
        .send()
        .await?;
    assert!(continue_response.status().is_success());
    let _continued: Value = continue_response.json().await?;
    let count_after_rest = session_message_count(&state_root, realm_id, &session_id).await?;
    assert!(count_after_rest > count_before);

    shutdown_rpc(rpc).await?;
    let mut rpc = spawn_rpc(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--context-root",
            project_dir.to_str().unwrap(),
            "--instance",
            "rpc-c",
        ],
    )
    .await?;
    let _ = rpc_call(&mut rpc, 10, "initialize", json!({}), 20).await?;
    let resumed = rpc_call(
        &mut rpc,
        11,
        "turn/start",
        json!({
            "session_id": session_id_str.clone(),
            "prompt": "Continue once more.",
        }),
        60,
    )
    .await?;
    assert_eq!(
        find_first_string_field(&resumed, "session_id").as_deref(),
        Some(session_id_str.as_str())
    );
    let count_after_rpc = session_message_count(&state_root, realm_id, &session_id).await?;
    assert!(
        count_after_rpc > count_after_rest,
        "expected RPC handoff to advance durable message count: before={count_before}, after_rest={count_after_rest}, after_rpc={count_after_rpc}"
    );

    shutdown_child(rest).await?;
    shutdown_rpc(rpc).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: spawns rkat-rpc/rkat binaries"]
async fn cli_rpc_cli_default_sqlite_shared_realm_roundtrip()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = sqlite_shared_realm_test_lock().lock().await;
    let rkat = binary_path("rkat");
    let rkat_rpc = binary_path("rkat-rpc");

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "sqlite-shared-cli-rpc";
    let run_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "run",
        "Create a sqlite-backed CLI session.",
        "--output",
        "json",
    ];
    let run_output = run_binary(&rkat, &project_dir, &run_args).await?;
    let run_stdout =
        output_ok_or_err(run_output, "rkat", &run_args).map_err(std::io::Error::other)?;
    let session_id = extract_json_string_field(&run_stdout, "session_id").expect("session id");
    let session_id = meerkat_core::SessionId(uuid::Uuid::parse_str(&session_id)?);
    let session_id_str = session_id.to_string();

    assert_default_sqlite_realm(&state_root, realm_id).await?;
    let count_before = session_message_count(&state_root, realm_id, &session_id).await?;

    let mut rpc = spawn_rpc(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--context-root",
            project_dir.to_str().unwrap(),
        ],
    )
    .await?;
    let _ = rpc_call(&mut rpc, 1, "initialize", json!({}), 20).await?;
    let read = rpc_call(
        &mut rpc,
        2,
        "session/read",
        json!({ "session_id": session_id_str.clone() }),
        20,
    )
    .await?;
    assert_eq!(
        find_first_string_field(&read, "session_id").as_deref(),
        Some(session_id_str.as_str())
    );
    let turn = rpc_call(
        &mut rpc,
        3,
        "turn/start",
        json!({
            "session_id": session_id_str.clone(),
            "prompt": "Continue from RPC.",
        }),
        60,
    )
    .await?;
    assert_eq!(
        find_first_string_field(&turn, "session_id").as_deref(),
        Some(session_id_str.as_str())
    );
    shutdown_rpc(rpc).await?;

    let count_after_rpc = session_message_count(&state_root, realm_id, &session_id).await?;
    assert!(count_after_rpc > count_before);

    let resume_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "resume",
        &session_id.to_string(),
        "Continue from CLI again.",
    ];
    let resume_output = run_binary(&rkat, &project_dir, &resume_args).await?;
    let _resume_stdout =
        output_ok_or_err(resume_output, "rkat", &resume_args).map_err(std::io::Error::other)?;
    let count_after_cli = session_message_count(&state_root, realm_id, &session_id).await?;
    assert!(count_after_cli > count_after_rpc);
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: spawns rkat-rest/rkat binaries"]
async fn cli_rest_cli_default_sqlite_shared_realm_roundtrip()
-> Result<(), Box<dyn std::error::Error>> {
    let _guard = sqlite_shared_realm_test_lock().lock().await;
    let rkat = binary_path("rkat");
    let rkat_rest = binary_path("rkat-rest");

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "sqlite-shared-cli-rest";
    let run_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "run",
        "Create a sqlite-backed CLI session.",
        "--output",
        "json",
    ];
    let run_output = run_binary(&rkat, &project_dir, &run_args).await?;
    let run_stdout =
        output_ok_or_err(run_output, "rkat", &run_args).map_err(std::io::Error::other)?;
    let session_id = extract_json_string_field(&run_stdout, "session_id").expect("session id");
    let session_id = meerkat_core::SessionId(uuid::Uuid::parse_str(&session_id)?);

    assert_default_sqlite_realm(&state_root, realm_id).await?;
    let count_before = session_message_count(&state_root, realm_id, &session_id).await?;

    let port = allocate_port();
    write_rest_config(&state_root, realm_id, port).await?;
    let rest = spawn_rest(
        &rkat_rest,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--context-root",
            project_dir.to_str().unwrap(),
        ],
        port,
    )
    .await?;

    let client = reqwest::Client::new();
    let get_response = client
        .get(format!("http://127.0.0.1:{port}/sessions/{session_id}"))
        .send()
        .await?;
    assert!(get_response.status().is_success());

    let continue_response = client
        .post(format!(
            "http://127.0.0.1:{port}/sessions/{session_id}/messages"
        ))
        .json(&json!({
            "session_id": session_id.to_string(),
            "prompt": "Continue from REST.",
        }))
        .send()
        .await?;
    assert!(continue_response.status().is_success());
    let _continued: Value = continue_response.json().await?;
    shutdown_child(rest).await?;

    let count_after_rest = session_message_count(&state_root, realm_id, &session_id).await?;
    assert!(count_after_rest > count_before);

    let resume_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "resume",
        &session_id.to_string(),
        "Continue from CLI again.",
    ];
    let resume_output = run_binary(&rkat, &project_dir, &resume_args).await?;
    let _resume_stdout =
        output_ok_or_err(resume_output, "rkat", &resume_args).map_err(std::io::Error::other)?;
    let count_after_cli = session_message_count(&state_root, realm_id, &session_id).await?;
    assert!(count_after_cli > count_after_rest);
    Ok(())
}
