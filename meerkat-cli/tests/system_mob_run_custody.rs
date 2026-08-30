//! 0.8.22 field-report custody fixes, e2e-system rows (real `rkat` binary):
//!
//! - Finding 9 (P0): a non-detached `rkat mob run <pack>` must survive
//!   SIGTERM honestly. The run executes on the realm's durable mob custody;
//!   the signal converges it to a durable Canceled terminal, the CLI prints
//!   the honest terminal envelope, exits nonzero, and `rkat mob flow-status`
//!   agrees afterwards. Member sessions stay readable.
//! - Finding 10 (P1): `rkat mob run --detach` warns that flow execution
//!   custody dies with the CLI process, and the next hydration converges the
//!   orphaned run to Canceled carrying the typed `execution_custody_lost`
//!   cause (never a bare Canceled).
//!
//! Lane: `e2e-system` (local `make e2e-system` / nightly - NOT GitHub CI).
//! Registered as the `cli-mob-run-custody` suite in
//! `tests/integration/src/e2e_lanes.rs`.

#![cfg(feature = "integration-real-tests")]
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[path = "support/deterministic_mob_provider.rs"]
mod deterministic_mob_provider;

use deterministic_mob_provider::{
    MODEL as DETERMINISTIC_MODEL, PROVIDER as DETERMINISTIC_PROVIDER,
    remove_ambient_provider_credentials,
};

