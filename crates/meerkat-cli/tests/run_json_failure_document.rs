//! The failure document of `rkat run --output json`.
//!
//! A failed run in JSON mode writes `{"error": {code, category, message,
//! details}}` (the `WireError` envelope) as the last document on stdout, so a
//! caller reads the failure's public code and, for a failed turn, the typed
//! turn error, without parsing stderr. Exit statuses and the stderr text are
//! what they were before the document existed; text and HTML modes print no
//! document. Each row runs the built binary against a loopback
//! OpenAI-compatible server wired in by the realm's own config file.

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
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_mins(2);
const REALM: &str = "json-failure";
const MODEL: &str = "loopback-model";
/// The variable a static-bearer route of these rows reads its token from.
const TOKEN_ENV: &str = "RUN_JSON_FAILURE_LOOPBACK_TOKEN";

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

/// What the loopback server answers.
#[derive(Clone)]
enum Script {
    /// Every request gets this status and body.
    Status { status: u16, body: String },
    /// Every request gets this status; the body repeats the request's
    /// `authorization` header, as a proxy or gateway can.
    EchoAuthorization { status: u16 },
    /// The first request streams one text delta and a tool call; every later
    /// request gets this status and body.
    DeltaThenStatus { status: u16, body: String },
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn sse_chunk(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn streamed_delta_and_tool_call() -> String {
    let mut body = String::new();
    body.push_str(&sse_chunk(serde_json::json!({
        "choices": [{ "delta": { "content": STREAMED_TEXT }, "finish_reason": null }]
    })));
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
    body.push_str(&sse_chunk(serde_json::json!({
        "choices": [],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
    })));
    body.push_str("data: [DONE]\n\n");
    body
}

const STREAMED_TEXT: &str = "streamed-before-the-failure";

/// Read one whole HTTP request (headers and body); returns its header block.
fn read_request(socket: &mut TcpStream) -> Option<String> {
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
    let raw_headers = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let content_length = raw_headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    Some(raw_headers)
}

fn serve(mut socket: TcpStream, script: &Script, request_index: usize) {
    let _ = socket.set_read_timeout(Some(Duration::from_secs(30)));
    let Some(headers) = read_request(&mut socket) else {
        return;
    };
    let (status, content_type, body) = match script {
        Script::Status { status, body } => (*status, "application/json", body.clone()),
        Script::EchoAuthorization { status } => {
            let authorization = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_string())
                })
                .unwrap_or_default();
            let body = serde_json::json!({
                "error": {
                    "message": format!("rejected request with header authorization: {authorization}"),
                    "type": "invalid_request_error"
                }
            })
            .to_string();
            (*status, "application/json", body)
        }
        Script::DeltaThenStatus { status, body } => {
            if request_index == 0 {
                (200, "text/event-stream", streamed_delta_and_tool_call())
            } else {
                (*status, "application/json", body.clone())
            }
        }
    };
    let phrase = if status == 200 {
        "OK"
    } else {
        reason_phrase(status)
    };
    let response = format!(
        "HTTP/1.1 {status} {phrase}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes());
    let _ = socket.shutdown(std::net::Shutdown::Both);
}

/// Start the loopback server on its own thread; returns its port. The thread
/// ends with the test process.
fn start_loopback(script: Script) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    let port = listener.local_addr().expect("loopback address").port();
    std::thread::spawn(move || {
        for (request_index, socket) in listener.incoming().enumerate() {
            let Ok(socket) = socket else {
                return;
            };
            serve(socket, &script, request_index);
        }
    });
    port
}

struct Roots {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
    user: PathBuf,
    /// The bearer token of a static-bearer route, passed to `rkat` in
    /// `TOKEN_ENV`.
    token: Option<String>,
}

/// Fresh roots whose realm config routes `MODEL` to the loopback server
/// without a credential.
fn roots(port: u16) -> Roots {
    roots_with_token(port, None)
}

