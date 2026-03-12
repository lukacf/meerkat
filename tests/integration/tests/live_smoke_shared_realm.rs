#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, sleep, timeout};

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn binary_path(name: &str) -> Option<PathBuf> {
    let root = workspace_root();
    [
        root.join(format!("target/debug/{name}")),
        root.join(format!("target/release/{name}")),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
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

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

fn skip_if_missing_binary(path: &Option<PathBuf>, name: &str) -> bool {
    if path.is_none() {
        eprintln!("Skipping: binary '{name}' not found under target/debug or target/release");
        return true;
    }
    false
}

async fn write_project_config(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let rkat_dir = project_dir.join(".rkat");
    tokio::fs::create_dir_all(&rkat_dir).await?;
    let config = format!(
        "[agent]\nmodel = \"{}\"\nmax_tokens_per_turn = 256\nbudget_warning_threshold = 0.8\n",
        smoke_model()
    );
    tokio::fs::write(rkat_dir.join("config.toml"), config).await?;
    for slug in [
        "task-workflow",
        "shell-patterns",
        "sub-agent-orchestration",
        "multi-agent-comms",
        "mcp-server-setup",
        "hook-authoring",
        "memory-retrieval",
        "session-management",
    ] {
        let skill_dir = rkat_dir.join("skills").join(slug);
        tokio::fs::create_dir_all(&skill_dir).await?;
        let body = format!(
            "---\nname: {slug}\ndescription: Minimal live smoke placeholder skill\n---\n\n# {slug}\n\nMinimal live smoke placeholder skill.\n"
        );
        tokio::fs::write(skill_dir.join("SKILL.md"), body).await?;
    }
    Ok(())
}

async fn run_binary(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
    api_key: Option<&str>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(binary);
    cmd.current_dir(cwd)
        .env("HOME", cwd)
        .env("XDG_DATA_HOME", cwd.join("data"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(args);
    if let Some(key) = api_key {
        cmd.env("ANTHROPIC_API_KEY", key)
            .env("RKAT_ANTHROPIC_API_KEY", key);
    }
    Ok(timeout(Duration::from_secs(180), cmd.output()).await??)
}

fn is_redb_lock_failure(output: &std::process::Output) -> bool {
    String::from_utf8_lossy(&output.stderr).contains("Database already open. Cannot acquire lock.")
}

async fn run_binary_with_redb_retry(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
    api_key: Option<&str>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let output = run_binary(binary, cwd, args, api_key).await?;
        if output.status.success() || !is_redb_lock_failure(&output) {
            return Ok(output);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(output);
        }
        sleep(Duration::from_millis(200)).await;
    }
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

fn extract_all_json_string_fields(payload: &str, field: &str) -> Vec<String> {
    let Some(value) = serde_json::from_str::<Value>(payload).ok() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_string_fields(&value, field, &mut out);
    out
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

fn collect_string_fields(value: &Value, field: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(field).and_then(Value::as_str) {
                out.push(found.to_string());
            }
            for child in map.values() {
                collect_string_fields(child, field, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_string_fields(child, field, out);
            }
        }
        _ => {}
    }
}

struct RpcProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: BufReader<ChildStderr>,
}

async fn spawn_stdio_process(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
    api_key: Option<&str>,
) -> Result<RpcProcess, Box<dyn std::error::Error>> {
    let mut cmd = Command::new(binary);
    cmd.current_dir(cwd)
        .env("HOME", cwd)
        .env("XDG_DATA_HOME", cwd.join("data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(args);
    if let Some(key) = api_key {
        cmd.env("ANTHROPIC_API_KEY", key)
            .env("RKAT_ANTHROPIC_API_KEY", key);
    }
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().ok_or("missing child stdin")?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let stderr = child.stderr.take().ok_or("missing child stderr")?;
    Ok(RpcProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        stderr: BufReader::new(stderr),
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

async fn rpc_read_response_line(
    process: &mut RpcProcess,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        let mut line = String::new();
        let bytes_read = timeout(
            Duration::from_secs(timeout_secs),
            process.stdout.read_line(&mut line),
        )
        .await??;
        if bytes_read == 0 {
            let mut stderr = String::new();
            let _ = process.stderr.read_to_string(&mut stderr).await;
            return Err(format!(
                "stdio process reached EOF before response\nstderr:\n{}",
                stderr.trim()
            )
            .into());
        }
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(&trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if parsed.get("id").is_some() {
            return Ok(trimmed);
        }
    }
}

fn parse_json_line(line: &str) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(line)?)
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
        &serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?,
    )
    .await?;
    let line = rpc_read_response_line(process, timeout_secs).await?;
    let value = parse_json_line(&line)?;
    if !value["error"].is_null() {
        return Err(format!("rpc {method} failed: {value}").into());
    }
    Ok(value["result"].clone())
}

async fn shutdown_stdio_process(mut process: RpcProcess) -> Result<(), Box<dyn std::error::Error>> {
    drop(process.stdin);
    let status = timeout(Duration::from_secs(20), process.child.wait()).await??;
    if !status.success() {
        return Err(format!("stdio process exited unsuccessfully: {status}").into());
    }
    Ok(())
}

fn allocate_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

async fn wait_for_rest_server(
    mut child: Child,
    port: u16,
) -> Result<Child, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(child);
        }
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output().await?;
            return Err(format!(
                "REST server exited before binding port {port}: {status}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
            .into());
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.start_kill();
            let output = child.wait_with_output().await?;
            return Err(format!(
                "timed out waiting for port {port}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn shutdown_child(mut child: Child) -> Result<(), Box<dyn std::error::Error>> {
    let _ = child.start_kill();
    match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(status) => {
            let _ = status?;
        }
        Err(_) => {
            if let Some(pid) = child.id() {
                let _ = Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .status()
                    .await;
            }
            let _ = timeout(Duration::from_secs(5), child.wait()).await?;
        }
    }
    Ok(())
}

async fn http_request(port: u16, request: String) -> Result<String, Box<dyn std::error::Error>> {
    let (head, body) = request
        .split_once("\r\n\r\n")
        .ok_or("invalid HTTP request: missing header/body separator")?;
    let mut lines = head.lines();
    let request_line = lines
        .next()
        .ok_or("invalid HTTP request: missing request line")?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or("invalid HTTP request: missing method")?;
    let path = request_parts
        .next()
        .ok_or("invalid HTTP request: missing path")?;

    let mut headers = reqwest::header::HeaderMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("connection")
            {
                continue;
            }
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
                reqwest::header::HeaderValue::from_str(value.trim())?,
            );
        }
    }

    let client = reqwest::Client::builder().build()?;
    let url = format!("http://127.0.0.1:{port}{path}");
    let mut request_builder = client.request(reqwest::Method::from_bytes(method.as_bytes())?, &url);
    if !headers.is_empty() {
        request_builder = request_builder.headers(headers);
    }
    if !body.is_empty() {
        request_builder = request_builder.body(body.to_string());
    }

    let response = request_builder.send().await?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let response_body = response.text().await?;

    let mut raw = format!(
        "HTTP/1.1 {} {}\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );
    let mut saw_content_length = false;
    for (name, value) in &response_headers {
        if name == reqwest::header::CONTENT_LENGTH {
            saw_content_length = true;
        }
        raw.push_str(name.as_str());
        raw.push_str(": ");
        raw.push_str(value.to_str().unwrap_or_default());
        raw.push_str("\r\n");
    }
    if !saw_content_length {
        raw.push_str(&format!("content-length: {}\r\n", response_body.len()));
    }
    raw.push_str("\r\n");
    raw.push_str(&response_body);
    Ok(raw)
}

fn http_body(response: &str) -> &str {
    response.split("\r\n\r\n").nth(1).unwrap_or_default()
}

fn http_json_body(response: &str) -> Result<Value, Box<dyn std::error::Error>> {
    serde_json::from_str(http_body(response)).map_err(|err| {
        format!("failed to parse HTTP JSON body: {err}\nfull response:\n{response}").into()
    })
}

fn parse_mcp_tool_payload(response: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .ok_or_else(|| format!("missing MCP tool payload text in response: {response}"))?;
    Ok(serde_json::from_str(text)?)
}

async fn initialize_mcp(
    process: &mut RpcProcess,
    timeout_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    rpc_send_line(
        process,
        &serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "meerkat-live-smoke",
                    "version": "1.0.0"
                }
            }
        }))?,
    )
    .await?;
    let init = parse_json_line(&rpc_read_response_line(process, timeout_secs).await?)?;
    if !init["error"].is_null() {
        return Err(format!("mcp initialize failed: {init}").into());
    }
    rpc_send_line(
        process,
        &serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))?,
    )
    .await?;
    Ok(())
}

