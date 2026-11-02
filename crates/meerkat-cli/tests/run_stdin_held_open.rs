//! `rkat run` with a stdin that is a pipe held open and silent, as some
//! launchers, supervisors and agent harnesses pass. Before the fix the default
//! `--stdin auto` read it to end of file before anything else and the run hung
//! with no output; each row here holds the pipe open for longer than its
//! deadline.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::panic
)]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);
const REALM: &str = "stdin-held-open";

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

struct Roots {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
    user: PathBuf,
}

fn roots() -> Roots {
    let temp = tempfile::TempDir::new().unwrap();
    let home = temp.path().to_path_buf();
    let project = temp.path().join("project");
    let state = temp.path().join("state");
    let user = temp.path().join("user");
    for dir in [&project, &state, &user, &project.join(".rkat")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    Roots {
        _temp: temp,
        home,
        project,
        state,
        user,
    }
}

/// A stdio MCP server that never answers the handshake (`/bin/sleep` with a
/// marker argument unique to this test process), listed in the realm's own
/// config document, which is trusted on every platform, unlike a project
/// `.rkat/mcp.toml`.
fn write_silent_mcp_server(roots: &Roots) {
    let realm = roots.state.join(REALM);
    std::fs::create_dir_all(&realm).unwrap();
    std::fs::write(
        realm.join("config.toml"),
        format!(
            "[[tools.mcp_servers]]\nname = \"silent\"\ncommand = \"/bin/sleep\"\nargs = [\"{}\"]\nenv = {{}}\nconnect_timeout_secs = 2\naccess = {{ allowed_operations = [\"noop\"] }}\n",
            silent_server_marker()
        ),
    )
    .unwrap();
}

/// The silent server's sleep argument: 600 s plus a fraction unique to this
/// test process, so the test can find and stop exactly its own server.
fn silent_server_marker() -> String {
    format!("600.{}", std::process::id())
}

/// Stop the silent server if it is still running: `rkat` stops it when the
/// run ends, but a killed `rkat` leaves it behind. Best effort.
fn stop_silent_server() {
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(format!("^[^ ]*/sleep 600\\.{}$", std::process::id()))
        .status();
}

/// Run `rkat` with `extra` root flags and `run_args`, its stdin a pipe this
/// test keeps open (never written, never closed) until the run ends or the
/// deadline passes. Returns the output, or panics at the deadline.
fn run_with_held_open_stdin(roots: &Roots, extra: &[&str], run_args: &[&str]) -> Output {
    let mut command = Command::new(rkat_binary());
    command
        .current_dir(&roots.project)
        .env("HOME", &roots.home)
        .env("RKAT_TEST_CLIENT", "1")
        .env_remove("RUST_LOG")
        .arg("--realm")
        .arg(REALM)
        .arg("--context-root")
        .arg(&roots.project)
        .arg("--state-root")
        .arg(&roots.state)
        .arg("--user-config-root")
        .arg(&roots.user)
        .args(extra)
        .arg("run")
        .args(run_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child: Child = command.spawn().expect("spawn rkat");
    // Hold the write end: the pipe stays open and silent for the whole row.
    let held_stdin = child.stdin.take().expect("piped stdin");
    // Drain both streams on their own threads, so a full pipe never stalls
    // rkat and a descendant still holding a stream never stalls the test.
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
            stop_silent_server();
            panic!(
                "rkat run did not finish within {DEADLINE:?} with stdin held open; stdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&collect(stdout)),
                String::from_utf8_lossy(&collect(stderr))
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    drop(held_stdin);
    Output {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    }
}

fn drain(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = send.send(bytes);
    });
    receive
}

/// What a drained stream produced, waiting a few seconds at most for its end.
fn collect(stream: mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    stream
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| b"<stream still held open by a descendant>".to_vec())
}

/// R1: the default `--stdin auto` gives up on a silent stdin after its grace
/// period, says so once, and runs the prompt.
#[test]
fn r1_auto_continues_without_a_held_open_silent_stdin() {
    let roots = roots();
    let output = run_with_held_open_stdin(&roots, &[], &["Reply with OK."]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={} stderr:\n{stderr}",
        output.status
    );
    assert_eq!(
        stderr.matches("no input arrived within").count(),
        1,
        "one warning naming the flags; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("--stdin blob") && stderr.contains("--stdin off"),
        "stderr:\n{stderr}"
    );
}

/// R2 (control): `--stdin off` never reads stdin.
#[test]
fn r2_stdin_off_ignores_a_held_open_stdin() {
    let roots = roots();
    let output = run_with_held_open_stdin(&roots, &[], &["--stdin", "off", "Reply with OK."]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={} stderr:\n{stderr}",
        output.status
    );
    assert!(
        !stderr.contains("no input arrived within"),
        "stderr:\n{stderr}"
    );
}

/// R3: the same under workspace confinement beside a stdio MCP server that
/// never answers, which `--wait-for-mcp` makes the run wait for, up to its
/// 2 s connect timeout. The run still finishes and the confinement report
/// prints. Where the host enforces confinement, the server was launched and
/// stayed silent (the timeout, not an early exit, ends the wait); where it
/// cannot, the report says so and the launch is refused.
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn r3_auto_under_workspace_confinement_with_a_silent_stdio_server() {
    let roots = roots();
    write_silent_mcp_server(&roots);
    let output = run_with_held_open_stdin(
        &roots,
        &["--confinement", "workspace"],
        &["--wait-for-mcp", "Reply with OK."],
    );
    stop_silent_server();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={} stderr:\n{stderr}",
        output.status
    );
    // A host that cannot enforce workspace confinement (for example one that
    // refuses unprivileged user namespaces) refuses the confined launch; the
    // run must still finish there, so only the launch check depends on it.
    if stderr.contains("mcp_stdio confinement: requested workspace, enforced by") {
        assert!(
            stderr.contains("Timed out connecting to 'silent'"),
            "the silent stdio server was launched and never answered; stderr:\n{stderr}"
        );
    } else {
        assert!(
            stderr.contains("cannot be enforced on this host"),
            "the confinement report states why the launch is refused; stderr:\n{stderr}"
        );
    }
    assert!(
        stderr.contains("mcp_stdio confinement: requested workspace"),
        "the confinement report prints; stderr:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("no input arrived within").count(),
        1,
        "stderr:\n{stderr}"
    );
}
