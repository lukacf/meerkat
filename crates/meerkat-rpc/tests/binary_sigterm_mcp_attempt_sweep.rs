//! SIGTERM on the real `rkat-rpc` binary with an MCP login completion still
//! in flight.
//!
//! The completion has consumed its OAuth attempt and is held in its
//! credential save: the save's temporary file is a FIFO with no reader, so
//! opening it blocks, with no test hook in the binary. SIGTERM then ends the
//! server: the connection drains its request handlers, aborts the held one at
//! the drain bound, and the runtime shutdown runs the MCP attempt sweep. The
//! sweep finds the consumed attempt absent while aborted work may still run,
//! so it reports it unresolved, never retired. The process stays up while the
//! save is held, lets the released save proceed to write, and then exits with
//! the fatal status for an incomplete shutdown, with its stdin still open
//! throughout. An idle server also ends on SIGTERM with stdin open.
//!
//! Claim limit: the written bytes prove only an attempted write: the released
//! save was still running and reached its write (it was not dropped with the
//! runtime). They do not prove its sync, its rename, any compensation, or the
//! credential coordinator's settlement. The FIFO cannot be synced, so this
//! save fails and its compensating save opens the FIFO again, which the
//! reader also drains until the process exits.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::disallowed_methods
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meerkat::test_fixtures::mcp_oauth::{CODE_CANARY, McpOAuthFixture, SUBJECT};
use serde_json::{Value, json};

const REALM_ID: &str = "rpc-sigterm-mcp-attempt-sweep";
const SERVER_NAME: &str = "attempts";
const REDIRECT_URI: &str = "http://127.0.0.1:1/mcp/oauth/callback";

/// Bound for one RPC answer or one stderr line. Startup under a fully
/// parallel crate run has been observed near 70 s.
const STEP_DEADLINE: Duration = Duration::from_mins(2);

/// The sweep runs after the 10 s request drain bound; its own grace is 10 s.
const SWEEP_DEADLINE: Duration = Duration::from_secs(90);

/// From the reported shutdown result (or the end of the held save) to
/// process exit.
const EXIT_DEADLINE: Duration = Duration::from_secs(20);

/// How long a held save is watched to keep the process alive.
const HELD_SAVE_OBSERVATION: Duration = Duration::from_secs(3);

/// Cargo and nextest advertise the crate's binary at run time.
/// The binary is required: a missing one fails the test instead of passing it
/// vacuously. Its path and SHA-256 are printed, so a retained result names the
/// exact binary that ran.
fn rkat_rpc_binary() -> PathBuf {
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat-rpc")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_rkat-rpc must name the rkat-rpc binary under test");
    assert!(
        binary.is_file(),
        "rkat-rpc binary missing at {}",
        binary.display()
    );
    println!(
        "rkat-rpc binary {} sha256 {}",
        binary.display(),
        sha256_of(&binary)
    );
    binary
}

/// SHA-256 of `path` by the platform's own tool (`sha256sum` on Linux,
/// `shasum -a 256` on macOS).
fn sha256_of(path: &Path) -> String {
    for (tool, args) in [("sha256sum", &[][..]), ("shasum", &["-a", "256"][..])] {
        if let Ok(output) = Command::new(tool).args(args).arg(path).output()
            && output.status.success()
        {
            let line = String::from_utf8_lossy(&output.stdout);
            if let Some(digest) = line.split_whitespace().next() {
                return digest.to_owned();
            }
        }
    }
    panic!("no sha256sum or shasum to hash {}", path.display())
}

/// A running `rkat-rpc` over stdio, with its output collected on threads.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: mpsc::Receiver<Value>,
    /// Answers read while waiting for another id.
    unmatched: Vec<Value>,
    stderr: Arc<Mutex<String>>,
}

