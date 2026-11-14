//! `rkat run --reasoning-effort`, through the built binary.
//!
//! Each row runs the shipped `rkat` against a scripted OpenAI-compatible
//! server on a loopback socket, wired in by the realm's own config file as a
//! self-hosted route whose models declare (or do not declare) the reasoning
//! effort levels they accept. The server records every request body, so a
//! row can say which provider field carried the level, and that a refused
//! level never reached the provider.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::panic
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_mins(3);
const REALM: &str = "reasoning-effort";
/// Declares `low`, `medium` and `high`.
const DECLARED_MODEL: &str = "loopback-declared";
/// Supports reasoning and declares no levels.
const UNDECLARED_MODEL: &str = "loopback-undeclared";
const STREAMED_ANSWER: &str = "the-file-is-written";

fn rkat_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "rkat.exe" } else { "rkat" });
    path
}

fn sse_chunk(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

/// The scripted model: asked for JSON it answers the structured output; after
/// a tool result it streams its answer; otherwise it asks `apply_patch` to
/// add `effect.txt` to the working directory.
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
    let stop = |body: &mut String| {
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        ));
    };
    if content.contains("valid JSON") {
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{ "delta": { "content": "{\"answer\": \"ok\"}" }, "finish_reason": null }]
        })));
        stop(&mut body);
    } else if last["role"].as_str() == Some("tool") || request["tools"].as_array().is_none() {
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{ "delta": { "content": STREAMED_ANSWER }, "finish_reason": null }]
        })));
        stop(&mut body);
    } else {
        let patch =
            "*** Begin Patch\n*** Add File: effect.txt\n+written by the tool\n*** End Patch";
        let arguments = serde_json::json!({ "patch": patch }).to_string();
        body.push_str(&sse_chunk(serde_json::json!({
            "choices": [{
                "delta": { "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "apply_patch", "arguments": arguments }
                }] },
                "finish_reason": null
            }]
        })));
        body.push_str(&sse_chunk(
            serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
        ));
    }
    body.push_str(&sse_chunk(serde_json::json!({
        "choices": [],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2 }
    })));
    body.push_str("data: [DONE]\n\n");
    body
}

/// Read one whole HTTP request; returns its JSON body.
fn read_request(socket: &mut TcpStream) -> Option<serde_json::Value> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
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
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    serde_json::from_slice(&buffer[header_end..header_end + content_length]).ok()
}

/// The loopback provider: its port and every request body it was sent.
struct Loopback {
    port: u16,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Loopback {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
        let port = listener.local_addr().expect("loopback address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else {
                    return;
                };
                let _ = socket.set_read_timeout(Some(Duration::from_secs(30)));
                let Some(request) = read_request(&mut socket) else {
                    continue;
                };
                let body = scripted_chat_body(&request);
                recorded.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
        });
        Self { port, requests }
    }

    fn requests(&self) -> Vec<serde_json::Value> {
        self.requests.lock().unwrap().clone()
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

struct Roots {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
    user: PathBuf,
}

/// Fresh roots whose realm config routes both models to the loopback server.
fn roots(port: u16) -> Roots {
    let temp = tempfile::TempDir::new().unwrap();
    let home = temp.path().to_path_buf();
    let project = temp.path().join("project");
    let state = temp.path().join("state");
    let user = temp.path().join("user");
    let realm = state.join(REALM);
    for dir in [&project, &state, &user, &project.join(".rkat"), &realm] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        realm.join("config.toml"),
        format!(
            r#"[self_hosted]
default_model = "{DECLARED_MODEL}"

[self_hosted.servers.loopback]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{DECLARED_MODEL}"]
server = "loopback"
remote_model = "{DECLARED_MODEL}"
supports_reasoning = true
reasoning_effort_levels = ["low", "medium", "high"]

[self_hosted.models."{UNDECLARED_MODEL}"]
server = "loopback"
remote_model = "{UNDECLARED_MODEL}"
supports_reasoning = true

[realm."{REALM}"]
default_binding = "loopback"

[realm."{REALM}".backend.loopback]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "loopback"

[realm."{REALM}".auth.loopback]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{REALM}".binding.loopback]
backend_profile = "loopback"
auth_profile = "loopback"
"#
        ),
    )
    .unwrap();
    Roots {
        _temp: temp,
        home,
        project,
        state,
        user,
    }
}