/// Fresh roots whose realm config routes `MODEL` to the loopback server;
/// with a token, the route signs its requests with it as a static bearer.
fn roots_with_token(port: u16, token: Option<String>) -> Roots {
    let auth = match token {
        Some(_) => format!(
            "auth_method = \"static_bearer\"\nsource = {{ kind = \"env\", env = \"{TOKEN_ENV}\" }}"
        ),
        None => "auth_method = \"none\"\nsource = { kind = \"platform_default\" }".to_string(),
    };
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
            r#"[self_hosted.servers.loopback]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{MODEL}"]
server = "loopback"
remote_model = "{MODEL}"

[realm."{REALM}"]
default_binding = "loopback"

[realm."{REALM}".backend.loopback]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "loopback"

[realm."{REALM}".auth.loopback]
provider = "self_hosted"
{auth}

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
        token,
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

/// Run `rkat <root flags> <args>` in the loopback realm.
fn rkat(roots: &Roots, args: &[&str]) -> Finished {
    rkat_in_realm(roots, REALM, args)
}

/// Run `rkat --realm <realm> <root flags> <args>` with no provider key in
/// its environment and stdin closed; panics at the deadline.
fn rkat_in_realm(roots: &Roots, realm: &str, args: &[&str]) -> Finished {
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
        TOKEN_ENV,
    ] {
        command.env_remove(name);
    }
    if let Some(token) = &roots.token {
        command.env(TOKEN_ENV, token);
    }
    command
        .current_dir(&roots.project)
        .env("HOME", &roots.home)
        .env("XDG_CONFIG_HOME", roots.home.join("config"))
        .arg("--realm")
        .arg(realm)
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

/// `rkat run` against the loopback route, with `extra` run flags.
fn run_turn(roots: &Roots, extra: &[&str]) -> Finished {
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["-m", MODEL, "--provider", "self-hosted", "Say ok."]);
    rkat(roots, &args)
}

/// The last JSON document on stdout: the whole of stdout when it is one
/// document, otherwise the pretty-printed object that follows streamed text.
fn last_document(finished: &Finished) -> serde_json::Value {
    let stdout = finished.stdout.trim_end();
    if let Ok(document) = serde_json::from_str::<serde_json::Value>(stdout.trim_start()) {
        return document;
    }
    let start = stdout
        .rfind("\n{\n")
        .map(|index| index + 1)
        .unwrap_or_else(|| {
            panic!(
                "stdout ends with no JSON document; stdout:\n{}\nstderr:\n{}",
                finished.stdout, finished.stderr
            )
        });
    serde_json::from_str(&stdout[start..]).unwrap_or_else(|error| {
        panic!(
            "the last stdout document is not JSON ({error}); stdout:\n{}",
            finished.stdout
        )
    })
}

/// The `Error: ` line stderr carries for a failed command.
fn stderr_error_line(finished: &Finished) -> &str {
    finished
        .stderr
        .lines()
        .find_map(|line| line.strip_prefix("Error: "))
        .unwrap_or_else(|| panic!("stderr has no `Error: ` line:\n{}", finished.stderr))
}

fn unavailable() -> Script {
    Script::Status {
        status: 503,
        body: r#"{"error":{"message":"overloaded","type":"server_error"}}"#.to_string(),
    }
}

fn bad_request() -> Script {
    Script::Status {
        status: 400,
        body: r#"{"error":{"message":"unsupported value","type":"invalid_request_error"}}"#
            .to_string(),
    }
}

/// An exact session id no realm of these rows holds.
const MISSING_SESSION: &str = "018f6f2e-7c1a-7b3e-9d2a-5a4c3b2a1f00";
/// A realm id the realm scope resolution refuses.
const UNRESOLVABLE_REALM: &str = "not a realm!";

/// The document of a failure with no typed owner: `INTERNAL_ERROR`, no
/// details, the stderr text as its message.
fn assert_untyped_failure_document(finished: &Finished) {
    let document = last_document(finished);
    assert_eq!(document["error"]["code"], "INTERNAL_ERROR", "{document}");
    assert_eq!(document["error"]["category"], "internal", "{document}");
    assert!(document["error"].get("details").is_none(), "{document}");
    assert_eq!(
        document["error"]["message"],
        stderr_error_line(finished),
        "{document}"
    );
}

