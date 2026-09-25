#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

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
    let workspace_root = manifest_dir.parent()?.parent()?;
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

fn assert_atif_trajectory(raw: &str) -> serde_json::Value {
    let trajectory: serde_json::Value =
        serde_json::from_str(raw).expect("trajectory file is valid JSON");
    assert_eq!(
        trajectory["schema_version"].as_str(),
        Some("ATIF-v1.7"),
        "trajectory carries the ATIF schema version"
    );
    let steps = trajectory["steps"]
        .as_array()
        .expect("trajectory has a steps array");
    assert!(
        steps
            .iter()
            .any(|step| step["source"].as_str() == Some("user")),
        "trajectory contains the user turn"
    );
    assert!(
        steps
            .iter()
            .any(|step| step["source"].as_str() == Some("agent")),
        "trajectory contains an agent turn"
    );
    trajectory
}

struct RunFixture {
    rkat: PathBuf,
    home_dir: PathBuf,
    data_dir: PathBuf,
    project_dir: PathBuf,
}

impl RunFixture {
    /// One deterministic turn against the test client shim; --output json also
    /// guards the pure-JSON stdout contract. Returns (session_id, session_ref).
    async fn run_turn(
        &self,
        extra_args: &[&str],
    ) -> Result<(String, String), Box<dyn std::error::Error>> {
        let mut args = vec![
            "run",
            "Say the word 'ok' and nothing else.",
            "--yolo",
            "--output",
            "json",
        ];
        args.extend_from_slice(extra_args);
        let run_output = timeout(
            Duration::from_secs(120),
            Command::new(&self.rkat)
                .current_dir(&self.project_dir)
                .env("HOME", &self.home_dir)
                .env("XDG_DATA_HOME", &self.data_dir)
                .env("RKAT_TEST_CLIENT", "1")
                .args(&args)
                .output(),
        )
        .await??;
        assert!(
            run_output.status.success(),
            "rkat run {extra_args:?} failed: {}",
            String::from_utf8_lossy(&run_output.stderr)
        );
        let run_stdout = String::from_utf8_lossy(&run_output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(run_stdout.trim())
            .map_err(|error| format!("run stdout is not pure JSON ({error}): {run_stdout}"))?;
        let session_id = parsed["session_id"]
            .as_str()
            .ok_or("session_id missing in run response")?
            .to_string();
        let session_ref = parsed["session_ref"]
            .as_str()
            .ok_or("session_ref missing in run response")?
            .to_string();
        Ok((session_id, session_ref))
    }

    /// Resolve the realm trajectory directory used by the auto-export.
    async fn trajectories_dir(
        &self,
        session_ref: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let realm_id = session_ref
            .split_once(':')
            .map(|(realm_part, _)| realm_part)
            .ok_or("session_ref missing realm prefix")?;
        let realm_show = Command::new(&self.rkat)
            .current_dir(&self.project_dir)
            .env("HOME", &self.home_dir)
            .env("XDG_DATA_HOME", &self.data_dir)
            .args(["realm", "show", realm_id])
            .output()
            .await?;
        assert!(
            realm_show.status.success(),
            "rkat realm show failed: {}",
            String::from_utf8_lossy(&realm_show.stderr)
        );
        let realm_show_stdout = String::from_utf8_lossy(&realm_show.stdout);
        let state_root = realm_show_stdout
            .lines()
            .find_map(|line| line.strip_prefix("state_root: "))
            .map(PathBuf::from)
            .ok_or_else(|| {
                format!("state_root missing in realm show output: {realm_show_stdout}")
            })?;
        Ok(state_root.join(realm_id).join("trajectories"))
    }
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_export_atif() -> Result<(), Box<dyn std::error::Error>> {
    let Some(rkat) = rkat_binary_path() else {
        eprintln!("Skipping: missing rkat binary (build with cargo build -p meerkat-cli)");
        return Ok(());
    };

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;
    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;
    let fixture = RunFixture {
        rkat,
        home_dir: temp_dir.path().to_path_buf(),
        data_dir,
        project_dir,
    };

    // Negative arm: without --export-atif no trajectory is written (default off).
    let (plain_session_id, plain_session_ref) = fixture.run_turn(&[]).await?;
    let trajectories_dir = fixture.trajectories_dir(&plain_session_ref).await?;
    let plain_trajectory_path = trajectories_dir.join(format!("{plain_session_id}.json"));
    assert!(
        !plain_trajectory_path.exists(),
        "run without --export-atif must not write a trajectory, found {}",
        plain_trajectory_path.display()
    );

    // Flagged arm: --export-atif persists a valid trajectory for the session.
    let (flagged_session_id, _) = fixture.run_turn(&["--export-atif"]).await?;
    let flagged_trajectory_path = trajectories_dir.join(format!("{flagged_session_id}.json"));
    let auto_exported = tokio::fs::read_to_string(&flagged_trajectory_path)
        .await
        .map_err(|error| {
            format!(
                "auto-exported trajectory missing at {}: {error}",
                flagged_trajectory_path.display()
            )
        })?;
    let auto_trajectory = assert_atif_trajectory(&auto_exported);
    assert_eq!(
        auto_trajectory["session_id"].as_str(),
        Some(flagged_session_id.as_str()),
        "auto-exported trajectory names its session"
    );

    // Explicit export works for a session that was never auto-exported.
    let export_path = temp_dir.path().join("exported-trajectory.json");
    let export_output = timeout(
        Duration::from_secs(60),
        Command::new(&fixture.rkat)
            .current_dir(&fixture.project_dir)
            .env("HOME", &fixture.home_dir)
            .env("XDG_DATA_HOME", &fixture.data_dir)
            .args([
                "session",
                "export-atif",
                &plain_session_id,
                "--output",
                export_path.to_str().ok_or("export path is not UTF-8")?,
            ])
            .output(),
    )
    .await??;
    assert!(
        export_output.status.success(),
        "rkat session export-atif failed: {}",
        String::from_utf8_lossy(&export_output.stderr)
    );
    let export_stdout = String::from_utf8_lossy(&export_output.stdout);
    assert!(
        export_stdout.contains("Wrote ATIF trajectory to"),
        "export announces its destination: {export_stdout}"
    );
    let exported = tokio::fs::read_to_string(&export_path).await?;
    let trajectory = assert_atif_trajectory(&exported);
    assert_eq!(
        trajectory["session_id"].as_str(),
        Some(plain_session_id.as_str()),
        "trajectory names the exported session"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Run output fidelity: every provider request of a run reaches the ATIF
// trajectory, the `--verbose` lines and total, and the session event log.
// ---------------------------------------------------------------------------

const FIDELITY_REALM: &str = "run-fidelity";
const FIDELITY_MODEL: &str = "fidelity-model";
/// Enough streamed deltas that the durable event projection trails the
/// session: an exit that does not wait for it leaves the log mid-stream.
const FIDELITY_ANSWER_DELTAS: usize = 1500;

/// Scripted OpenAI-compatible chat-completions server. The first request of
/// a run asks for the `datetime` tool, the request carrying the tool result
/// answers in many deltas, and a structured-output extraction request
/// answers with JSON. Each request reports distinct usage.
struct ScriptedChatServer {
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ScriptedChatServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn sse_chunk(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn chat_usage(prompt: u64, completion: u64, cached: u64, reasoning: u64) -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "prompt_tokens_details": { "cached_tokens": cached },
        "completion_tokens_details": { "reasoning_tokens": reasoning },
    })
}

fn scripted_chat_body(request: &serde_json::Value) -> String {
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let content = match &last["content"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let mut body = String::new();
    if content.contains("valid JSON") {
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{ "delta": { "content": "{\"answer\": \"ok\"}" }, "finish_reason": null }]
        })));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        ));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [], "usage": chat_usage(1300, 30, 1200, 5) }),
        ));
    } else if last["role"].as_str() == Some("tool") {
        for index in 0..FIDELITY_ANSWER_DELTAS {
            body.push_str(&sse_chunk(serde_json::json!({
                "choices": [{ "delta": { "content": format!("w{index} ") }, "finish_reason": null }]
            })));
        }
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        ));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [], "usage": chat_usage(1200, 20, 1000, 4) }),
        ));
    } else {
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{
                "delta": { "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "datetime", "arguments": "{}" }
                }] },
                "finish_reason": null
            }]
        })));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
        ));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [], "usage": chat_usage(1000, 10, 0, 3) }),
        ));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