async fn mcp_call_tool(
    process: &mut RpcProcess,
    id: u64,
    name: &str,
    arguments: Value,
    timeout_secs: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    rpc_send_line(
        process,
        &serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
            }
        }))?,
    )
    .await?;
    parse_json_line(&rpc_read_response_line(process, timeout_secs).await?)
}

async fn write_cli_mobpack_fixture(
    project_dir: &Path,
    mob_id: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mob_dir = project_dir.join(format!("{mob_id}-fixture"));
    tokio::fs::create_dir_all(&mob_dir).await?;
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        format!("[mobpack]\nname = \"{mob_id}\"\nversion = \"1.0.0\"\n"),
    )
    .await?;
    let definition = format!(
        r#"{{
  "id":"{mob_id}",
  "profiles":{{
    "lead":{{"model":"{model}","tools":{{"comms":true}},"external_addressable":true}},
    "worker":{{"model":"{model}","tools":{{"comms":true}}}},
    "reviewer":{{"model":"{model}","tools":{{"comms":true}}}}
  }},
  "wiring":{{"auto_wire_orchestrator":false,"role_wiring":[{{"a":"lead","b":"worker"}},{{"a":"worker","b":"reviewer"}}]}},
  "skills":{{}}
}}"#,
        model = smoke_model()
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition).await?;
    Ok(mob_dir)
}

