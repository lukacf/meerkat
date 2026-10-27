//! One inert admitted worker through the installed factory-owned MCP lifecycle.
//! The fixture changes one job body, not launcher/adapter/backend bytes or close.

use super::*;
use std::{future::Future, path::Path};
use meerkat_connect::{ConnectError, setup::PreparedSetupConnection};
use nix::{errno::Errno, sys::signal::{Signal, kill, killpg}, unistd::{Pid, getpgid, getsid}};
use sha2::{Digest, Sha256};

const BOUND: Duration = Duration::from_secs(20);
const HELD: Duration = Duration::from_millis(500);
const SHIM: &str = include_str!("active_worker.py");
type CloseResult = std::result::Result<(), ConnectError>;

struct Fixture {
    root: PathBuf,
    token: String,
}

#[derive(Clone, Copy)]
struct Identity { pid: i32, pgid: i32, sid: i32, ppid: i32 }

impl Fixture {
    fn marker(&self, name: &str) -> Result<Value> {
        let value: Value = serde_json::from_slice(&std::fs::read(self.root.join(format!("{name}.json")))?)?;
        require(value["token"] == self.token, "marker belongs to another fixture")?;
        Ok(value)
    }

    async fn wait_marker(&self, name: &str) -> Result<Value> {
        tokio::time::timeout(BOUND, async {
            while !self.root.join(format!("{name}.json")).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.marker(name)
        }).await.map_err(|_| std::io::Error::other(format!("missing {name} observation")))?
    }

    fn identity(&self, name: &str) -> Result<Identity> {
        let value = self.marker(name)?;
        let integer = |key: &str| -> Result<i32> {
            let id = value[key].as_i64().and_then(|id| i32::try_from(id).ok())
                .ok_or("invalid process identity")?;
            require(id > if key == "ppid" { 0 } else { 1 }, "process identity is not a signalable fixture process")?;
            Ok(id)
        };
        Ok(Identity { pid: integer("pid")?, pgid: integer("pgid")?, sid: integer("sid")?, ppid: integer("ppid")? })
    }

    fn release(&self) -> Result {
        let temporary = self.root.join("release.pending");
        std::fs::write(&temporary, &self.token)?;
        std::fs::rename(temporary, self.root.join("release"))?;
        Ok(())
    }

    fn connection(&self, prepared: &Path, probe: &Probe) -> Result<PreparedSetupConnection> {
        let raw = std::fs::read(prepared.join("prepare.stdout"))?;
        let mut descriptor: Value = serde_json::from_slice(&raw)?;
        let server = &mut descriptor["mcpServers"]["toolkit-setup"];
        let args = server["args"].as_array().ok_or("prepared arguments unavailable")?
            .iter().map(|value| value.as_str().map(str::to_owned).ok_or("invalid prepared argument"))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        require(server["command"] == "/usr/bin/env" && args.len() > 7
            && args[..3] == ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1"]
            && args[4..6] == ["-I", "-B"] && Path::new(&args[3]).is_absolute()
            && Path::new(&args[6]).file_name().and_then(|name| name.to_str()) == Some("local_session.py"),
            "fixture requires the unchanged current installed launcher envelope")?;
        let shim = self.root.join("active_worker.py");
        std::fs::write(&shim, SHIM)?;
        // Retain the exact interpreter/environment prefix and all original
        // launcher arguments. The shim delegates to that installed launcher.
        let mut wrapped = args[..6].to_vec();
        wrapped.extend([shim.to_str().ok_or("fixture path is not UTF-8")?.to_owned(),
            self.root.to_str().ok_or("fixture root is not UTF-8")?.to_owned(), self.token.clone()]);
        wrapped.extend_from_slice(&args[6..]);
        server["args"] = json!(wrapped);
        let selected = serde_json::to_vec(&descriptor)?;
        std::fs::write(self.root.join("fixture-mcp.json"), &selected)?;
        probe.record(json!({"kind":"active_worker_fixture_selection", "original_descriptor_sha256":format!("{:x}", Sha256::digest(&raw)),
            "fixture_descriptor_sha256":format!("{:x}", Sha256::digest(&selected)), "shim_sha256":format!("{:x}", Sha256::digest(SHIM.as_bytes())),
            "substitution":"one Services preparation job body; actual Controller.start_job and Runner.run", "launcher_prefix_preserved":true}));
        PreparedSetupConnection::from_preparer_output(&selected).map_err(Into::into)
    }
}

fn alive(identity: Identity) -> bool {
    kill(Pid::from_raw(identity.pid), None).is_ok()
        && getpgid(Some(Pid::from_raw(identity.pid))).is_ok_and(|id| id.as_raw() == identity.pgid)
        && getsid(Some(Pid::from_raw(identity.pid))).is_ok_and(|id| id.as_raw() == identity.sid)
}