async fn serve_scripted_chat_connection(
    mut socket: tokio::net::TcpStream,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    let request: serde_json::Value =
        serde_json::from_slice(&buffer[header_end..header_end + content_length])
            .unwrap_or_default();
    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let body = scripted_chat_body(&request);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

async fn install_scripted_chat_server(state_root: &std::path::Path) -> ScriptedChatServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scripted chat server");
    let port = listener.local_addr().expect("scripted chat address").port();
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let served = std::sync::Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(serve_scripted_chat_connection(
                socket,
                std::sync::Arc::clone(&served),
            ));
        }
    });
    let realm_dir = state_root.join(FIDELITY_REALM);
    tokio::fs::create_dir_all(&realm_dir)
        .await
        .expect("realm config dir");
    tokio::fs::write(
        realm_dir.join("config.toml"),
        format!(
            r#"[self_hosted.servers.scripted]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{FIDELITY_MODEL}"]
server = "scripted"
remote_model = "{FIDELITY_MODEL}"

[realm."{FIDELITY_REALM}"]
default_binding = "scripted"

[realm."{FIDELITY_REALM}".backend.scripted]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "scripted"

[realm."{FIDELITY_REALM}".auth.scripted]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{FIDELITY_REALM}".binding.scripted]
backend_profile = "scripted"
auth_profile = "scripted"
"#
        ),
    )
    .await
    .expect("write scripted provider config");
    ScriptedChatServer { requests, task }
}