// ===========================================================================
// Scenario 49: RPC -> REST shared-realm session continuity
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: shared realm rpc/rest live API"]
async fn e2e_scenario_49_rpc_rest_shared_realm_roundtrip() -> Result<(), Box<dyn std::error::Error>>
{
    let rkat_rpc = binary_path("rkat-rpc");
    let rkat_rest = binary_path("rkat-rest");
    if skip_if_missing_binary(&rkat_rpc, "rkat-rpc")
        || skip_if_missing_binary(&rkat_rest, "rkat-rest")
    {
        return Ok(());
    }
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: no Anthropic API key configured");
        return Ok(());
    };
    let rkat_rpc = rkat_rpc.unwrap();
    let rkat_rest = rkat_rest.unwrap();

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "scenario-49-shared";
    let mut rpc = spawn_stdio_process(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--realm-backend",
            "redb",
            "--context-root",
            project_dir.to_str().unwrap(),
        ],
        Some(&api_key),
    )
    .await?;

    let initialize = rpc_call(&mut rpc, 1, "initialize", json!({}), 20).await?;
    assert!(
        initialize["methods"]
            .as_array()
            .is_some_and(|methods| methods
                .iter()
                .any(|entry| entry.as_str() == Some("session/create"))),
        "rpc initialize should advertise session/create: {initialize}"
    );

    let created = rpc_call(
        &mut rpc,
        2,
        "session/create",
        json!({
            "prompt": "My codename is SharedKite49 and my favorite texture is linen. Reply briefly.",
            "model": smoke_model(),
        }),
        180,
    )
    .await?;
    let session_id = created["session_id"]
        .as_str()
        .ok_or("rpc session/create missing session_id")?
        .to_string();
    shutdown_stdio_process(rpc).await?;

    let port = allocate_port();
    let realm_paths = meerkat_store::realm_paths_in(&state_root, realm_id);
    tokio::fs::create_dir_all(realm_paths.root.clone()).await?;
    let rest_config = format!(
        "[agent]\nmodel = \"{}\"\nmax_tokens_per_turn = 256\nbudget_warning_threshold = 0.8\n\n[rest]\nhost = \"127.0.0.1\"\nport = {port}\n",
        smoke_model()
    );
    tokio::fs::write(&realm_paths.config_path, rest_config).await?;

    let mut rest = Command::new(&rkat_rest);
    rest.current_dir(&project_dir)
        .env("HOME", &project_dir)
        .env("XDG_DATA_HOME", project_dir.join("data"))
        .env("ANTHROPIC_API_KEY", &api_key)
        .env("RKAT_ANTHROPIC_API_KEY", &api_key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--context-root",
            project_dir.to_str().unwrap(),
            "--instance",
            "scenario-49-rest",
        ]);
    let rest_child = wait_for_rest_server(rest.spawn()?, port).await?;

    let mut last_read_response: Option<String> = None;
    let read_json = {
        let mut parsed = None;
        for _ in 0..20 {
            let read_response = http_request(
                port,
                format!(
                    "GET /sessions/{session_id} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                ),
            )
            .await?;
            last_read_response = Some(read_response);
            let latest = last_read_response.as_deref().unwrap_or_default();
            if latest.starts_with("HTTP/1.1 200") && !http_body(latest).trim().is_empty() {
                parsed = Some(http_json_body(latest)?);
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }
        parsed.ok_or_else(|| {
            format!(
                "REST never surfaced the RPC-created session with a JSON body: {}",
                last_read_response.unwrap_or_default()
            )
        })?
    };
    assert_eq!(read_json["session_id"], session_id);

    let continue_body = format!(
        r#"{{"session_id":"{session_id}","prompt":"What are my codename and favorite texture? Reply in one sentence."}}"#
    );
    let continue_response = http_request(
        port,
        format!(
            "POST /sessions/{session_id}/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            continue_body.len(),
            continue_body
        ),
    )
    .await?;
    assert!(
        continue_response.starts_with("HTTP/1.1 200")
            && !http_body(&continue_response).trim().is_empty(),
        "REST continuation should return JSON body: {continue_response}"
    );
    let continued = http_json_body(&continue_response)?;
    let text = continued["text"].as_str().unwrap_or("").to_lowercase();
    assert!(
        (text.contains("sharedkite49") || text.contains("shared kite 49"))
            && text.contains("linen"),
        "REST continuation should observe RPC-authored state: {continued}"
    );

    shutdown_child(rest_child).await?;

    let mut rpc = spawn_stdio_process(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--realm-backend",
            "redb",
            "--context-root",
            project_dir.to_str().unwrap(),
        ],
        Some(&api_key),
    )
    .await?;
    let _ = rpc_call(&mut rpc, 10, "initialize", json!({}), 20).await?;
    let resumed = rpc_call(
        &mut rpc,
        11,
        "turn/start",
        json!({
            "session_id": session_id,
            "prompt": "Confirm my codename and favorite texture one more time in one sentence."
        }),
        180,
    )
    .await?;
    let resumed_text = resumed["text"].as_str().unwrap_or("").to_lowercase();
    assert!(
        (resumed_text.contains("sharedkite49") || resumed_text.contains("shared kite 49"))
            && resumed_text.contains("linen"),
        "RPC should observe the REST-authored continuation after reopen: {resumed}"
    );
    shutdown_stdio_process(rpc).await?;
    Ok(())
}