struct Finished {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

fn drain(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

fn collect(receiver: mpsc::Receiver<Vec<u8>>) -> String {
    String::from_utf8_lossy(
        &receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_default(),
    )
    .into_owned()
}

/// Run `rkat <root flags> <args>` in the loopback realm with no provider key
/// in its environment and stdin closed; panics at the deadline.
fn rkat(roots: &Roots, args: &[&str]) -> Finished {
    let mut command = Command::new(rkat_binary());
    for name in [
        "RKAT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "GEMINI_API_KEY",
        "XAI_API_KEY",
        "RKAT_TEST_CLIENT",
        "RUST_LOG",
        "RUST_BACKTRACE",
    ] {
        command.env_remove(name);
    }
    command
        .current_dir(&roots.project)
        .env("HOME", &roots.home)
        .env("XDG_CONFIG_HOME", roots.home.join("config"))
        .arg("--realm")
        .arg(REALM)
        .arg("--context-root")
        .arg(&roots.project)
        .arg("--state-root")
        .arg(&roots.state)
        .arg("--user-config-root")
        .arg(&roots.user)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn rkat");
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll rkat") {
            break status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "rkat {args:?} did not finish within {DEADLINE:?}; stdout:\n{}\nstderr:\n{}",
                collect(stdout),
                collect(stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Finished {
        status: status.code(),
        stdout: collect(stdout),
        stderr: collect(stderr),
    }
}

/// `rkat run <extra> -m <model> --provider self-hosted <prompt>`.
fn run_turn(roots: &Roots, model: &str, extra: &[&str]) -> Finished {
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&[
        "-m",
        model,
        "--provider",
        "self-hosted",
        "Write the file, then answer.",
    ]);
    rkat(roots, &args)
}

/// `rkat run --resume <session> <extra> <prompt>`.
fn resume_turn(roots: &Roots, session: &str, extra: &[&str]) -> Finished {
    let mut args = vec!["run", "--resume", session];
    args.extend_from_slice(extra);
    args.push("Continue.");
    rkat(roots, &args)
}

/// The whole of stdout as exactly one JSON document.
fn only_document(finished: &Finished) -> serde_json::Value {
    serde_json::from_str(finished.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout is not exactly one JSON document ({error}); stdout:\n{}\nstderr:\n{}",
            finished.stdout, finished.stderr
        )
    })
}

/// The pretty-printed JSON document that ends stdout, and the text before it.
fn text_then_document(finished: &Finished) -> (String, serde_json::Value) {
    let stdout = finished.stdout.trim_end();
    let start = if stdout.starts_with("{\n") {
        0
    } else {
        stdout
            .rfind("\n{\n")
            .map(|index| index + 1)
            .unwrap_or_else(|| {
                panic!(
                    "stdout ends with no JSON document; stdout:\n{}\nstderr:\n{}",
                    finished.stdout, finished.stderr
                )
            })
    };
    let document = serde_json::from_str(&stdout[start..]).unwrap_or_else(|error| {
        panic!(
            "the last stdout document is not JSON ({error}); stdout:\n{}",
            finished.stdout
        )
    });
    (stdout[..start].to_string(), document)
}

fn assert_ok(finished: &Finished, what: &str) {
    assert_eq!(
        finished.status,
        Some(0),
        "{what} succeeds; stdout:\n{}\nstderr:\n{}",
        finished.stdout,
        finished.stderr
    );
}

/// The level as the chat-completions route carries it: both spellings of
/// the provider field.
fn carried_level(request: &serde_json::Value) -> (Option<&str>, Option<&str>) {
    (
        request["reasoning_effort"].as_str(),
        request["reasoning"]["effort"].as_str(),
    )
}

/// The failure document of a refused level: its typed parts, checked field
/// by field and never by message text. Returns the refusal object.
fn refusal_of(finished: &Finished) -> serde_json::Value {
    assert_eq!(
        finished.status,
        Some(2),
        "a refused level exits 2; stdout:\n{}\nstderr:\n{}",
        finished.stdout,
        finished.stderr
    );
    let document = only_document(finished);
    let object = document
        .as_object()
        .expect("the failure document is an object");
    assert_eq!(
        object.keys().collect::<Vec<_>>(),
        ["error"],
        "one failure document and nothing else: {document}"
    );
    let error = &document["error"];
    assert_eq!(error["code"], "INVALID_PARAMS", "{document}");
    assert_eq!(error["category"], "validation", "{document}");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{document}"
    );
    assert_eq!(
        error["details"]["kind"], "reasoning_effort_refused",
        "{document}"
    );
    assert!(
        finished.stderr.contains("Error: "),
        "stderr keeps its error line:\n{}",
        finished.stderr
    );
    error["details"].clone()
}