struct FidelityRun {
    stdout: String,
    stderr: String,
    result: serde_json::Value,
}

async fn run_fidelity_turn(
    rkat: &std::path::Path,
    home: &std::path::Path,
    state_root: &std::path::Path,
    extra_args: &[&str],
) -> FidelityRun {
    let mut command = Command::new(rkat);
    for name in [
        "RKAT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "GEMINI_API_KEY",
        "XAI_API_KEY",
        "RKAT_TEST_CLIENT",
    ] {
        command.env_remove(name);
    }
    command
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .arg("--state-root")
        .arg(state_root)
        .args(["--realm", FIDELITY_REALM, "run"])
        .args(extra_args)
        .args([
            "--output",
            "json",
            "--stream",
            "--verbose",
            "-m",
            FIDELITY_MODEL,
            "--provider",
            "self-hosted",
            "Check the time, then answer.",
        ])
        .stdin(std::process::Stdio::null());
    let output = timeout(Duration::from_secs(180), command.output())
        .await
        .expect("rkat run within budget")
        .expect("spawn rkat run");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "rkat run {extra_args:?} failed: {stderr}"
    );
    // Streamed text precedes the pretty-printed result object on stdout.
    let result_start = stdout
        .rfind("\n{\n")
        .map(|index| index + 1)
        .expect("stdout ends with the JSON run result");
    let result = serde_json::from_str(&stdout[result_start..]).expect("run result is JSON");
    FidelityRun {
        stdout,
        stderr,
        result,
    }
}

fn session_event_types(state_root: &std::path::Path, session_id: &str) -> Vec<String> {
    let path = state_root
        .join(FIDELITY_REALM)
        .join(".rkat/sessions")
        .join(session_id)
        .join("events.jsonl");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("events.jsonl at {}: {error}", path.display()));
    raw.lines()
        .map(|line| {
            let row: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("events.jsonl row is complete JSON ({error}): {line}")
            });
            row["event"]["type"]
                .as_str()
                .expect("event row names its type")
                .to_string()
        })
        .collect()
}