fn absent(identity: Identity) -> bool {
    kill(Pid::from_raw(identity.pid), None) == Err(Errno::ESRCH)
        && (identity.pgid != identity.pid || killpg(Pid::from_raw(identity.pgid), None) == Err(Errno::ESRCH))
}

async fn enter(session: &ConnectSession, fixture: &Fixture, probe: &Probe) -> Result<String> {
    await_setup_catalog(session, probe, PREPARATION_DENIED).await?;
    setup_load_accepted(&value(call_before(session, probe, "tool_catalog_load", json!({"names":SETUP_TOOLS}),
        Some(tokio::time::Instant::now() + BOUND)).await?)?, PREPARATION_DENIED)?;
    let job = value(call_before(session, probe, "setup_prepare", json!({"family":"services"}),
        Some(tokio::time::Instant::now() + BOUND)).await?)?;
    require(job["job"]["kind"] == "prepare" && job["job"]["status"] == "running", "controlled job was not admitted")?;
    let id = job["job"]["id"].as_str().ok_or("controlled job identity missing")?.to_owned();
    fixture.wait_marker("child-entered").await?;
    let child = fixture.identity("child-entered")?;
    let adapter = fixture.identity("adapter-entered")?;
    require(child.pid == child.pgid && child.pid == child.sid && child.ppid == adapter.pid
        && child.pid != adapter.pid && child.sid != adapter.sid
        && child.pid != i32::try_from(std::process::id())? && alive(child) && alive(adapter),
        "Runner child did not enter its own live process session")?;
    require(!fixture.root.join("release").exists() && !fixture.root.join("child-completed.json").exists(),
        "worker was released before native shutdown")?;
    probe.record(json!({"kind":"active_worker_entered", "job_id":id,
        "child":fixture.marker("child-entered")?, "adapter":fixture.marker("adapter-entered")?}));
    Ok(id)
}

async fn observe(
    fixture: &Fixture, probe: &Probe, job: &str,
    mut close: Pin<&mut impl Future<Output = CloseResult>>, closed: &mut Option<CloseResult>,
) -> Result {
    probe.record(json!({"kind":"native_close_started_with_held_worker"}));
    let admission = tokio::select! {
        outcome = close.as_mut() => {
            *closed = Some(outcome);
            return Err("native close finished while the controlled child was held".into());
        }
        value = fixture.wait_marker("admission-closed") => value?,
    };
    require(admission["closing"] == true, "adapter admission remained open")?;
    probe.record(json!({"kind":"adapter_admission_closed", "observation":admission}));
    tokio::select! {
        outcome = close.as_mut() => {
            *closed = Some(outcome);
            return Err("native close finished before the controlled worker release".into());
        }
        _ = tokio::time::sleep(HELD) => {}
    }
    let child = fixture.identity("child-entered")?;
    let adapter = fixture.identity("adapter-entered")?;
    require(alive(child) && alive(adapter) && !fixture.root.join("child-completed.json").exists()
        && !fixture.root.join("worker-returned.json").exists() && !fixture.root.join("controller-joined.json").exists(),
        "held worker or adapter exited, or join was reported before release")?;
    probe.record(json!({"kind":"original_native_close_pending_with_live_held_child", "held_ms":HELD.as_millis()}));
    fixture.release()?;
    probe.record(json!({"kind":"controlled_child_released"}));
    *closed = Some(tokio::time::timeout(BOUND, close.as_mut()).await
        .map_err(|_| std::io::Error::other("native close did not join after release; original future retained"))?);
    require(closed.as_ref().is_some_and(|result| result.is_ok()), "native close returned an error")?;
    let completed = fixture.marker("child-completed")?;
    let worker = fixture.marker("worker-returned")?;
    let joined = fixture.marker("controller-joined")?;
    let returned = fixture.marker("adapter-returned")?;
    require(completed["pid"] == child.pid && completed["pgid"] == child.pgid && completed["sid"] == child.sid
        && worker["exit_code"] == 0 && worker["output_bytes"] == 0 && worker["child_reaped"] == true
        && joined["closing"] == true && joined["workers_joined"] == true && joined["job_count"] == 1
        && joined["job"]["id"] == job && joined["job"]["status"] == "succeeded"
        && joined["job"]["result"]["scope"] == "inert_shutdown_fixture_only"
        && returned["launcher_return_code"] == 0 && returned["sources_unchanged"] == true,
        "graceful child/worker/controller/adapter completion evidence is incomplete")?;
    require(absent(child) && absent(adapter), "native close left an observed process or owned group alive")?;
    probe.record(json!({"kind":"active_worker_native_shutdown_passed_before_cleanup", "child_completed":completed,
        "worker_returned":worker,"controller_joined":joined,"adapter_returned":returned,
        "observed_child_and_adapter_absent":true,"failure_cleanup_used":false}));
    Ok(())
}