/// A supported level completes with workspace tools, a schema and
/// streaming; every provider request carries the route's field; the run
/// result reports what was requested and sent.
#[test]
fn cli_supported_run_reports_requested_and_sent() {
    let provider = Loopback::start();
    let roots = roots(provider.port);
    let schema = roots.home.join("schema.json");
    std::fs::write(
        &schema,
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#,
    )
    .unwrap();

    let finished = run_turn(
        &roots,
        DECLARED_MODEL,
        &[
            "--reasoning-effort",
            "high",
            "--tools",
            "workspace",
            "--schema",
            schema.to_str().unwrap(),
            "--stream",
            "--output",
            "json",
        ],
    );
    assert_ok(&finished, "a run at a declared level");

    let requests = provider.requests();
    assert_eq!(
        requests.len(),
        3,
        "the tool call, the answer and the extraction: {requests:#?}"
    );
    for request in &requests {
        assert_eq!(
            carried_level(request),
            (Some("high"), Some("high")),
            "every provider request carries the level in the route's field: {request}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(roots.project.join("effect.txt"))
            .ok()
            .as_deref(),
        Some("written by the tool\n"),
        "the tool call's effect exists in the working directory"
    );

    let (streamed, result) = text_then_document(&finished);
    assert!(
        streamed.contains(STREAMED_ANSWER),
        "streamed text precedes the result: {:?}",
        finished.stdout
    );
    assert_eq!(
        result["structured_output"],
        serde_json::json!({"answer": "ok"})
    );
    assert_eq!(
        result["reasoning_effort"],
        serde_json::json!({
            "requested": "high",
            "sent": [{
                "level": "high",
                "provider": "self_hosted",
                "model": DECLARED_MODEL,
                "route": {"backend": "self_hosted", "api": "chat_completions"},
                "fields": ["reasoning_effort", "reasoning.effort"],
                "facts": "operator",
                "decided_by": "request",
                "requests": 3
            }]
        }),
        "the result reports the requested level and what was sent: {result}"
    );

    // `--param reasoning_effort=<LEVEL>` is the same setting, alone or
    // beside an equal flag.
    for extra in [
        vec!["--param", "reasoning_effort=low", "--output", "json"],
        vec![
            "--param",
            "reasoning_effort=low",
            "--reasoning-effort",
            "low",
            "--output",
            "json",
        ],
    ] {
        let before = provider.request_count();
        let finished = run_turn(&roots, DECLARED_MODEL, &extra);
        assert_ok(&finished, "a run that names the level as a param");
        let requests = provider.requests();
        assert!(requests.len() > before);
        for request in &requests[before..] {
            assert_eq!(
                carried_level(request),
                (Some("low"), Some("low")),
                "{request}"
            );
        }
        let (_, result) = text_then_document(&finished);
        assert_eq!(result["reasoning_effort"]["requested"], "low", "{result}");
    }

    // A run without the flag reports nothing about a level and sends none.
    let before = provider.request_count();
    let plain = run_turn(&roots, DECLARED_MODEL, &["--output", "json"]);
    assert_ok(&plain, "a run without a level");
    for request in &provider.requests()[before..] {
        assert_eq!(carried_level(request), (None, None), "{request}");
    }
    let (_, result) = text_then_document(&plain);
    assert!(result.get("reasoning_effort").is_none(), "{result}");
}

/// A level the route does not accept, and a model whose levels no
/// source states, are refused before any provider request: exit status 2
/// and one failure document. No session is created.
#[test]
fn cli_refusal_prints_the_failure_document_and_exits_2() {
    let provider = Loopback::start();
    let roots = roots(provider.port);
    let sessions_before = rkat(&roots, &["session", "list"]).stdout;

    // Unsupported: the operator declared low, medium and high.
    let refused = run_turn(
        &roots,
        DECLARED_MODEL,
        &["--reasoning-effort", "max", "--output", "json"],
    );
    let details = refusal_of(&refused);
    assert_eq!(details["retryable"], false, "{details}");
    assert_eq!(
        details["reasoning_effort_refusal"],
        serde_json::json!({
            "requested": "max",
            "provider": "self_hosted",
            "model": DECLARED_MODEL,
            "route": {"backend": "self_hosted", "api": "chat_completions"},
            "reason": "unsupported_level",
            "supported": ["low", "medium", "high"],
            "facts": "operator"
        })
    );

    // Unknown: the model declares no levels.
    let refused = run_turn(
        &roots,
        UNDECLARED_MODEL,
        &["--reasoning-effort", "high", "--stream", "--output", "json"],
    );
    let details = refusal_of(&refused);
    assert_eq!(details["retryable"], false, "{details}");
    assert_eq!(
        details["reasoning_effort_refusal"],
        serde_json::json!({
            "requested": "high",
            "provider": "self_hosted",
            "model": UNDECLARED_MODEL,
            "route": {"backend": "self_hosted", "api": "chat_completions"},
            "reason": "supported_levels_unknown",
            "supported": null,
            "facts": null
        })
    );

    // Text mode: the same exit status, and no document on stdout.
    let text = run_turn(&roots, DECLARED_MODEL, &["--reasoning-effort", "max"]);
    assert_eq!(text.status, Some(2), "stderr:\n{}", text.stderr);
    assert!(text.stdout.trim().is_empty(), "stdout: {:?}", text.stdout);
    assert!(text.stderr.contains("Error: "), "stderr:\n{}", text.stderr);

    // A level and a different level as a param are refused by the CLI
    // itself, before anything runs: an invalid request, exit status 2, as
    // JSON-RPC and REST answer the same disagreement.
    let disagree = run_turn(
        &roots,
        DECLARED_MODEL,
        &[
            "--reasoning-effort",
            "high",
            "--param",
            "reasoning_effort=low",
        ],
    );
    assert_eq!(disagree.status, Some(2), "stderr:\n{}", disagree.stderr);
    assert!(
        disagree.stderr.contains("reasoning_effort is 'high'")
            && disagree
                .stderr
                .contains("provider_params.reasoning_effort is 'low'"),
        "stderr:\n{}",
        disagree.stderr
    );
    assert!(
        disagree.stdout.trim().is_empty(),
        "stdout: {:?}",
        disagree.stdout
    );
    // With JSON output the failure document names the same class: invalid
    // params. The disagreement is not a route's refusal, so the document
    // carries no refusal details.
    let disagree = run_turn(
        &roots,
        DECLARED_MODEL,
        &[
            "--reasoning-effort",
            "high",
            "--param",
            "reasoning_effort=low",
            "--output",
            "json",
        ],
    );
    assert_eq!(disagree.status, Some(2), "stderr:\n{}", disagree.stderr);
    let document = only_document(&disagree);
    assert_eq!(document["error"]["code"], "INVALID_PARAMS", "{document}");
    assert_eq!(document["error"]["category"], "validation", "{document}");
    assert!(document["error"].get("details").is_none(), "{document}");

    assert_eq!(
        provider.request_count(),
        0,
        "no refused run reached the provider: {:#?}",
        provider.requests()
    );
    assert_eq!(
        rkat(&roots, &["session", "list"]).stdout,
        sessions_before,
        "a refused run creates no session"
    );

    // Control: the same route answers a run without the flag.
    let control = run_turn(&roots, UNDECLARED_MODEL, &["--output", "json"]);
    assert_ok(&control, "a control run without the flag");
    assert!(provider.request_count() > 0);
}

/// A refused level on `--resume` exits 2 with
/// the failure document and leaves the stored session as it was; a resume
/// that names no level keeps the session's, one that replaces other params
/// keeps it too, and `--no-reasoning-effort` removes it.
#[test]
fn cli_refusal_on_resume_leaves_the_stored_identity_unchanged() {
    let provider = Loopback::start();
    let roots = roots(provider.port);

    let created = run_turn(
        &roots,
        DECLARED_MODEL,
        &["--reasoning-effort", "low", "--output", "json"],
    );
    assert_ok(&created, "the session's first run");
    let (_, result) = text_then_document(&created);
    let session = result["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();
    let shown_before = rkat(&roots, &["session", "show", &session]);
    assert_ok(&shown_before, "session show");
    let requests_before = provider.request_count();

    // A level outside the declared set is refused on resume.
    let refused = resume_turn(
        &roots,
        &session,
        &["--reasoning-effort", "max", "--output", "json"],
    );
    let details = refusal_of(&refused);
    assert_eq!(
        details["reasoning_effort_refusal"]["reason"],
        "unsupported_level"
    );
    assert_eq!(details["reasoning_effort_refusal"]["requested"], "max");
    assert_eq!(
        details["reasoning_effort_refusal"]["supported"],
        serde_json::json!(["low", "medium", "high"])
    );
    assert_eq!(
        provider.request_count(),
        requests_before,
        "the refused resume reached no provider"
    );
    let shown_after = rkat(&roots, &["session", "show", &session]);
    assert_eq!(
        shown_after.stdout, shown_before.stdout,
        "the stored session is unchanged by the refused resume"
    );

    // A resume that names no level keeps the stored one.
    let kept = resume_turn(&roots, &session, &["--output", "json"]);
    assert_ok(&kept, "a resume that names no level");
    let requests = provider.requests();
    assert!(requests.len() > requests_before);
    for request in &requests[requests_before..] {
        assert_eq!(
            carried_level(request),
            (Some("low"), Some("low")),
            "{request}"
        );
    }
    let (_, result) = text_then_document(&kept);
    assert_eq!(result["reasoning_effort"]["requested"], "low", "{result}");

    // A resume that replaces other provider params keeps it too.
    let before = provider.request_count();
    let replaced = resume_turn(
        &roots,
        &session,
        &["--param", "temperature=0.5", "--output", "json"],
    );
    assert_ok(&replaced, "a resume that replaces provider params");
    for request in &provider.requests()[before..] {
        assert_eq!(
            carried_level(request),
            (Some("low"), Some("low")),
            "{request}"
        );
        assert_eq!(request["temperature"], 0.5, "{request}");
    }

    // A resume can change it.
    let before = provider.request_count();
    let changed = resume_turn(
        &roots,
        &session,
        &["--reasoning-effort", "medium", "--output", "json"],
    );
    assert_ok(&changed, "a resume that sets another level");
    for request in &provider.requests()[before..] {
        assert_eq!(
            carried_level(request),
            (Some("medium"), Some("medium")),
            "{request}"
        );
    }

    // `--no-reasoning-effort` removes it, for this turn and the next.
    let before = provider.request_count();
    let cleared = resume_turn(
        &roots,
        &session,
        &["--no-reasoning-effort", "--output", "json"],
    );
    assert_ok(&cleared, "a resume that clears the level");
    let (_, result) = text_then_document(&cleared);
    assert!(result.get("reasoning_effort").is_none(), "{result}");
    let later = resume_turn(&roots, &session, &["--output", "json"]);
    assert_ok(&later, "the resume after the clear");
    let requests = provider.requests();
    assert!(requests.len() > before);
    for request in &requests[before..] {
        assert_eq!(carried_level(request), (None, None), "{request}");
    }
}

/// A resume that names the level in two places that disagree is the
/// caller's request, refused before anything is built: exit status 2 and,
/// with JSON output, the failure document of an invalid request, as on a
/// new session. The stored session is unchanged and no provider is reached.
#[test]
fn cli_two_spellings_that_disagree_on_resume_are_an_invalid_request() {
    let provider = Loopback::start();
    let roots = roots(provider.port);

    let created = run_turn(
        &roots,
        DECLARED_MODEL,
        &["--reasoning-effort", "low", "--output", "json"],
    );
    assert_ok(&created, "the session's first run");
    let (_, result) = text_then_document(&created);
    let session = result["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();
    let shown_before = rkat(&roots, &["session", "show", &session]);
    assert_ok(&shown_before, "session show");
    let requests_before = provider.request_count();

    // What each command answered, read before anything is asserted, so a
    // failure prints all of them.
    let level_json = resume_turn(
        &roots,
        &session,
        &[
            "--reasoning-effort",
            "high",
            "--param",
            "reasoning_effort=low",
            "--output",
            "json",
        ],
    );
    let level_text = resume_turn(
        &roots,
        &session,
        &[
            "--reasoning-effort",
            "high",
            "--param",
            "reasoning_effort=low",
        ],
    );
    let clear_json = resume_turn(
        &roots,
        &session,
        &[
            "--no-reasoning-effort",
            "--param",
            "reasoning_effort=low",
            "--output",
            "json",
        ],
    );
    let clear_text = resume_turn(
        &roots,
        &session,
        &["--no-reasoning-effort", "--param", "reasoning_effort=low"],
    );
    let code_of = |finished: &Finished| {
        serde_json::from_str::<serde_json::Value>(finished.stdout.trim())
            .ok()
            .and_then(|document| document["error"]["code"].as_str().map(str::to_string))
    };
    let observed = (
        (level_json.status, code_of(&level_json)),
        level_text.status,
        (clear_json.status, code_of(&clear_json)),
        clear_text.status,
    );
    assert_eq!(
        observed,
        (
            (Some(2), Some("INVALID_PARAMS".to_string())),
            Some(2),
            (Some(2), Some("INVALID_PARAMS".to_string())),
            Some(2),
        ),
        "exit status and document code of (a level beside another nested level, json), (the same, \
         text), (a clear beside a nested level, json), (the same, text)\nstdout of the first: {}\n\
         stderr of the second: {}\nstdout of the third: {}\nstderr of the fourth: {}",
        level_json.stdout,
        level_text.stderr,
        clear_json.stdout,
        clear_text.stderr
    );

    // The documents are an invalid request's: validation, and no refusal
    // details (the disagreement is not a route's refusal).
    for (finished, places) in [
        (
            &level_json,
            "reasoning_effort is 'high' but provider_params.reasoning_effort is 'low'",
        ),
        (
            &clear_json,
            "reasoning_effort is cleared but provider_params.reasoning_effort is 'low'",
        ),
    ] {
        let document = only_document(finished);
        assert_eq!(document["error"]["category"], "validation", "{document}");
        assert!(document["error"].get("details").is_none(), "{document}");
        // The same text the create path's document carries: both places.
        assert!(
            document["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(places)),
            "{document}"
        );
    }
    // Text mode: both places are named on stderr, and stdout is empty.
    assert!(
        level_text.stderr.contains("reasoning_effort is 'high'")
            && level_text
                .stderr
                .contains("provider_params.reasoning_effort is 'low'"),
        "stderr:\n{}",
        level_text.stderr
    );
    assert!(
        clear_text
            .stderr
            .contains("provider_params.reasoning_effort is 'low'"),
        "stderr:\n{}",
        clear_text.stderr
    );
    for finished in [&level_text, &clear_text] {
        assert!(
            finished.stdout.trim().is_empty(),
            "stdout: {:?}",
            finished.stdout
        );
    }

    assert_eq!(
        provider.request_count(),
        requests_before,
        "no refused resume reached the provider"
    );
    let shown_after = rkat(&roots, &["session", "show", &session]);
    assert_eq!(
        shown_after.stdout, shown_before.stdout,
        "the stored session is unchanged by the refused resumes"
    );
}

/// A word that is not a level is an argument error: exit status 2, the
/// argument parser's text on stderr, and no failure document.
#[test]
fn an_argument_error_prints_no_failure_document() {
    let provider = Loopback::start();
    let roots = roots(provider.port);
    for word in ["extreme", "x_high"] {
        for output in [&["--output", "json"][..], &[][..]] {
            let mut extra = vec!["--reasoning-effort", word];
            extra.extend_from_slice(output);
            let finished = run_turn(&roots, DECLARED_MODEL, &extra);
            assert_eq!(
                finished.status,
                Some(2),
                "'{word}' {output:?}; stderr:\n{}",
                finished.stderr
            );
            assert!(
                finished.stdout.is_empty(),
                "'{word}' {output:?} prints nothing on stdout: {:?}",
                finished.stdout
            );
            assert!(
                finished.stderr.contains("--reasoning-effort") && finished.stderr.contains("xhigh"),
                "the parser names the flag and lists the levels for '{word}':\n{}",
                finished.stderr
            );
        }
    }
    // `--no-reasoning-effort` belongs to `--resume`.
    let finished = run_turn(&roots, DECLARED_MODEL, &["--no-reasoning-effort"]);
    assert_eq!(finished.status, Some(2), "stderr:\n{}", finished.stderr);
    assert!(finished.stdout.is_empty(), "stdout: {:?}", finished.stdout);
    assert!(
        finished.stderr.contains("--resume"),
        "stderr:\n{}",
        finished.stderr
    );
    assert_eq!(provider.request_count(), 0);
}