// ===========================================================================
// Scenario 50: REST -> CLI shared-realm session continuity
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: shared realm rest/cli live API"]
async fn e2e_scenario_50_rest_cli_shared_realm_roundtrip() -> Result<(), Box<dyn std::error::Error>>
{
    let rkat = binary_path("rkat");
    let rkat_rest = binary_path("rkat-rest");
    if skip_if_missing_binary(&rkat, "rkat") || skip_if_missing_binary(&rkat_rest, "rkat-rest") {
        return Ok(());
    }
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: no Anthropic API key configured");
        return Ok(());
    };
    let rkat = rkat.unwrap();
    let rkat_rest = rkat_rest.unwrap();

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "scenario-50-shared";
    let port = allocate_port();
    let realm_paths = meerkat_store::realm_paths_in(&state_root, realm_id);
    tokio::fs::create_dir_all(realm_paths.root.clone()).await?;
    let rest_config = format!(
        "[agent]\nmodel = \"{}\"\nmax_tokens_per_turn = 256\nbudget_warning_threshold = 0.8\n\n[rest]\nhost = \"127.0.0.1\"\nport = {port}\n",
        smoke_model()
    );
    tokio::fs::write(&realm_paths.config_path, rest_config).await?;

    let mut rest = Command::new(&rkat_rest);
    rest.current_dir(&project_dir)
        .env("HOME", &project_dir)
        .env("XDG_DATA_HOME", project_dir.join("data"))
        .env("ANTHROPIC_API_KEY", &api_key)
        .env("RKAT_ANTHROPIC_API_KEY", &api_key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--realm-backend",
            "redb",
            "--context-root",
            project_dir.to_str().unwrap(),
            "--instance",
            "scenario-50-rest",
        ]);
    let rest_child = wait_for_rest_server(rest.spawn()?, port).await?;

    let create_body = format!(
        r#"{{"prompt":"My codename is SharedPine50 and my favorite bird is a waxwing. Reply briefly.","model":"{}"}}"#,
        smoke_model()
    );
    let create_response = http_request(
        port,
        format!(
            "POST /sessions HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            create_body.len(),
            create_body
        ),
    )
    .await?;
    eprintln!("scenario 50: received REST create response");
    let created = http_json_body(&create_response)?;
    let session_id = created["session_id"]
        .as_str()
        .ok_or("REST create missing session_id")?
        .to_string();
    eprintln!("scenario 50: parsed session_id {session_id}");

    eprintln!("scenario 50: shutting down REST child");
    shutdown_child(rest_child).await?;
    eprintln!("scenario 50: REST child shut down");

    let resume_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "resume",
        &session_id,
        "What are my codename and favorite bird? Reply in one sentence.",
    ];
    eprintln!("scenario 50: starting CLI resume");
    let resume_out =
        run_binary_with_redb_retry(&rkat, &project_dir, &resume_args, Some(&api_key)).await?;
    eprintln!("scenario 50: CLI resume finished");
    let resume_stdout =
        output_ok_or_err(resume_out, "rkat", &resume_args).map_err(std::io::Error::other)?;
    let lower = resume_stdout.to_lowercase();
    assert!(
        (lower.contains("sharedpine50") || lower.contains("shared pine 50"))
            && lower.contains("waxwing"),
        "CLI resume should observe REST-created session state: {resume_stdout}"
    );

    Ok(())
}