/// A turn that fails on a transient provider answer (503, past the retries)
/// says so in typed fields: the provider error code and the
/// `retry_exhausted` kind. The exit status stays 1.
#[test]
fn cli_failed_turn_document_carries_typed_retryability() {
    let roots = roots(start_loopback(unavailable()));
    let finished = run_turn(&roots, &["--output", "json"]);
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    let document = last_document(&finished);
    assert_eq!(document["error"]["code"], "PROVIDER_ERROR", "{document}");
    assert_eq!(document["error"]["category"], "provider", "{document}");
    assert_eq!(
        document["error"]["details"]["error"]["kind"], "retry_exhausted",
        "{document}"
    );
}

/// The typed turn error of a retry-exhausted turn as it is today: its kind,
/// that it is terminal, its outcome and a diagnostic, and no `retryable`,
/// `provider` or `reason`. A turn error that gains those fields changes this
/// row on purpose.
#[test]
fn a_retry_exhausted_turn_error_has_its_current_fields() {
    let roots = roots(start_loopback(unavailable()));
    let finished = run_turn(&roots, &["--output", "json"]);
    let document = last_document(&finished);
    let turn_error = document["error"]["details"]["error"]
        .as_object()
        .unwrap_or_else(|| panic!("no typed turn error: {document}"));
    let mut fields: Vec<&str> = turn_error.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["detail", "kind", "outcome", "terminal"],
        "{document}"
    );
    assert_eq!(turn_error["kind"], "retry_exhausted", "{document}");
    assert_eq!(turn_error["terminal"], true, "{document}");
    assert_eq!(turn_error["outcome"], "failed", "{document}");
}

/// A turn that fails on a permanent provider answer (400) is not retryable.
#[test]
fn a_permanent_provider_failure_is_not_retryable() {
    let roots = roots(start_loopback(bad_request()));
    let finished = run_turn(&roots, &["--json"]);
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    let document = last_document(&finished);
    assert_eq!(document["error"]["code"], "PROVIDER_ERROR", "{document}");
    let turn_error = &document["error"]["details"]["error"];
    assert_eq!(turn_error["kind"], "llm_failure", "{document}");
    assert_ne!(turn_error["retryable"], true, "{document}");
}

/// The document's message is the text of stderr's `Error:` line, and the
/// typed turn error that line embeds is the document's `details.error`.
#[test]
fn the_document_message_is_the_stderr_text() {
    let roots = roots(start_loopback(bad_request()));
    let finished = run_turn(&roots, &["--output", "json"]);
    let document = last_document(&finished);
    let line = stderr_error_line(&finished);
    assert_eq!(document["error"]["message"], line, "{document}");
    let (text, embedded) = line
        .split_once("; error=")
        .unwrap_or_else(|| panic!("stderr line has no typed error: {line}"));
    assert!(text.starts_with("turn abandoned: "), "{line}");
    let embedded: serde_json::Value = serde_json::from_str(embedded).expect("typed error JSON");
    assert_eq!(document["error"]["details"]["error"], embedded);
}

/// Text and HTML modes print no failure document; stderr has the `Error:`
/// line and the exit status is 1.
#[test]
fn text_and_html_modes_print_no_failure_document() {
    let roots = roots(start_loopback(bad_request()));
    for mode in [
        &["--output", "text"][..],
        &["--output", "html"][..],
        &[][..],
    ] {
        let finished = run_turn(&roots, mode);
        assert_eq!(finished.status, Some(1), "{mode:?}: {}", finished.stderr);
        assert!(
            !finished.stdout.contains("\"error\""),
            "{mode:?} printed a failure document: {}",
            finished.stdout
        );
        assert!(
            stderr_error_line(&finished).starts_with("turn abandoned: "),
            "{mode:?}: {}",
            finished.stderr
        );
    }
}

/// With `--stream`, streamed text precedes the document, which is the last
/// JSON object on stdout.
#[test]
fn streamed_text_precedes_the_failure_document() {
    let script = Script::DeltaThenStatus {
        status: 400,
        body: r#"{"error":{"message":"unsupported value","type":"invalid_request_error"}}"#
            .to_string(),
    };
    let roots = roots(start_loopback(script));
    let finished = run_turn(&roots, &["--output", "json", "--stream"]);
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    let text_at = finished
        .stdout
        .find(STREAMED_TEXT)
        .unwrap_or_else(|| panic!("streamed text missing from stdout: {}", finished.stdout));
    let document_at = finished
        .stdout
        .rfind("\n{\n")
        .unwrap_or_else(|| panic!("no document after streamed text: {}", finished.stdout));
    assert!(text_at < document_at, "{}", finished.stdout);
    let document = last_document(&finished);
    assert_eq!(document["error"]["code"], "PROVIDER_ERROR", "{document}");
    assert!(
        finished.stdout.trim_end().ends_with('}'),
        "the document is the last thing on stdout: {}",
        finished.stdout
    );
}