/// Budget for one CLI verb against a cold debug-build realm.
const VERB_TIMEOUT: Duration = Duration::from_secs(150);
/// Budget between run start and the stalled member turn's HTTP request.
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Budget between SIGTERM and the CLI's post-cancel exit (the in-process
/// cancel grace is 30s; the flow's own cancel grace is 2s).
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(90);

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("RKAT_TEST_BIN_RKAT") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        for profile in ["debug", "release"] {
            let candidate = target_dir.join(profile).join("rkat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    for target in ["target-codex", "target"] {
        for profile in ["debug", "release"] {
            let candidate = workspace_root.join(target).join(profile).join("rkat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

struct ConsoleHome {
    temp: TempDir,
    realm: &'static str,
}

impl ConsoleHome {
    fn new(realm: &'static str) -> Self {
        Self {
            temp: TempDir::new().expect("console home tempdir"),
            realm,
        }
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn command(&self, rkat: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(rkat);
        remove_ambient_provider_credentials(&mut cmd);
        cmd.current_dir(self.temp.path())
            .env("HOME", self.temp.path())
            .env("XDG_CONFIG_HOME", self.temp.path().join("config"))
            .arg("--state-root")
            .arg(self.temp.path().join("realms"))
            .arg("--realm")
            .arg(self.realm)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    async fn run_rkat(&self, rkat: &Path, args: &[&str]) -> std::process::Output {
        tokio::time::timeout(VERB_TIMEOUT, self.command(rkat, args).output())
            .await
            .unwrap_or_else(|_| panic!("rkat {args:?} exceeded the verb budget"))
            .expect("spawn rkat")
    }
}

fn stdout_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn assert_success(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed ({:?}):\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout_str(output),
        stderr_str(output),
    );
}

/// `mob run --json` appends `warning\t…` lines after the JSON document, so
/// parse the FIRST document in the stream.
fn first_json_document(raw: &str) -> serde_json::Value {
    serde_json::Deserializer::from_str(raw.trim())
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("expected a JSON document in output:\n{raw}"))
        .unwrap_or_else(|error| panic!("invalid JSON document ({error}):\n{raw}"))
}

/// Minimal mobpack fixture: one worker role, one `main` flow step, and a
/// short flow-level cancel grace so signal-driven convergence stays snappy.
/// `provider` annotates uncatalogued model ids (the self-hosted stall model).
async fn write_mobpack_fixture(
    project_dir: &Path,
    mob_id: &str,
    model: &str,
    provider: Option<&str>,
) -> PathBuf {
    let mob_dir = project_dir.join(format!("{mob_id}-fixture"));
    tokio::fs::create_dir_all(&mob_dir)
        .await
        .expect("fixture dir");
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        format!("[mobpack]\nname = \"{mob_id}\"\nversion = \"1.0.0\"\n"),
    )
    .await
    .expect("write manifest");
    let provider_annotation = provider
        .map(|provider| format!("\n      \"provider\":\"{provider}\","))
        .unwrap_or_default();
    let definition = format!(
        r#"{{
  "id":"{mob_id}",
  "profiles":{{
    "worker":{{
      "model":"{model}",{provider_annotation}
      "tools":{{"comms":true}},
      "peer_description":"Worker"
    }}
  }},
  "flows":{{
    "main":{{
      "description":"custody fixture flow",
      "steps":{{
        "work":{{
          "role":"worker",
          "message":"Say ok.",
          "timeout_ms":600000
        }}
      }}
    }}
  }},
  "limits":{{"cancel_grace_timeout_ms":2000}},
  "skills":{{}}
}}"#
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition)
        .await
        .expect("write definition");
    mob_dir
}

async fn pack_fixture(
    console: &ConsoleHome,
    rkat: &Path,
    fixture: &Path,
    pack_name: &str,
) -> PathBuf {
    let pack_path = console.path().join(pack_name);
    let packed = console
        .run_rkat(
            rkat,
            &[
                "mob",
                "pack",
                fixture.to_str().expect("utf8"),
                "-o",
                pack_path.to_str().expect("utf8"),
            ],
        )
        .await;
    assert_success(&packed, "rkat mob pack");
    pack_path
}

fn send_signal(pid: u32, signal: &str) {
    let status = std::process::Command::new("kill")
        .args([signal, pid.to_string().as_str()])
        .status()
        .expect("send signal");
    assert!(status.success(), "kill {signal} {pid} failed");
}

/// Finding 9 (P0): SIGTERM during a foreground `rkat mob run <pack>` must
/// yield an honest, durable Canceled terminal instead of total run loss.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-system"]
async fn integration_real_mob_run_pack_sigterm_converges_to_durable_canceled() {
    const MOB_ID: &str = "mob-run-sigterm";
    let rkat = rkat_binary_path().expect("rkat binary not found");
    let console = ConsoleHome::new("mob-run-sigterm-e2e");

    let (_provider, mut requests) =
        deterministic_mob_provider::install(&console.path().join("realms"), console.realm, MOB_ID)
            .await;
    let fixture = write_mobpack_fixture(
        console.path(),
        MOB_ID,
        DETERMINISTIC_MODEL,
        Some(DETERMINISTIC_PROVIDER),
    )
    .await;
    let pack_path = pack_fixture(&console, &rkat, &fixture, "mob-run-sigterm.mobpack").await;

    let mut run_child = console
        .command(
            &rkat,
            &[
                "mob",
                "run",
                pack_path.to_str().expect("utf8"),
                "--trust-policy",
                "permissive",
                "--json",
            ],
        )
        .kill_on_drop(true)
        .spawn()
        .expect("spawn rkat mob run");
    let run_pid = run_child.id().expect("run child pid");
    let mut run_stdout_pipe = run_child.stdout.take().expect("run child stdout");
    let run_stdout_task = tokio::spawn(async move {
        let mut output = String::new();
        let _ = run_stdout_pipe.read_to_string(&mut output).await;
        output
    });
    let mut run_stderr_pipe = run_child.stderr.take().expect("run child stderr");
    let run_stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        let _ = run_stderr_pipe.read_to_string(&mut output).await;
        output
    });

    // The stalled member turn's HTTP request proves the flow is mid-step. A
    // premature CLI exit is surfaced with its output instead of a bare
    // dispatch-budget timeout.
    tokio::select! {
        request = tokio::time::timeout(DISPATCH_TIMEOUT, requests.recv()) => {
            request
                .expect("flow step never dispatched a member turn before the budget")
                .expect("stall server closed before observing a member turn");
        }
        early_exit = run_child.wait() => {
            let status = early_exit.expect("await early rkat mob run exit");
            let child_stdout = run_stdout_task.await.expect("join stdout reader");
            let child_stderr = run_stderr_task.await.expect("join stderr reader");
            panic!(
                "rkat mob run exited before dispatching a member turn: {status}\nstdout: {child_stdout}\nstderr: {child_stderr}"
            );
        }
    }

    send_signal(run_pid, "-TERM");

    let status = match tokio::time::timeout(SHUTDOWN_TIMEOUT, run_child.wait()).await {
        Ok(status) => status.expect("wait for rkat mob run after SIGTERM"),
        Err(_) => {
            let _ = run_child.start_kill();
            let _ = run_child.wait().await;
            let run_stdout = run_stdout_task
                .await
                .expect("join stdout reader after timeout");
            let run_stderr = run_stderr_task
                .await
                .expect("join stderr reader after timeout");
            panic!(
                "rkat mob run did not exit after SIGTERM within the grace budget\nstdout: {run_stdout}\nstderr: {run_stderr}"
            );
        }
    };
    let run_stdout = run_stdout_task.await.expect("join stdout reader");
    let run_stderr = run_stderr_task.await.expect("join stderr reader");
    assert!(
        !status.success(),
        "a signal-interrupted run must exit nonzero\nstdout: {run_stdout}\nstderr: {run_stderr}"
    );
    assert!(
        run_stderr.contains("mob run interrupted"),
        "typed interruption error must surface on stderr: {run_stderr}"
    );

    // The honest terminal envelope was rendered before the nonzero exit.
    let envelope = first_json_document(&run_stdout);
    assert_eq!(
        envelope["status"], "canceled",
        "signal-driven terminal envelope must be honest: {run_stdout}"
    );
    let run_id = envelope["run_id"]
        .as_str()
        .expect("run_id in envelope")
        .to_string();
    assert_eq!(envelope["mob_id"], MOB_ID);

    // (b) The run exists in durable storage as Canceled and flow-status agrees.
    let status = console
        .run_rkat(&rkat, &["mob", "flow-status", MOB_ID, run_id.as_str()])
        .await;
    assert_success(&status, "rkat mob flow-status after SIGTERM");
    let status_doc = first_json_document(&stdout_str(&status));
    assert_eq!(
        status_doc["status"],
        "canceled",
        "durable flow-status must agree with the rendered terminal: {}",
        stdout_str(&status)
    );

    // The terminal carrier records an ordinary requested cancel, not a
    // custody loss: the signal path converged the run before process exit.
    let logs = console
        .run_rkat(&rkat, &["mob", "logs", MOB_ID, "--json"])
        .await;
    assert_success(&logs, "rkat mob logs after SIGTERM");
    let logs_doc = first_json_document(&stdout_str(&logs));
    let canceled_event = logs_doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|event| {
            event["kind"]["type"] == "flow_canceled" && event["kind"]["run_id"] == run_id.as_str()
        })
        .unwrap_or_else(|| panic!("missing flow_canceled event: {}", stdout_str(&logs)));
    assert_eq!(
        canceled_event["kind"]["cause"],
        "cancel_requested",
        "signal-driven cancel is an ordinary requested cancel: {}",
        stdout_str(&logs)
    );

    // (c) Member sessions remain readable.
    let sessions = console.run_rkat(&rkat, &["session", "list"]).await;
    assert_success(&sessions, "rkat session list after SIGTERM");
    assert!(
        !stdout_str(&sessions).contains("No sessions found."),
        "member sessions must remain readable after the canceled run: {}",
        stdout_str(&sessions)
    );
}