// ===========================================================================
// Scenario 51: RPC -> MCP shared-realm visibility and event parity
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: shared realm rpc/mcp live API"]
async fn e2e_scenario_51_rpc_mcp_shared_realm_roundtrip() -> Result<(), Box<dyn std::error::Error>>
{
    let rkat_rpc = binary_path("rkat-rpc");
    let rkat_mcp = binary_path("rkat-mcp");
    if skip_if_missing_binary(&rkat_rpc, "rkat-rpc")
        || skip_if_missing_binary(&rkat_mcp, "rkat-mcp")
    {
        return Ok(());
    }
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: no Anthropic API key configured");
        return Ok(());
    };
    let rkat_rpc = rkat_rpc.unwrap();
    let rkat_mcp = rkat_mcp.unwrap();

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "scenario-51-shared";
    let mut rpc = spawn_stdio_process(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
        ],
        Some(&api_key),
    )
    .await?;
    let _ = rpc_call(&mut rpc, 1, "initialize", json!({}), 20).await?;
    let created = rpc_call(
        &mut rpc,
        2,
        "session/create",
        json!({
            "prompt": "Remember the parity marker SharedRiver51 and reply briefly.",
            "model": smoke_model(),
        }),
        180,
    )
    .await?;
    let session_id = created["session_id"]
        .as_str()
        .ok_or("rpc session/create missing session_id")?
        .to_string();
    shutdown_stdio_process(rpc).await?;

    let mut mcp = spawn_stdio_process(
        &rkat_mcp,
        &project_dir,
        &[
            "--realm",
            realm_id,
            "--instance",
            "scenario-51-mcp",
            "--realm-backend",
            "redb",
            "--state-root",
            state_root.to_str().unwrap(),
            "--context-root",
            project_dir.to_str().unwrap(),
            "--expose-paths",
        ],
        Some(&api_key),
    )
    .await?;
    initialize_mcp(&mut mcp, 30).await?;

    let read = parse_mcp_tool_payload(
        &mcp_call_tool(
            &mut mcp,
            10,
            "meerkat_read",
            json!({ "session_id": session_id }),
            30,
        )
        .await?,
    )?;
    assert_eq!(read["session_id"], session_id);

    let resumed = parse_mcp_tool_payload(
        &mcp_call_tool(
            &mut mcp,
            11,
            "meerkat_resume",
            json!({
                "session_id": session_id,
                "prompt": "What parity marker was I asked to remember? Reply with the marker."
            }),
            180,
        )
        .await?,
    )?;
    let resumed_text = resumed["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        resumed_text.contains("sharedriver51") || resumed_text.contains("shared river 51"),
        "MCP resume should observe RPC-created session state: {resumed}"
    );

    let opened = parse_mcp_tool_payload(
        &mcp_call_tool(
            &mut mcp,
            12,
            "meerkat_event_stream_open",
            json!({ "session_id": session_id }),
            30,
        )
        .await?,
    )?;
    let stream_id = opened["stream_id"]
        .as_str()
        .ok_or("missing stream_id from MCP event stream open")?
        .to_string();

    let follow_up = parse_mcp_tool_payload(
        &mcp_call_tool(
            &mut mcp,
            13,
            "meerkat_resume",
            json!({
                "session_id": session_id,
                "prompt": "Repeat the parity marker again in one short sentence."
            }),
            180,
        )
        .await?,
    )?;
    let follow_up_text = follow_up["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        follow_up_text.contains("sharedriver51") || follow_up_text.contains("shared river 51"),
        "second MCP resume should keep session continuity before stream reads: {follow_up}"
    );

    let mut saw_event = false;
    for request_id in 20..34 {
        let event = parse_mcp_tool_payload(
            &mcp_call_tool(
                &mut mcp,
                request_id,
                "meerkat_event_stream_read",
                json!({ "stream_id": stream_id, "timeout_ms": 2_000 }),
                30,
            )
            .await?,
        )?;
        if event["status"] == "event" {
            saw_event = true;
            break;
        }
    }
    assert!(
        saw_event,
        "MCP event stream should observe at least one session event"
    );

    let closed = parse_mcp_tool_payload(
        &mcp_call_tool(
            &mut mcp,
            40,
            "meerkat_event_stream_close",
            json!({ "stream_id": stream_id }),
            30,
        )
        .await?,
    )?;
    assert_eq!(closed["closed"], true);

    shutdown_stdio_process(mcp).await?;
    Ok(())
}