/// A provider (or a proxy in front of it) that repeats the request's
/// credential in its error body: the credential reaches neither the failure
/// document nor stderr. The route's authorizer removes it from the provider
/// text before the error leaves the client; the document adds no redaction
/// of its own, and the rest of the provider's answer still reaches the
/// typed error.
#[test]
fn a_credential_echoed_by_the_provider_does_not_reach_stdout() {
    // Credential-shaped, assembled here so no scanner reads it as a secret.
    let token = format!("sk-{}", "loopbackcredential".repeat(3));
    let roots = roots_with_token(
        start_loopback(Script::EchoAuthorization { status: 400 }),
        Some(token.clone()),
    );
    let finished = run_turn(&roots, &["--output", "json"]);
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    let document = last_document(&finished);
    assert_eq!(document["error"]["code"], "PROVIDER_ERROR", "{document}");
    let detail = document["error"]["details"]["error"]["detail"]
        .as_str()
        .unwrap_or_default();
    assert!(
        detail.contains("rejected request with header authorization: Bearer [REDACTED]"),
        "the provider's answer reaches the typed error without the credential: {document}"
    );
    assert!(
        !finished.stdout.contains(&token),
        "the credential reached stdout: {}",
        finished.stdout
    );
    assert!(
        !finished.stderr.contains(&token),
        "the credential reached stderr: {}",
        finished.stderr
    );
}

/// A typed failure before any turn (`--resume` of an exact session id the
/// realm does not hold) prints its own code and keeps its exit status.
#[test]
fn a_resume_of_an_unknown_session_prints_its_code() {
    let roots = roots(start_loopback(bad_request()));
    let finished = rkat(
        &roots,
        &[
            "run",
            "--resume",
            MISSING_SESSION,
            "--output",
            "json",
            "keep going",
        ],
    );
    assert_eq!(finished.status, Some(10), "stderr:\n{}", finished.stderr);
    let document = last_document(&finished);
    assert_eq!(document["error"]["code"], "SESSION_NOT_FOUND", "{document}");
    assert_eq!(document["error"]["category"], "session", "{document}");
    assert_eq!(
        document["error"]["message"],
        stderr_error_line(&finished),
        "{document}"
    );
}

/// A check the CLI makes itself has no typed owner: `INTERNAL_ERROR`, no
/// details, exit status 1.
#[test]
fn a_cli_check_failure_prints_internal_error() {
    let roots = roots(start_loopback(bad_request()));
    let finished = rkat(
        &roots,
        &[
            "run",
            "--json",
            "--no-comms",
            "--comms-listen-tcp",
            "127.0.0.1:0",
            "hello",
        ],
    );
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    assert_untyped_failure_document(&finished);
}

/// An `--auth-binding` value that does not parse is refused before the run
/// command starts; the document is printed there too.
#[test]
fn an_unparsable_auth_binding_prints_internal_error() {
    let roots = roots(start_loopback(bad_request()));
    let finished = rkat(
        &roots,
        &["run", "--json", "--auth-binding", "not a binding", "hello"],
    );
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    assert_untyped_failure_document(&finished);
}

/// A realm scope that does not resolve is refused before any command
/// starts; a JSON run prints the document there too.
#[test]
fn an_unresolvable_realm_scope_prints_internal_error() {
    let roots = roots(start_loopback(bad_request()));
    let finished = rkat_in_realm(&roots, UNRESOLVABLE_REALM, &["run", "--json", "hello"]);
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    assert_untyped_failure_document(&finished);
}