impl Server {
    fn spawn(binary: &Path, temp: &Path, project: &Path) -> Self {
        let state_root = temp.join("state");
        let user_root = temp.join("user");
        std::fs::create_dir_all(&user_root).unwrap();
        let mut child = Command::new(binary)
            .current_dir(project)
            .args([
                "--realm",
                REALM_ID,
                "--realm-backend",
                "sqlite",
                "--state-root",
                state_root.to_str().unwrap(),
                "--context-root",
                project.to_str().unwrap(),
                "--user-config-root",
                user_root.to_str().unwrap(),
            ])
            // The file credential store lives under the config directory.
            .env("XDG_CONFIG_HOME", temp.join("xdg"))
            .env("HOME", temp.join("home"))
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("rkat-rpc should spawn");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && value.get("id").is_some()
                    && tx.send(value).is_err()
                {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut pipe = child.stderr.take().unwrap();
        let collected = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                collected
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&chunk[..read]));
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            responses,
            unmatched: Vec::new(),
            stderr,
        }
    }

    fn send(&mut self, id: u64, method: &str, params: Value) {
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
    }

    fn answer(&mut self, id: u64) -> Value {
        let deadline = Instant::now() + STEP_DEADLINE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.responses.recv_timeout(left) {
                Ok(value) if value["id"] == json!(id) => return value,
                Ok(value) => self.unmatched.push(value),
                Err(error) => panic!(
                    "no answer to request {id} ({error}); stderr:\n{}",
                    self.stderr_text()
                ),
            }
        }
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(id, method, params);
        let answer = self.answer(id);
        assert!(
            answer.get("error").is_none(),
            "{method} failed: {answer}; stderr:\n{}",
            self.stderr_text()
        );
        answer["result"].clone()
    }

    fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn wait_for_stderr(&self, needle: &str, deadline: Duration) {
        let until = Instant::now() + deadline;
        while !self.stderr_text().contains(needle) {
            assert!(
                Instant::now() < until,
                "stderr never showed {needle:?}; stderr:\n{}",
                self.stderr_text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Where the binary's default token store keeps credentials under the
/// test's `HOME` and `XDG_CONFIG_HOME`: the platform config directory, which
/// on macOS is `$HOME/Library/Application Support` whatever XDG says.
fn credentials_root(temp: &Path) -> PathBuf {
    let config = if cfg!(target_os = "macos") {
        temp.join("home/Library/Application Support")
    } else {
        temp.join("xdg")
    };
    config.join("meerkat/credentials")
}

/// The single credential file the store wrote for the MCP target.
fn credential_file(credentials: &Path) -> PathBuf {
    let mut found = Vec::new();
    for realm in std::fs::read_dir(credentials).unwrap() {
        let realm = realm.unwrap().path();
        if !realm.is_dir() || realm.ends_with("refresh-locks") {
            continue;
        }
        for entry in std::fs::read_dir(&realm).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                found.push(path);
            }
        }
    }
    assert_eq!(found.len(), 1, "one MCP credential file: {found:?}");
    found.remove(0)
}

fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo").arg(path).status().unwrap();
    assert!(status.success(), "mkfifo {}", path.display());
}