// ===========================================================================
// Scenario 52: CLI -> RPC -> CLI shared-realm session continuity
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: shared realm cli/rpc live API"]
async fn e2e_scenario_52_cli_rpc_shared_realm_roundtrip() -> Result<(), Box<dyn std::error::Error>>
{
    let rkat = binary_path("rkat");
    let rkat_rpc = binary_path("rkat-rpc");
    if skip_if_missing_binary(&rkat, "rkat") || skip_if_missing_binary(&rkat_rpc, "rkat-rpc") {
        return Ok(());
    }
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: no Anthropic API key configured");
        return Ok(());
    };
    let rkat = rkat.unwrap();
    let rkat_rpc = rkat_rpc.unwrap();

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "scenario-52-shared";
    let run_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "--realm-backend",
        "redb",
        "run",
        "My codename is SharedOtter. Reply briefly.",
        "--output",
        "json",
    ];
    let run_out =
        run_binary_with_redb_retry(&rkat, &project_dir, &run_args, Some(&api_key)).await?;
    let run_stdout = output_ok_or_err(run_out, "rkat", &run_args).map_err(std::io::Error::other)?;
    let session_id = extract_json_string_field(&run_stdout, "session_id")
        .ok_or("session_id missing from CLI run output")?;

    let rpc_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "--realm-backend",
        "redb",
        "--context-root",
        project_dir.to_str().unwrap(),
    ];
    let mut rpc = spawn_stdio_process(&rkat_rpc, &project_dir, &rpc_args, Some(&api_key)).await?;

    rpc_send_line(
        &mut rpc,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .await?;
    let init = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        init["error"].is_null()
            && init["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods
                    .iter()
                    .any(|value| value.as_str() == Some("session/create"))),
        "rpc initialize should succeed and advertise session methods: {init}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"session/read","params":{{"session_id":"{session_id}"}}}}"#
        ),
    )
    .await?;
    let read = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        read["error"].is_null() && read["result"]["session_id"] == session_id,
        "rpc session/read should see the CLI-created session: {read}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"turn/start","params":{{"session_id":"{session_id}","prompt":"What is my codename? Reply with just the codename."}}}}"#
        ),
    )
    .await?;
    let turn = parse_json_line(&rpc_read_response_line(&mut rpc, 180).await?)?;
    assert!(
        turn["error"].is_null()
            && turn["result"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains("sharedotter"),
        "rpc turn/start should recall the CLI-established state: {turn}"
    );
    shutdown_stdio_process(rpc).await?;

    let resume_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "resume",
        &session_id,
        "What is my codename now? Reply with just the codename.",
    ];
    let resume_out =
        run_binary_with_redb_retry(&rkat, &project_dir, &resume_args, Some(&api_key)).await?;
    let resume_stdout =
        output_ok_or_err(resume_out, "rkat", &resume_args).map_err(std::io::Error::other)?;
    assert!(
        resume_stdout.to_lowercase().contains("sharedotter"),
        "CLI resume should observe the RPC-authored continuation: {resume_stdout}"
    );

    Ok(())
}

