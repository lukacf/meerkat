//! `rkat run` against a self-hosted server that repeats the request's
//! `Authorization` header in its error body, as a proxy or gateway can. The
//! static bearer that signed the request must not reach the run's output:
//! the failure text on stderr carries the turn error's message, its typed
//! `detail` and its `reason.provider_error.message`, each of which held the
//! bearer before the provider clients redacted what they read from a
//! response. One row answers 400 (the turn fails at once), the other 500
//! (the failure text is assembled after the retries are used up).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::panic
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const REALM: &str = "credential-echo";
const TOKEN_VARIABLE: &str = "RUN_CREDENTIAL_ECHO_LOOPBACK_TOKEN";

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

/// A made-up bearer, assembled here so no scanner reads the source as a
/// secret.
fn credential() -> String {
    format!("loopback-{}", "madeupcredential".repeat(3))
}

struct EchoServer {
    port: u16,
    requests: Arc<AtomicUsize>,
}

/// A loopback HTTP server that answers every request with `status` and a
/// JSON error whose message repeats the request's `authorization` header
/// exactly as received.
fn spawn_echo_server(status: &'static str) -> EchoServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    let port = listener.local_addr().expect("local addr").port();
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || answer(stream, status, &seen));
        }
    });
    EchoServer { port, requests }
}

fn answer(mut stream: TcpStream, status: &str, seen: &AtomicUsize) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            }
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let header = |name: &str| {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    };
    let body_len = header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while request.len() < head_end + body_len {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    seen.fetch_add(1, Ordering::SeqCst);
    let authorization = header("authorization").unwrap_or_default();
    let body = serde_json::json!({
        "error": {
            "message": format!("rejected request with header authorization: {authorization}"),
            "type": "invalid_request_error",
        }
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Run `rkat run` in a private home against a self-hosted route on `port`
/// whose static bearer is read from the child's environment.
fn run_against(port: u16) -> Output {
    let temp = tempfile::TempDir::new().unwrap();
    let home = temp.path().to_path_buf();
    let project = temp.path().join("project");
    let state = temp.path().join("state");
    let user = temp.path().join("user");
    let realm = state.join(REALM);
    for dir in [&project, &user, &project.join(".rkat"), &realm] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        realm.join("config.toml"),
        format!(
            r#"[retry]
max_retries = 1
initial_delay = "10ms"
max_delay = "20ms"

[self_hosted.servers.loopback]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."loopback-model"]
server = "loopback"
remote_model = "loopback-model"

[realm."{REALM}"]
default_binding = "loopback"

[realm."{REALM}".backend.loopback]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "loopback"

[realm."{REALM}".auth.loopback]
provider = "self_hosted"
auth_method = "static_bearer"
source = {{ kind = "env", env = "{TOKEN_VARIABLE}" }}

[realm."{REALM}".binding.loopback]
backend_profile = "loopback"
auth_profile = "loopback"
"#
        ),
    )
    .unwrap();
    let mut command = Command::new(rkat_binary());
    command
        .current_dir(&project)
        .env("HOME", &home)
        .env(TOKEN_VARIABLE, credential())
        .env_remove("RUST_LOG");
    for variable in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "RKAT_ANTHROPIC_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(variable);
    }
    command
        .arg("--realm")
        .arg(REALM)
        .arg("--context-root")
        .arg(&project)
        .arg("--state-root")
        .arg(&state)
        .arg("--user-config-root")
        .arg(&user)
        .args([
            "run",
            "-m",
            "loopback-model",
            "--provider",
            "self-hosted",
            "Say ok.",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.output().expect("run rkat")
}

/// The run failed with the server's answer in its failure text, and the
/// bearer that signed the request is in neither output stream.
fn assert_failure_without_the_credential(output: &Output, row: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{row}: the run fails; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("rejected request"),
        "{row}: the server's answer reaches the failure text; stderr:\n{stderr}"
    );
    let credential = credential();
    assert!(
        !stderr.contains(&credential),
        "{row}: stderr carries the credential:\n{stderr}"
    );
    assert!(
        !stdout.contains(&credential),
        "{row}: stdout carries the credential:\n{stdout}"
    );
}

/// A 400 whose body repeats the request's `Authorization` header: the turn
/// fails at once, and its failure text (the message, the typed `detail` and
/// `reason.provider_error.message`) does not carry the bearer.
#[test]
fn a_credential_echoed_in_a_rejection_does_not_reach_the_run_output() {
    let server = spawn_echo_server("400 Bad Request");
    let output = run_against(server.port);
    assert_failure_without_the_credential(&output, "rejection");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"provider_error\":{\"message\":"),
        "the typed reason carries the provider message; stderr:\n{stderr}"
    );
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        1,
        "a rejection is not retried"
    );
}

/// A 500 whose body repeats the header is retried; the failure text the run
/// reports once the retries are used up does not carry the bearer either.
#[test]
fn a_credential_echoed_in_a_retried_failure_does_not_reach_the_run_output() {
    let server = spawn_echo_server("500 Internal Server Error");
    let output = run_against(server.port);
    assert_failure_without_the_credential(&output, "retried failure");
    assert!(
        server.requests.load(Ordering::SeqCst) > 1,
        "the failure was retried: {} request(s)",
        server.requests.load(Ordering::SeqCst)
    );
}
