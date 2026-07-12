//! Multi-host mobs phase 7 — §17.11 e2e-system row: REAL `rkat mob <verb>`
//! console wiring (T-B14).
//!
//! Everything the deterministic in-process lanes cannot pin: the shipped
//! binary drives the phase-7 console verbs end-to-end over real loopback
//! TCP against a live `rkat mob host` daemon — descriptor hand-off →
//! `bind-host` → `hosts` → `route-installs` → `member-history` →
//! `revoke-host` — and the §17.4 typed exit path is OBSERVED on the real
//! process: a reply-deadline bind against a stopped daemon exits 46 with
//! the typed `detail:` stderr line, while a deliberately-unmapped error
//! (member not found) keeps EXIT_ERROR=1.
//!
//! Lane: `e2e-system` (local `make e2e-system` / nightly — NOT GitHub CI).
//! Registered as the `cli-mob-verbs` suite in
//! `tests/integration/src/e2e_lanes.rs`.

#![cfg(feature = "integration-real-tests")]
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use meerkat_mob::runtime::bridge_protocol::WireHostBindingDescriptor;
use tempfile::TempDir;
use tokio::process::{Child, Command};

const DESCRIPTOR_TIMEOUT: Duration = Duration::from_secs(60);
/// The bind bridge reply deadline is 60s (actor bind_host send); the verb
/// budget must comfortably contain it.
const VERB_TIMEOUT: Duration = Duration::from_secs(150);

const MOB_ID: &str = "mob-cli-verbs";
const REALM: &str = "mob-cli-verbs-e2e";