/// Finding 10 (P1): a detached run's orphaned row must converge on the next
/// hydration to Canceled WITH the typed execution-custody-lost cause, and the
/// detach output must warn about the custody boundary.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-system"]
async fn integration_real_mob_run_detach_converges_execution_custody_lost() {
    const MOB_ID: &str = "mob-run-detach-custody";
    let rkat = rkat_binary_path().expect("rkat binary not found");
    let console = ConsoleHome::new("mob-run-detach-custody-e2e");

    let (_provider, _requests) =
        deterministic_mob_provider::install(&console.path().join("realms"), console.realm, MOB_ID)
            .await;
    let fixture = write_mobpack_fixture(
        console.path(),
        MOB_ID,
        DETERMINISTIC_MODEL,
        Some(DETERMINISTIC_PROVIDER),
    )
    .await;
    let pack_path = pack_fixture(&console, &rkat, &fixture, "mob-run-detach.mobpack").await;

    let ran = console
        .run_rkat(
            &rkat,
            &[
                "mob",
                "run",
                pack_path.to_str().expect("utf8"),
                "--trust-policy",
                "permissive",
                "--detach",
                "--json",
            ],
        )
        .await;
    assert_success(&ran, "rkat mob run --detach");
    let run_stdout = stdout_str(&ran);
    let run_stderr = stderr_str(&ran);
    let run_doc = first_json_document(&run_stdout);
    assert_eq!(run_doc["mob_id"], MOB_ID);
    let run_id = run_doc["run_id"].as_str().expect("run_id").to_string();
    assert!(
        run_stderr.contains("execution custody lost"),
        "detach must warn about the execution-custody boundary on stderr: {run_stderr}"
    );

    // The detached CLI process has exited; the next hydration must converge
    // the orphaned run to an honest Canceled terminal.
    let status = console
        .run_rkat(&rkat, &["mob", "flow-status", MOB_ID, run_id.as_str()])
        .await;
    assert_success(&status, "rkat mob flow-status after detach exit");
    let status_doc = first_json_document(&stdout_str(&status));
    assert_eq!(
        status_doc["status"],
        "canceled",
        "orphaned detached run must converge to canceled: {}",
        stdout_str(&status)
    );

    let logs = console
        .run_rkat(&rkat, &["mob", "logs", MOB_ID, "--json"])
        .await;
    assert_success(&logs, "rkat mob logs after detach convergence");
    let logs_doc = first_json_document(&stdout_str(&logs));
    let canceled_event = logs_doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .find(|event| {
            event["kind"]["type"] == "flow_canceled" && event["kind"]["run_id"] == run_id.as_str()
        })
        .unwrap_or_else(|| panic!("missing flow_canceled event: {}", stdout_str(&logs)));
    assert_eq!(
        canceled_event["kind"]["cause"],
        "execution_custody_lost",
        "convergence must carry the typed custody-lost cause, never a bare Canceled: {}",
        stdout_str(&logs)
    );
}
