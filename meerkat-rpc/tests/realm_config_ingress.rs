//! Head realm config ingress on the rkat-rpc surface.
//!
//! The head realm document is authoritative. `rkat-rpc` must refuse to start
//! when the head `.rkat/config.toml` fails its ingress checks (here the typed
//! refusal of an unwired `[agent] provider_params` table) instead of replacing
//! the whole head document with `Config::default()` and serving on a
//! configuration the operator never wrote. The observable is the real binary:
//! a non-zero exit before the server loop and the `ConfigError::Validation`
//! text on stderr. The failure mode has its own positive observable: a server
//! that booted on defaults answers the `initialize` request written to its
//! stdin, and that answer fails the test immediately.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The refused table, in the shape an operator hunting for a fleet-wide
/// Anthropic cache policy would write into `.rkat/config.toml`.
const HEAD_WITH_AGENT_PROVIDER_PARAMS: &str = "[agent]\n\
max_tokens_per_turn = 256\n\
provider_params = { provider_tag = { provider = \"anthropic\", cache_control = \"disabled\" } }\n";

/// Substring of `Config::reject_unwired_agent_provider_params`'s
/// `ConfigError::Validation` payload.
const REFUSAL_TEXT: &str = "[agent] provider_params is not applied to any session";

const REALM_ID: &str = "rpc-realm-config-ingress-refused";

/// A server that booted answers this; a server that refused its head config
/// never reads it.
const INITIALIZE_REQUEST: &str =
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";

/// Last-resort bound for a server that neither refuses nor answers. Chosen
/// well above the observed startup latency under a fully parallel crate run
/// (about 70 s) and below the CI unit profile's 240 s termination ceiling.
const EXIT_DEADLINE: Duration = Duration::from_secs(200);

/// Cargo advertises the crate's own binary to its integration tests at
/// compile time; a runtime override is honoured for staged binaries.
fn rkat_rpc_binary() -> Option<PathBuf> {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rpc")
        .map(PathBuf::from)
        .or_else(|| option_env!("CARGO_BIN_EXE_rkat-rpc").map(PathBuf::from))?;
    advertised.exists().then_some(advertised)
}

struct Exit {
    status: ExitStatus,
    stderr: String,
}

/// Run `rkat-rpc` against `state_root`, hand it one `initialize` request on
/// stdin, and wait for it to exit.
///
/// A refused head config exits before the server loop and never reads stdin.
/// A server that booted on defaults answers the request on stdout, which is
/// reported as the failure it is instead of waiting for a shutdown that may
/// never come; the deadline only guards a process that does neither.
fn run_rkat_rpc(binary: &Path, project: &Path, state_root: &Path, user_root: &Path) -> Exit {
    let mut child = Command::new(binary)
        .current_dir(project)
        .args([
            "--realm",
            REALM_ID,
            "--realm-backend",
            "memory",
            "--state-root",
            state_root.to_str().expect("utf-8 state root"),
            "--context-root",
            project.to_str().expect("utf-8 project root"),
            "--user-config-root",
            user_root.to_str().expect("utf-8 user root"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rkat-rpc should spawn");

    // A refusal may already have closed the pipe; that write failure is the
    // expected shape, not a test defect. Dropping stdin afterwards hands a
    // booted server EOF so it can shut down on its own once observed.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(INITIALIZE_REQUEST.as_bytes());
        let _ = stdin.flush();
    }

    let mut stderr_pipe = child.stderr.take().expect("stderr pipe");
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = String::new();
        stderr_pipe
            .read_to_string(&mut buffer)
            .expect("stderr should be readable");
        buffer
    });
    let stdout_pipe = child.stdout.take().expect("stdout pipe");
    let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
    let stdout_reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout_pipe).lines() {
            let Ok(line) = line else { break };
            if stdout_tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + EXIT_DEADLINE;
    let status = loop {
        if let Ok(line) = stdout_rx.try_recv()
            && line.contains("\"jsonrpc\"")
        {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "rkat-rpc booted and answered `initialize` instead of refusing the head realm \
                 config carrying [agent] provider_params; first stdout line: {line}"
            );
        }
        if let Some(status) = child.try_wait().expect("try_wait should succeed") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stderr = stderr_reader
                .join()
                .expect("stderr reader thread should finish");
            panic!(
                "rkat-rpc neither refused its head config nor answered `initialize` within \
                 {EXIT_DEADLINE:?}. stderr:\n{stderr}"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stderr = stderr_reader
        .join()
        .expect("stderr reader thread should finish");
    stdout_reader
        .join()
        .expect("stdout reader thread should finish");
    Exit { status, stderr }
}

#[test]
fn head_config_with_agent_provider_params_refuses_rkat_rpc_startup() {
    let Some(binary) = rkat_rpc_binary() else {
        eprintln!(
            "skipping: rkat-rpc binary not advertised (CARGO_BIN_EXE_rkat-rpc unset or missing)"
        );
        return;
    };

    let temp = TempDir::new().expect("temp dir");
    let project = temp.path().join("project");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(project.join(".rkat")).expect("project root should initialize");
    std::fs::create_dir_all(&user_root).expect("user root should initialize");
    let paths = meerkat_store::realm_paths_in(&state_root, REALM_ID);
    std::fs::create_dir_all(&paths.root).expect("realm dir should initialize");
    std::fs::write(&paths.config_path, HEAD_WITH_AGENT_PROVIDER_PARAMS)
        .expect("head config should be written");

    let exit = run_rkat_rpc(&binary, &project, &state_root, &user_root);

    assert!(
        !exit.status.success(),
        "rkat-rpc must refuse a head realm config carrying [agent] provider_params instead \
         of booting on Config::default(); exit={:?} stderr:\n{}",
        exit.status,
        exit.stderr
    );
    assert!(
        exit.stderr.contains(REFUSAL_TEXT),
        "rkat-rpc stderr must carry the ingress refusal text; stderr:\n{}",
        exit.stderr
    );
    assert!(
        exit.stderr.contains("Validation error"),
        "rkat-rpc stderr must surface the typed ConfigError::Validation; stderr:\n{}",
        exit.stderr
    );
    assert!(
        exit.stderr.contains(&format!("realm '{REALM_ID}'")),
        "rkat-rpc stderr must name the realm whose head document was refused; stderr:\n{}",
        exit.stderr
    );
}