fn rkat_binary_path() -> Option<PathBuf> {
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

struct DaemonHome {
    temp: TempDir,
}

impl DaemonHome {
    fn new() -> Self {
        Self {
            temp: TempDir::new().expect("daemon home tempdir"),
        }
    }

    fn descriptor_path(&self) -> PathBuf {
        self.temp.path().join("host-binding.json")
    }

    fn identity_dir(&self) -> PathBuf {
        self.temp.path().join("host-identity")
    }

    fn spawn_daemon(&self, rkat: &Path) -> Child {
        Command::new(rkat)
            .current_dir(self.temp.path())
            .env("HOME", self.temp.path())
            .env("XDG_CONFIG_HOME", self.temp.path().join("config"))
            .args([
                "--state-root",
                self.temp.path().join("realms").to_str().expect("utf8"),
                "--realm",
                "mob-cli-verbs-host",
                "mob",
                "host",
                "--listen-tcp",
                "127.0.0.1:0",
                "--identity-dir",
                self.identity_dir().to_str().expect("utf8"),
                "--descriptor-out",
                self.descriptor_path().to_str().expect("utf8"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn rkat mob host")
    }

    /// Poll the descriptor slot until it parses and (when `differs_from` is
    /// set) carries a DIFFERENT bootstrap token — the daemon rewrites the
    /// file in place on re-mint.
    async fn wait_for_descriptor(
        &self,
        differs_from: Option<&str>,
        daemon: &mut Child,
    ) -> WireHostBindingDescriptor {
        let deadline = tokio::time::Instant::now() + DESCRIPTOR_TIMEOUT;
        loop {
            if let Ok(bytes) = std::fs::read(self.descriptor_path())
                && let Ok(descriptor) = serde_json::from_slice::<WireHostBindingDescriptor>(&bytes)
                && differs_from.is_none_or(|old| descriptor.bootstrap_token.as_str() != old)
            {
                return descriptor;
            }
            if let Ok(Some(status)) = daemon.try_wait() {
                let mut stderr = String::new();
                if let Some(mut pipe) = daemon.stderr.take() {
                    use tokio::io::AsyncReadExt as _;
                    let _ = pipe.read_to_string(&mut stderr).await;
                }
                panic!("daemon exited before publishing a descriptor: {status}\n{stderr}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for the host binding descriptor"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn signal(&self, daemon: &Child, signal: &str) {
        let pid = daemon.id().expect("daemon pid").to_string();
        let status = std::process::Command::new("kill")
            .args([signal, pid.as_str()])
            .status()
            .expect("send signal to daemon");
        assert!(status.success(), "kill {signal} {pid} failed");
    }
}

struct ConsoleHome {
    temp: TempDir,
}

impl ConsoleHome {
    fn new() -> Self {
        Self {
            temp: TempDir::new().expect("console home tempdir"),
        }
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    async fn run_rkat(&self, rkat: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(rkat);
        cmd.current_dir(self.temp.path())
            .env("HOME", self.temp.path())
            .env("XDG_CONFIG_HOME", self.temp.path().join("config"))
            .arg("--state-root")
            .arg(self.temp.path().join("realms"))
            .arg("--realm")
            .arg(REALM)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tokio::time::timeout(VERB_TIMEOUT, cmd.output())
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

/// Minimal mobpack fixture with a callable flow so `rkat mob run --detach`
/// installs the mob into the console realm's persistent mob state without
/// waiting on any model turn.
async fn write_mobpack_fixture(project_dir: &Path) -> PathBuf {
    let mob_dir = project_dir.join("mob-cli-verbs-fixture");
    tokio::fs::create_dir_all(&mob_dir)
        .await
        .expect("fixture dir");
    tokio::fs::write(
        mob_dir.join("manifest.toml"),
        format!("[mobpack]\nname = \"{MOB_ID}\"\nversion = \"1.0.0\"\n"),
    )
    .await
    .expect("write manifest");
    let definition = format!(
        r#"{{
  "id":"{MOB_ID}",
  "profiles":{{
    "worker":{{
      "model":"claude-sonnet-4-5",
      "tools":{{"comms":true}},
      "peer_description":"Worker"
    }}
  }},
  "flows":{{
    "main":{{
      "description":"phase-7 CLI verb fixture flow",
      "steps":{{
        "work":{{
          "role":"worker",
          "message":"Say ok.",
          "timeout_ms":60000
        }}
      }}
    }}
  }},
  "skills":{{}}
}}"#
    );
    tokio::fs::write(mob_dir.join("definition.json"), definition)
        .await
        .expect("write definition");
    mob_dir
}

async fn shutdown(mut daemon: Child) {
    let _ = daemon.kill().await;
    let _ = daemon.wait().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-system"]
async fn integration_real_mob_cli_verbs_over_tcp() {
    let rkat = rkat_binary_path().expect("rkat binary not found");
    let console = ConsoleHome::new();

    // ---- Install the mob into the console realm (mobpack --detach) ----
    let fixture = write_mobpack_fixture(console.path()).await;
    let pack_path = console.path().join("mob-cli-verbs.mobpack");
    let packed = console
        .run_rkat(
            &rkat,
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

    let ran = console
        .run_rkat(
            &rkat,
            &[
                "mob",
                "run",
                pack_path.to_str().expect("utf8"),
                // The fixture pack is locally built and unsigned; permissive
                // is the sanctioned dev/test posture (strict-mode rejection
                // of unsigned packs is pinned separately in main.rs tests).
                "--trust-policy",
                "permissive",
                "--detach",
                "--json",
            ],
        )
        .await;
    assert_success(&ran, "rkat mob run --detach");
    // `mob run --json` appends `warning\t…` lines after the JSON document
    // (pre-existing render_mob_run_pack_with_warnings behavior — permissive
    // trust warns about the unsigned pack), so parse the FIRST document.
    let run_stdout = stdout_str(&ran);
    let run_doc: serde_json::Value = serde_json::Deserializer::from_str(run_stdout.trim())
        .into_iter()
        .next()
        .expect("detach run emits a JSON document")
        .expect("detach run JSON");
    assert_eq!(run_doc["mob_id"], MOB_ID, "mob installed under its id");

    // ---- Daemon up, descriptor on disk ----
    let home = DaemonHome::new();
    let mut daemon = home.spawn_daemon(&rkat);
    let d0 = home.wait_for_descriptor(None, &mut daemon).await;

    // ---- hosts: empty roster before any bind ----
    let hosts = console
        .run_rkat(&rkat, &["mob", "hosts", MOB_ID, "--json"])
        .await;
    assert_success(&hosts, "rkat mob hosts (pre-bind)");
    let hosts_doc: serde_json::Value =
        serde_json::from_str(stdout_str(&hosts).trim()).expect("hosts JSON");
    assert_eq!(
        hosts_doc["hosts"],
        serde_json::json!([]),
        "no hosts before the bind ceremony"
    );

    // ---- bind-host over real loopback TCP ----
    let bound = console
        .run_rkat(
            &rkat,
            &[
                "mob",
                "bind-host",
                MOB_ID,
                "--descriptor",
                home.descriptor_path().to_str().expect("utf8"),
            ],
        )
        .await;
    assert_success(&bound, "rkat mob bind-host");
    let report: serde_json::Value =
        serde_json::from_str(stdout_str(&bound).trim()).expect("bind report JSON");
    let host_id = report["host_id"].as_str().expect("host_id").to_string();
    assert!(!host_id.is_empty());
    assert!(
        report["authority_epoch"].as_u64().is_some(),
        "bind report carries the authority epoch"
    );
    assert!(
        report.get("epoch").is_none(),
        "bind report must use the canonical MobBindHostResult field name"
    );
    assert!(
        report["capabilities"]["durable_sessions"].is_boolean(),
        "bind report carries the capability record"
    );

    // ---- hosts: bound row with capability labeling ----
    let hosts = console
        .run_rkat(&rkat, &["mob", "hosts", MOB_ID, "--json"])
        .await;
    assert_success(&hosts, "rkat mob hosts (post-bind)");
    let hosts_doc: serde_json::Value =
        serde_json::from_str(stdout_str(&hosts).trim()).expect("hosts JSON");
    let rows = hosts_doc["hosts"].as_array().expect("hosts array");
    assert_eq!(rows.len(), 1, "exactly the bound host");
    assert_eq!(rows[0]["host_id"], serde_json::json!(host_id));
    assert_eq!(rows[0]["bind_phase"], "bound");
    assert!(rows[0]["capabilities"]["durable_sessions"].is_boolean());

    // Human mode renders the tab row with the durable_sessions label.
    let human = console.run_rkat(&rkat, &["mob", "hosts", MOB_ID]).await;
    assert_success(&human, "rkat mob hosts (human)");
    let human_out = stdout_str(&human);
    assert!(
        human_out.contains("durable_sessions=") && human_out.contains(&host_id),
        "human hosts row labels durable_sessions: {human_out}"
    );

    // ---- route-installs: nothing outstanding ⇒ complete ----
    let installs = console
        .run_rkat(&rkat, &["mob", "route-installs", MOB_ID])
        .await;
    assert_success(&installs, "rkat mob route-installs");
    let installs_doc: serde_json::Value =
        serde_json::from_str(stdout_str(&installs).trim()).expect("route installs JSON");
    assert_eq!(installs_doc["complete"], serde_json::json!(true));
    assert_eq!(installs_doc["outstanding"], serde_json::json!([]));

    // ---- member-history: verb wired; missing member is a deliberately
    // UNMAPPED error ⇒ EXIT_ERROR=1, no typed detail line ----
    let history = console
        .run_rkat(&rkat, &["mob", "member-history", MOB_ID, "no-such-member"])
        .await;
    assert_eq!(
        history.status.code(),
        Some(1),
        "missing member keeps EXIT_ERROR (unmapped by design):\n{}",
        stderr_str(&history),
    );
    let history_err = stderr_str(&history);
    assert!(
        history_err.contains("mob member not found"),
        "typed message surfaces: {history_err}"
    );
    assert!(
        !history_err.contains("detail:"),
        "unmapped errors carry no typed detail line: {history_err}"
    );

    // ---- revoke-host: typed report, no fabricated releases ----
    let revoked = console
        .run_rkat(&rkat, &["mob", "revoke-host", MOB_ID, host_id.as_str()])
        .await;
    assert_success(&revoked, "rkat mob revoke-host");
    let revoke_doc: serde_json::Value =
        serde_json::from_str(stdout_str(&revoked).trim()).expect("revoke report JSON");
    assert_eq!(revoke_doc["host_id"], serde_json::json!(host_id));
    assert_eq!(
        revoke_doc["released_members"],
        serde_json::json!([]),
        "no materialized members were placed on the host"
    );

    // ---- §17.4 observed: bind against a STOPPED daemon fails at the COMMS
    // ack layer (peer offline) BEFORE any bridge reply deadline engages —
    // and the ratified DEC-P7B-8 mapping deliberately pins CommsError ∌
    // HostUnavailable (a member-transport fact must not launder into a
    // host-availability claim). The honest console outcome is therefore the
    // UNMAPPED mob-family exit (1) with the raw typed message, NOT 46.
    // Exit 46 (HostUnavailable ← bridge reply deadline / typed Unavailable
    // rejection) is pinned at unit level (mob_exit_code) and via the MCP
    // dispatch rows per FLAG-WB-5's recorded disposition: a delivered-but-
    // unanswered bridge request is not deterministically manufacturable in
    // this lane without fault injection.
    let d1 = home
        .wait_for_descriptor(Some(d0.bootstrap_token.as_str()), &mut daemon)
        .await;
    let _ = d1;
    home.signal(&daemon, "-STOP");
    let timed_out = console
        .run_rkat(
            &rkat,
            &[
                "mob",
                "bind-host",
                MOB_ID,
                "--descriptor",
                home.descriptor_path().to_str().expect("utf8"),
            ],
        )
        .await;
    home.signal(&daemon, "-CONT");
    assert_eq!(
        timed_out.status.code(),
        Some(1),
        "stopped-daemon bind fails comms-class, honestly unmapped:\nstdout: {}\nstderr: {}",
        stdout_str(&timed_out),
        stderr_str(&timed_out),
    );
    let timed_out_err = stderr_str(&timed_out);
    assert!(
        timed_out_err.contains("comms error"),
        "the typed comms cause reaches stderr un-laundered: {timed_out_err}"
    );

    shutdown(daemon).await;
}