async fn failure_cleanup(
    fixture: &Fixture, probe: &Probe, mut close: Pin<&mut impl Future<Output = CloseResult>>,
    closed: &mut Option<CloseResult>,
) {
    let released = fixture.release();
    probe.record(json!({"kind":"failure_cleanup_started", "gate_release_error":released.err().map(|error| error.to_string())}));
    if closed.is_none() {
        if let Ok(outcome) = tokio::time::timeout(BOUND, close.as_mut()).await { *closed = Some(outcome); }
    }
    // Failure only. Give the actual adapter the chance to reap after release.
    // A PID is signalable only while it still has the recorded session/group.
    for name in ["child-entered", "adapter-entered"] {
        if let Ok(identity) = fixture.identity(name) {
            let graceful = tokio::time::timeout(BOUND, async {
                while !absent(identity) { tokio::time::sleep(Duration::from_millis(10)).await; }
            }).await.is_ok();
            if !graceful && alive(identity) && identity.pid != i32::try_from(std::process::id()).unwrap_or(0) {
                probe.record(json!({"kind":"failure_cleanup_signals_observed_fixture", "marker":name,"pid":identity.pid}));
                let _ = if identity.pgid == identity.pid {
                    killpg(Pid::from_raw(identity.pgid), Some(Signal::SIGKILL))
                } else { kill(Pid::from_raw(identity.pid), Some(Signal::SIGKILL)) };
                // Let the still-owned adapter reap its direct child before any
                // fallback signal to the adapter itself.
                let _ = tokio::time::timeout(BOUND, async {
                    while !absent(identity) { tokio::time::sleep(Duration::from_millis(10)).await; }
                }).await;
            }
        }
    }
    if closed.is_none() {
        // Never create a second close or drop the first after a deadline. If the
        // native owner still cannot settle, the external runner records failure.
        probe.record(json!({"kind":"failure_waits_original_native_close"}));
        *closed = Some(close.as_mut().await);
    }
    for name in ["child-entered", "adapter-entered"] {
        if let Ok(identity) = fixture.identity(name) {
            let settled = tokio::time::timeout(BOUND, async {
                while !absent(identity) { tokio::time::sleep(Duration::from_millis(10)).await; }
            }).await.is_ok();
            probe.record(json!({"kind":"failure_cleanup_process_observation", "marker":name,"absent":settled}));
        }
    }
}

pub(super) async fn run() -> Result {
    let current = std::env::current_dir()?;
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent)
        .ok_or("compiled checkout location unavailable")?;
    require(!current.starts_with(checkout), "acceptance must run outside the checkout")?;
    let path = std::env::var_os("PATH").ok_or("explicit empty-runtime PATH required")?;
    for directory in std::env::split_paths(&path) {
        require(directory.is_absolute() && directory.is_dir(), "PATH must name an absolute empty-runtime directory")?;
        for name in ["python", "python3", "python3.11", "node", "npm"] {
            require(!directory.join(name).exists(), "PATH contains a forbidden runtime")?;
        }
    }
    let root = tempfile::Builder::new().prefix("connect-active-worker-acceptance-").tempdir()?.keep().canonicalize()?;
    eprintln!("Retained active worker acceptance: {}", root.display());
    let fixture = Fixture { root, token: meerkat_core::SessionId::new().to_string() };
    let storage = RealmOptions { context_root: current, state_root: Some(fixture.root.join("state")),
        user_config_root: Some(fixture.root.join("home")) }.resolve("active-worker-acceptance")?;
    let prepared = InstalledResources::locate()?.prepare(&storage, std::future::pending()).await?;
    let probe = Arc::new(Probe { inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        command: Mutex::new(None), trace: fixture.root.join("native-dispatch.jsonl"),
        stall: std::sync::atomic::AtomicBool::new(false), started: tokio::sync::Notify::new() });
    let connection = fixture.connection(&prepared.artifact_directory, &probe)?;
    let options = connection.preparation_session(SessionOptions {
        model: meerkat::CreateSessionModelResolutionRequest { model: Some("gpt-5.4".into()), ..Default::default() },
        build: SessionBuildOptions { llm_client_override: Some(meerkat::encode_llm_client_override_for_service(probe.clone())),
            ..Default::default() },
    })?;
    let session = ConnectSession::open_with_config(storage, Config::default(), options).await?;
    let entered = enter(&session, &fixture, &probe).await;
    let close = session.close();
    tokio::pin!(close);
    let mut closed = None;
    let result = match entered {
        Ok(job) => observe(&fixture, &probe, &job, close.as_mut(), &mut closed).await,
        Err(error) => Err(error),
    };
    if result.is_err() { failure_cleanup(&fixture, &probe, close.as_mut(), &mut closed).await; }
    probe.record(json!({"kind":"active_worker_acceptance_result", "passed":result.is_ok(),
        "exercise_error":result.as_ref().err().map(ToString::to_string),
        "native_close":closed.as_ref().map(|value| format!("{value:?}")), "failure_cleanup_used":result.is_err()}));
    // Preserve a native failure's retained owner in the returned error.
    closed.ok_or("original native close has no terminal observation")??;
    result
}