/// A command other than `rkat run` prints no failure document, whatever its
/// own JSON flag: `rkat help --output json` shares the run path and stays
/// without one.
#[cfg(feature = "skills")]
#[test]
fn rkat_help_prints_no_failure_document() {
    let roots = roots(start_loopback(bad_request()));
    let finished = rkat(
        &roots,
        &[
            "help",
            "--output",
            "json",
            "-m",
            MODEL,
            "--provider",
            "self-hosted",
            "how do I resume a session?",
        ],
    );
    assert_eq!(finished.status, Some(1), "stderr:\n{}", finished.stderr);
    assert!(
        finished.stdout.trim().is_empty(),
        "rkat help printed to stdout: {}",
        finished.stdout
    );
}

/// The exit statuses and stderr lines of failed runs, the same in JSON and
/// text mode: a failed turn 1, an unknown exact session id 10, a check made
/// by the CLI 1, an unparsable `--auth-binding` 1, an unresolvable realm
/// scope 1, and an argument the parser refuses 2 with nothing on stdout.
#[test]
fn failed_runs_keep_their_exit_statuses() {
    let roots = roots(start_loopback(bad_request()));
    let mut stderr_lines: Vec<Vec<String>> = Vec::new();
    for mode in [&["--output", "json"][..], &["--output", "text"][..]] {
        let mut lines = Vec::new();

        let turn = run_turn(&roots, mode);
        assert_eq!(turn.status, Some(1), "{mode:?}: {}", turn.stderr);
        assert!(
            stderr_error_line(&turn).starts_with("turn abandoned: "),
            "{mode:?}: {}",
            turn.stderr
        );
        lines.push(stderr_error_line(&turn).to_string());

        let mut resume = vec!["run", "--resume", MISSING_SESSION];
        resume.extend_from_slice(mode);
        resume.push("keep going");
        let resume = rkat(&roots, &resume);
        assert_eq!(resume.status, Some(10), "{mode:?}: {}", resume.stderr);
        assert_eq!(
            stderr_error_line(&resume),
            format!("Session not found: {MISSING_SESSION}"),
            "{mode:?}"
        );

        let mut check = vec!["run", "--no-comms", "--comms-listen-tcp", "127.0.0.1:0"];
        check.extend_from_slice(mode);
        check.push("hello");
        let check = rkat(&roots, &check);
        assert_eq!(check.status, Some(1), "{mode:?}: {}", check.stderr);
        assert_eq!(
            stderr_error_line(&check),
            "--comms-listen-tcp cannot be used with --no-comms",
            "{mode:?}"
        );

        let mut binding = vec!["run", "--auth-binding", "not a binding"];
        binding.extend_from_slice(mode);
        binding.push("hello");
        let binding = rkat(&roots, &binding);
        assert_eq!(binding.status, Some(1), "{mode:?}: {}", binding.stderr);
        // The whole of stderr: this failure is reported by the process
        // exit, not by the command's failure report.
        assert_eq!(
            binding.stderr,
            "Error: --auth-binding requires `realm:binding[:profile]`; got `not a binding` \
             with no `:` separator\n",
            "{mode:?}"
        );
        lines.push(binding.stderr.clone());

        let mut scope = vec!["run"];
        scope.extend_from_slice(mode);
        scope.push("hello");
        let scope = rkat_in_realm(&roots, UNRESOLVABLE_REALM, &scope);
        assert_eq!(scope.status, Some(1), "{mode:?}: {}", scope.stderr);
        assert!(
            scope
                .stderr
                .starts_with("Error: invalid explicit realm id: not a realm!; "),
            "{mode:?}: {}",
            scope.stderr
        );
        lines.push(scope.stderr.clone());

        let mut usage = vec!["run", "--no-such-flag"];
        usage.extend_from_slice(mode);
        usage.push("hello");
        let usage = rkat(&roots, &usage);
        assert_eq!(usage.status, Some(2), "{mode:?}: {}", usage.stderr);
        assert!(usage.stdout.is_empty(), "{mode:?}: {}", usage.stdout);

        stderr_lines.push(lines);
    }
    // Each failure prints the same stderr text in both modes.
    assert_eq!(
        stderr_lines[0], stderr_lines[1],
        "JSON mode changes no stderr text"
    );
    for stderr in &stderr_lines[0][1..] {
        assert!(stderr.starts_with("Error: "), "{stderr}");
        assert_eq!(stderr.lines().count(), 1, "one stderr line: {stderr}");
    }
}