/// Token numbers of the `--verbose` per-request lines (`  N tokens (I in /
/// O out...`, optionally `extraction: ` labelled) and of the total line.
fn verbose_token_lines(stderr: &str) -> (Vec<(u64, u64)>, Option<(u64, u64)>) {
    fn in_out(summary: &str) -> Option<(u64, u64)> {
        let details = summary.split_once(" tokens (")?.1;
        let (input, rest) = details.split_once(" in / ")?;
        let output = rest.split(" out").next()?;
        Some((input.trim().parse().ok()?, output.trim().parse().ok()?))
    }
    let mut requests = Vec::new();
    let mut total = None;
    for line in stderr.lines() {
        let trimmed = line.trim_start();
        if let Some(summary) = trimmed.strip_prefix("total: ") {
            total = in_out(summary);
        } else if line.starts_with("  ") {
            let summary = trimmed.strip_prefix("extraction: ").unwrap_or(trimmed);
            if summary
                .split_once(" tokens (")
                .is_some_and(|(count, _)| count.chars().all(|c| c.is_ascii_digit()))
                && let Some(numbers) = in_out(summary)
            {
                requests.push(numbers);
            }
        }
    }
    (requests, total)
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_run_output_fidelity() {
    let Some(rkat) = rkat_binary_path() else {
        eprintln!("Skipping: missing rkat binary (build with cargo build -p rkat)");
        return;
    };
    let temp_dir = TempDir::new().expect("temp dir");
    let home = temp_dir.path().to_path_buf();
    let state_root = home.join("realms");
    let server = install_scripted_chat_server(&state_root).await;

    // ---- A plain tool-using run: the event log ends with run_completed. ----
    let plain = run_fidelity_turn(&rkat, &home, &state_root, &[]).await;
    let plain_session = plain.result["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();
    let plain_types = session_event_types(&state_root, &plain_session);
    assert_eq!(
        plain_types.last().map(String::as_str),
        Some("run_completed"),
        "the event log must end with the run's terminal event, not mid-stream"
    );
    assert_eq!(
        plain_types
            .iter()
            .filter(|kind| *kind == "text_delta")
            .count(),
        FIDELITY_ANSWER_DELTAS,
        "every streamed delta reached the event log"
    );
    assert_eq!(server.requests.load(std::sync::atomic::Ordering::SeqCst), 2);
    let (plain_requests, plain_total) = verbose_token_lines(&plain.stderr);
    assert_eq!(
        plain_requests,
        vec![(1000, 10), (1200, 20)],
        "one verbose line per provider request, the first included: {}",
        plain.stderr
    );
    assert_eq!(plain_total, Some((2200, 30)));

    // ---- A structured-output run with ATIF export. -------------------------
    let schema_path = home.join("schema.json");
    tokio::fs::write(
        &schema_path,
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#,
    )
    .await
    .expect("write schema");
    let structured = run_fidelity_turn(
        &rkat,
        &home,
        &state_root,
        &[
            "--export-atif",
            "--schema",
            schema_path.to_str().expect("utf8 schema path"),
        ],
    )
    .await;
    assert_eq!(
        server.requests.load(std::sync::atomic::Ordering::SeqCst),
        5,
        "tool call, answer, and one extraction request"
    );
    assert_eq!(
        structured.result["structured_output"],
        serde_json::json!({"answer": "ok"}),
        "stdout: {}",
        structured.stdout
    );
    let run_usage = &structured.result["run_usage"];
    assert_eq!(run_usage["input_tokens"].as_u64(), Some(3500));
    assert_eq!(run_usage["output_tokens"].as_u64(), Some(60));

    // Verbose: every request, the extraction included, and the run's total.
    let (requests, total) = verbose_token_lines(&structured.stderr);
    assert_eq!(
        requests,
        vec![(1000, 10), (1200, 20), (1300, 30)],
        "verbose lines cover the tool call, the answer, and the extraction: {}",
        structured.stderr
    );
    assert_eq!(
        total,
        Some((3500, 60)),
        "the verbose total is the run's own usage, extraction included"
    );

    // Event log: complete through the extraction outcome.
    let session_id = structured.result["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();
    let types = session_event_types(&state_root, &session_id);
    assert!(types.iter().any(|kind| kind == "run_completed"));
    assert_eq!(
        types.last().map(String::as_str),
        Some("extraction_succeeded"),
        "a structured-output run's log ends at its extraction outcome"
    );

    // ATIF: a step per provider request, each with metrics.
    let trajectory_path = state_root
        .join(FIDELITY_REALM)
        .join("trajectories")
        .join(format!("{session_id}.json"));
    let trajectory = assert_atif_trajectory(
        &tokio::fs::read_to_string(&trajectory_path)
            .await
            .expect("auto-exported trajectory"),
    );
    let agent_steps = trajectory["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .filter(|step| step["source"].as_str() == Some("agent"))
        .collect::<Vec<_>>();
    let prompt_tokens = agent_steps
        .iter()
        .map(|step| step["metrics"]["prompt_tokens"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(
        prompt_tokens,
        vec![Some(1000), Some(1200), Some(1300)],
        "tool-call turn, answer, and extraction are each a metered step: {trajectory}"
    );
    assert_eq!(
        agent_steps[0]["tool_calls"][0]["function_name"].as_str(),
        Some("datetime")
    );
    assert_eq!(
        trajectory["final_metrics"]["total_prompt_tokens"].as_u64(),
        Some(3500)
    );
    assert_eq!(
        trajectory["final_metrics"]["total_completion_tokens"].as_u64(),
        Some(60)
    );
}