// ===========================================================================
// Scenario 53: CLI -> REST -> CLI shared-realm continuity
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: shared realm cli/rest live API"]
async fn e2e_scenario_53_cli_rest_shared_realm_roundtrip() -> Result<(), Box<dyn std::error::Error>>
{
    let rkat = binary_path("rkat");
    let rkat_rest = binary_path("rkat-rest");
    if skip_if_missing_binary(&rkat, "rkat") || skip_if_missing_binary(&rkat_rest, "rkat-rest") {
        return Ok(());
    }
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: no Anthropic API key configured");
        return Ok(());
    };
    let rkat = rkat.unwrap();
    let rkat_rest = rkat_rest.unwrap();

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let realm_id = "scenario-53-shared";
    let run_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "--realm-backend",
        "redb",
        "run",
        "My codename is SharedHeron. Reply briefly.",
        "--output",
        "json",
    ];
    let run_out =
        run_binary_with_redb_retry(&rkat, &project_dir, &run_args, Some(&api_key)).await?;
    let run_stdout = output_ok_or_err(run_out, "rkat", &run_args).map_err(std::io::Error::other)?;
    let session_id = extract_json_string_field(&run_stdout, "session_id")
        .ok_or("session_id missing from CLI run output")?;

    let port = allocate_port();
    let realm_paths = meerkat_store::realm_paths_in(&state_root, realm_id);
    tokio::fs::create_dir_all(realm_paths.root.clone()).await?;
    let rest_config = format!(
        "[agent]\nmodel = \"{}\"\nmax_tokens_per_turn = 256\nbudget_warning_threshold = 0.8\n\n[rest]\nhost = \"127.0.0.1\"\nport = {port}\n",
        smoke_model()
    );
    tokio::fs::write(&realm_paths.config_path, rest_config).await?;

    let mut rest = Command::new(&rkat_rest);
    rest.current_dir(&project_dir)
        .env("HOME", &project_dir)
        .env("XDG_DATA_HOME", project_dir.join("data"))
        .env("ANTHROPIC_API_KEY", &api_key)
        .env("RKAT_ANTHROPIC_API_KEY", &api_key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args([
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--realm-backend",
            "redb",
            "--context-root",
            project_dir.to_str().unwrap(),
            "--instance",
            "scenario-53-rest",
        ]);
    let rest_child = wait_for_rest_server(rest.spawn()?, port).await?;

    let get_response = http_request(
        port,
        format!(
            "GET /sessions/{session_id} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await?;
    assert!(
        get_response.starts_with("HTTP/1.1 200") && http_body(&get_response).contains(&session_id),
        "REST GET should observe the CLI-created session: {get_response}"
    );

    let continue_body = format!(
        r#"{{"session_id":"{session_id}","prompt":"What is my codename? Reply with just the codename."}}"#
    );
    let continue_response = http_request(
        port,
        format!(
            "POST /sessions/{session_id}/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            continue_body.len(),
            continue_body
        ),
    )
    .await?;
    let continue_body = http_body(&continue_response).to_string();
    assert!(
        continue_response.starts_with("HTTP/1.1 200")
            && continue_body.to_lowercase().contains("sharedheron"),
        "REST continue should recall the CLI-established state: {continue_response}"
    );

    shutdown_child(rest_child).await?;

    let resume_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "resume",
        &session_id,
        "Confirm the codename one more time.",
    ];
    let resume_out =
        run_binary_with_redb_retry(&rkat, &project_dir, &resume_args, Some(&api_key)).await?;
    let resume_stdout =
        output_ok_or_err(resume_out, "rkat", &resume_args).map_err(std::io::Error::other)?;
    assert!(
        resume_stdout.to_lowercase().contains("sharedheron"),
        "CLI resume should observe the REST-authored continuation: {resume_stdout}"
    );

    Ok(())
}

// ===========================================================================
// Scenario 54: shared-realm mob deployment and CLI visibility
// ===========================================================================

#[tokio::test]
#[ignore = "integration-real: shared realm cli mob rpc surface"]
async fn e2e_scenario_54_shared_realm_mob_sessions_visible_to_cli()
-> Result<(), Box<dyn std::error::Error>> {
    let rkat = binary_path("rkat");
    let rkat_rpc = binary_path("rkat-rpc");
    if skip_if_missing_binary(&rkat, "rkat") || skip_if_missing_binary(&rkat_rpc, "rkat-rpc") {
        return Ok(());
    }
    let rkat = rkat.unwrap();
    let rkat_rpc = rkat_rpc.unwrap();
    let Some(api_key) = anthropic_api_key() else {
        eprintln!("Skipping: no Anthropic API key configured");
        return Ok(());
    };

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    write_project_config(&project_dir).await?;

    let mob_id = "scenario-54-mob";
    let realm_id = "scenario-54-shared";
    let mob_dir = write_cli_mobpack_fixture(&project_dir, mob_id).await?;
    let pack = project_dir.join("scenario-54.mobpack");

    let pack_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "mob",
        "pack",
        mob_dir.to_str().unwrap(),
        "-o",
        pack.to_str().unwrap(),
    ];
    let pack_out = run_binary(&rkat, &project_dir, &pack_args, None).await?;
    let _ = output_ok_or_err(pack_out, "rkat", &pack_args).map_err(std::io::Error::other)?;

    let deploy_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "--realm-backend",
        "redb",
        "mob",
        "deploy",
        pack.to_str().unwrap(),
        "bootstrap",
        "--surface",
        "rpc",
        "--trust-policy",
        "permissive",
    ];
    let mut rpc = spawn_stdio_process(&rkat, &project_dir, &deploy_args, None).await?;

    rpc_send_line(
        &mut rpc,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .await?;
    let init = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        init["error"].is_null()
            && init["result"]["methods"]
                .as_array()
                .is_some_and(|methods| methods
                    .iter()
                    .any(|value| value.as_str() == Some("mob/spawn_many"))),
        "deployed mob rpc surface should initialize cleanly: {init}"
    );

    rpc_send_line(
        &mut rpc,
        r#"{"jsonrpc":"2.0","id":2,"method":"mob/list","params":{}}"#,
    )
    .await?;
    let list = rpc_read_response_line(&mut rpc, 20).await?;
    assert!(
        list.contains(mob_id),
        "deployed mob should appear in mob/list: {list}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"mob/spawn_many","params":{{"mob_id":"{mob_id}","specs":[{{"profile":"lead","meerkat_id":"lead-1","runtime_mode":"turn_driven"}},{{"profile":"worker","meerkat_id":"worker-1","runtime_mode":"turn_driven"}},{{"profile":"reviewer","meerkat_id":"reviewer-1","runtime_mode":"turn_driven"}}]}}}}"#
        ),
    )
    .await?;
    let spawned = parse_json_line(&rpc_read_response_line(&mut rpc, 30).await?)?;
    assert!(
        spawned["error"].is_null()
            && spawned["result"]["results"]
                .as_array()
                .is_some_and(|results| results.iter().all(|entry| entry["ok"] == true)),
        "mob/spawn_many should succeed on shared realm surface: {spawned}"
    );
    let session_ids = extract_all_json_string_fields(&spawned.to_string(), "session_id");
    assert!(
        session_ids.len() >= 3,
        "expected session ids for spawned mob members: {spawned}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"mob/wire","params":{{"mob_id":"{mob_id}","a":"lead-1","b":"worker-1"}}}}"#
        ),
    )
    .await?;
    let wire = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        wire["error"].is_null() && wire["result"]["wired"] == true,
        "mob/wire should succeed: {wire}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"mob/unwire","params":{{"mob_id":"{mob_id}","a":"lead-1","b":"worker-1"}}}}"#
        ),
    )
    .await?;
    let unwire = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        unwire["error"].is_null() && unwire["result"]["unwired"] == true,
        "mob/unwire should succeed: {unwire}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":6,"method":"mob/events","params":{{"mob_id":"{mob_id}","after_cursor":0,"limit":200}}}}"#
        ),
    )
    .await?;
    let events = rpc_read_response_line(&mut rpc, 20).await?;
    assert!(
        events.contains("peers_wired") || events.contains("peers_unwired"),
        "mob event ledger should record wiring transitions: {events}"
    );

    shutdown_stdio_process(rpc).await?;

    let mut ordinary_rpc = spawn_stdio_process(
        &rkat_rpc,
        &project_dir,
        &[
            "--state-root",
            state_root.to_str().unwrap(),
            "--realm",
            realm_id,
            "--realm-backend",
            "redb",
            "--context-root",
            project_dir.to_str().unwrap(),
        ],
        Some(&api_key),
    )
    .await?;
    rpc_send_line(
        &mut ordinary_rpc,
        r#"{"jsonrpc":"2.0","id":348,"method":"initialize","params":{}}"#,
    )
    .await?;
    let ordinary_init = parse_json_line(&rpc_read_response_line(&mut ordinary_rpc, 20).await?)?;
    assert!(
        ordinary_init["error"].is_null(),
        "ordinary rpc surface should initialize cleanly: {ordinary_init}"
    );
    rpc_send_line(
        &mut ordinary_rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":350,"method":"session/create","params":{{"prompt":"Create an ordinary session with a mob-shaped comms name and confirm ORDINARY_SHAPED_54.","model":"{}","comms_name":"{mob_id}/reviewer/alice"}}}}"#,
            smoke_model()
        ),
    )
    .await?;
    let ordinary = parse_json_line(&rpc_read_response_line(&mut ordinary_rpc, 20).await?)?;
    assert!(
        ordinary["error"].is_null()
            && ordinary["result"]["text"]
                .as_str()
                .is_some_and(|text| text.to_uppercase().contains("ORDINARY_SHAPED_54")),
        "ordinary session/create with mob-shaped comms name should still succeed: {ordinary}"
    );
    let ordinary_session_id = ordinary["result"]["session_id"]
        .as_str()
        .ok_or("ordinary session/create missing session_id")?
        .to_string();
    shutdown_stdio_process(ordinary_rpc).await?;

    let mut rpc = spawn_stdio_process(&rkat, &project_dir, &deploy_args, None).await?;
    rpc_send_line(
        &mut rpc,
        r#"{"jsonrpc":"2.0","id":349,"method":"initialize","params":{}}"#,
    )
    .await?;
    let reinit = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        reinit["error"].is_null(),
        "restarted mob rpc surface should initialize cleanly: {reinit}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":351,"method":"session/read","params":{{"session_id":"{ordinary_session_id}"}}}}"#
        ),
    )
    .await?;
    let ordinary_read = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        ordinary_read["error"].is_null()
            && ordinary_read["result"]["session_id"].as_str() == Some(&ordinary_session_id),
        "mob routing must not steal an ordinary session just because its comms_name looks mob-shaped: {ordinary_read}"
    );

    rpc_send_line(
        &mut rpc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":352,"method":"session/archive","params":{{"session_id":"{ordinary_session_id}"}}}}"#
        ),
    )
    .await?;
    let ordinary_archive = parse_json_line(&rpc_read_response_line(&mut rpc, 20).await?)?;
    assert!(
        ordinary_archive["error"].is_null(),
        "ordinary session/archive should stay on the generic session path after reopen even when a mob with the same prefix exists: {ordinary_archive}"
    );

    let sessions_args = [
        "--state-root",
        state_root.to_str().unwrap(),
        "--realm",
        realm_id,
        "sessions",
        "list",
        "--limit",
        "20",
    ];
    shutdown_stdio_process(rpc).await?;

    let sessions_out =
        run_binary_with_redb_retry(&rkat, &project_dir, &sessions_args, None).await?;
    let sessions_stdout =
        output_ok_or_err(sessions_out, "rkat", &sessions_args).map_err(std::io::Error::other)?;
    for session_id in session_ids.iter().take(3) {
        assert!(
            sessions_stdout.contains(session_id),
            "CLI sessions list should surface mob member session {session_id}: {sessions_stdout}"
        );
    }

    Ok(())
}