fn wait_exit(child: &mut Child, deadline: Duration) -> Option<ExitStatus> {
    let until = Instant::now() + deadline;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The exit status once the process ends with its stdin still open. A
/// process still running at the deadline fails the test, reporting whether
/// stdin EOF would have ended it.
fn exit_with_stdin_open(server: &mut Server) -> ExitStatus {
    if let Some(status) = wait_exit(&mut server.child, EXIT_DEADLINE) {
        return status;
    }
    drop(server.stdin.take());
    let after_eof = wait_exit(&mut server.child, EXIT_DEADLINE);
    let _ = server.child.kill();
    panic!(
        "rkat-rpc did not exit within {EXIT_DEADLINE:?} with stdin open; after closing \
         stdin: {after_eof:?}; stderr:\n{}",
        server.stderr_text()
    )
}

fn sigterm(child: &Child) {
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
}

/// An idle server ends on SIGTERM while its client keeps stdin open, with
/// the success status.
#[test]
fn sigterm_with_stdin_open_exits_an_idle_server() {
    let binary = rkat_rpc_binary();
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".rkat")).unwrap();
    let mut server = Server::spawn(&binary, temp.path(), &project);
    server.call(1, "initialize", json!({}));
    sigterm(&server.child);
    server.wait_for_stderr("SIGTERM received, shutting down", STEP_DEADLINE);
    let status = exit_with_stdin_open(&mut server);
    assert!(
        status.success(),
        "a complete shutdown exits with success: {status:?}; stderr:\n{}",
        server.stderr_text()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigterm_with_a_held_credential_save_reports_the_attempt_unresolved() {
    let binary = rkat_rpc_binary();
    let fixture = McpOAuthFixture::spawn().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".rkat")).unwrap();
    std::fs::write(
        project.join(".rkat/mcp.toml"),
        format!(
            "[[servers]]\nname = \"{SERVER_NAME}\"\nurl = \"{}\"\noauth_account = \"{SUBJECT}\"\n",
            fixture.mcp_url()
        ),
    )
    .unwrap();
    let mcp = json!({
        "server_name": SERVER_NAME,
        "server_url": fixture.mcp_url(),
        "oauth_account": SUBJECT,
    });
    let start_params = json!({"mcp": mcp, "redirect_uri": REDIRECT_URI});
    let complete_params = |start: &Value| {
        json!({
            "mcp": mcp,
            "code": CODE_CANARY,
            "state": start["state"],
            "redirect_uri": start["redirect_uri"],
        })
    };

    let temp_path = temp.path().to_owned();
    let mut server =
        tokio::task::spawn_blocking(move || Server::spawn(&binary, &temp_path, &project))
            .await
            .unwrap();
    let mut server = tokio::task::block_in_place(|| {
        server.call(1, "initialize", json!({}));
        // A first, unheld login writes the target's credential file, which
        // names the temporary file the next save opens.
        let first = server.call(2, "auth/login/start", start_params.clone());
        server.call(3, "auth/login/complete", complete_params(&first));
        server
    });
    let credential = credential_file(&credentials_root(temp.path()));
    let fifo = credential.with_extension("json.tmp");
    mkfifo(&fifo);

    let (mut server, attempt_ref) = tokio::task::block_in_place(|| {
        let second = server.call(4, "auth/login/start", start_params.clone());
        let state = second["state"].as_str().unwrap().to_owned();
        let attempt_ref =
            meerkat_providers::connector_oauth::OAuthBrowserActionRef::project(&state)
                .as_str()
                .to_owned();
        // In flight: it consumes the attempt, then blocks opening the FIFO.
        server.send(5, "auth/login/complete", complete_params(&second));
        // The attempt is consumed once the status no longer shows it pending;
        // the completion has not answered, so its save is held.
        let until = Instant::now() + STEP_DEADLINE;
        let mut id = 100;
        loop {
            id += 1;
            let status = server.call(id, "auth/status/get", json!({"mcp": mcp}));
            if status.get("attempt").is_none_or(Value::is_null) {
                break;
            }
            assert!(Instant::now() < until, "the attempt was never consumed");
            std::thread::sleep(Duration::from_millis(50));
        }
        (server, attempt_ref)
    });
    sigterm(&server.child);

    let (status, stderr, saved_bytes) = tokio::task::block_in_place(|| {
        server.wait_for_stderr("SIGTERM received, shutting down", STEP_DEADLINE);
        // The drain bound aborts the held completion handler.
        server.wait_for_stderr(
            "inline RPC requests still running when the connection ended; aborting them",
            SWEEP_DEADLINE,
        );
        // The sweep reports the consumed attempt as unresolved.
        server.wait_for_stderr(
            "MCP login attempt absent while aborted request work may still run",
            SWEEP_DEADLINE,
        );
        // The held save still runs on the blocking pool, so the process
        // exits only once that store call ends.
        assert!(
            wait_exit(&mut server.child, HELD_SAVE_OBSERVATION).is_none(),
            "rkat-rpc exited while its credential save was still held; stderr:\n{}",
            server.stderr_text()
        );
        // The reader receives whatever the save writes once it may proceed.
        let fifo = fifo.clone();
        // Drain every write: a FIFO cannot be synced, so the released save
        // fails and its compensating save opens the FIFO again. The reader
        // stops once the process has exited.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_stop = Arc::clone(&stop);
        let reader_fifo = fifo.clone();
        let save_reader = std::thread::spawn(move || {
            let mut total = 0;
            while !reader_stop.load(std::sync::atomic::Ordering::Acquire) {
                let mut written = Vec::new();
                if std::fs::File::open(&reader_fifo)
                    .and_then(|mut f| f.read_to_end(&mut written))
                    .is_err()
                {
                    break;
                }
                total += written.len();
            }
            total
        });
        // SIGTERM ended the server while its stdin is still open, so stdin
        // EOF must not be what lets the process exit.
        let status = exit_with_stdin_open(&mut server);
        // Unblock a reader still waiting in its open. It may enter its open
        // only after a first non-blocking writer found no reader, so retry
        // until the reader has finished (bounded).
        stop.store(true, std::sync::atomic::Ordering::Release);
        let unblock_until = Instant::now() + EXIT_DEADLINE;
        while !save_reader.is_finished() && Instant::now() < unblock_until {
            use std::os::unix::fs::OpenOptionsExt as _;
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(if cfg!(target_os = "linux") {
                    0o4000
                } else {
                    0x4
                })
                .open(&fifo);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            save_reader.is_finished(),
            "the FIFO reader did not stop after the process exited"
        );
        // Let the stderr thread take the last lines.
        std::thread::sleep(Duration::from_millis(200));
        let saved_bytes = save_reader.join().unwrap();
        (status, server.stderr_text(), saved_bytes)
    });

    // The released save attempted its write before the process exited: it was
    // not dropped with the runtime. This proves an attempted write only, not
    // its sync, rename or compensation.
    assert!(
        saved_bytes > 0,
        "the held credential save never wrote after its release; stderr:\n{stderr}"
    );

    assert_eq!(
        status.code(),
        Some(1),
        "an incomplete shutdown exits with the fatal status; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("MCP login attempts were not all retired at shutdown")
            && stderr.contains(&attempt_ref),
        "the shutdown reports the exact attempt unresolved; stderr:\n{stderr}"
    );
    for retired in [
        "retired MCP login attempt at shutdown",
        "MCP login attempt already absent at shutdown",
    ] {
        assert!(
            !stderr.contains(retired),
            "the consumed attempt must not be reported settled ({retired:?}); stderr:\n{stderr}"
        );
    }
    // The aborted completion never answered.
    let answered: Vec<Value> = server
        .unmatched
        .iter()
        .cloned()
        .chain(server.responses.try_iter())
        .collect();
    assert!(
        answered.iter().all(|answer| answer["id"] != json!(5)),
        "the held completion must not have answered: {answered:?}"
    );
}
