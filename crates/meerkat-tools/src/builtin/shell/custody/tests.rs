#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::builtin::shell::{SecurityMode, ShellConfig, ShellTool};
use serde_json::json;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use tempfile::TempDir;

const GATEWAY_ROOT_ENV: &str = "MEERKAT_TEST_CUSTODY_GATEWAY_ROOT";
const GATEWAY_SCOPE_ENV: &str = "MEERKAT_TEST_CUSTODY_GATEWAY_SCOPE";
const GATEWAY_PROJECT_ENV: &str = "MEERKAT_TEST_CUSTODY_GATEWAY_PROJECT";
const GATEWAY_FIFO_ENV: &str = "MEERKAT_TEST_CUSTODY_GATEWAY_FIFO";
const GATEWAY_EFFECT_ENV: &str = "MEERKAT_TEST_CUSTODY_GATEWAY_EFFECT";
const GATEWAY_CHILD_TEST: &str = "builtin::shell::custody::tests::custody_gateway_child_role";

fn sh_config(project: &Path) -> ShellConfig {
    ShellConfig {
        enabled: true,
        default_timeout_secs: 60,
        restrict_to_project: false,
        shell: "sh".to_string(),
        shell_path: Some(PathBuf::from("/bin/sh")),
        project_root: project.to_path_buf(),
        security_mode: SecurityMode::Unrestricted,
        ..Default::default()
    }
}

fn scope() -> ProcessCustodyScope {
    ProcessCustodyScope::session(&SessionId::new())
}

/// Identity of a process that has exited and been reaped.
fn dead_identity() -> ProcessIdentity {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "read line"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let identity = ProcessIdentity::capture(child.id() as i32)
        .unwrap()
        .unwrap();
    drop(child.stdin.take());
    child.wait().unwrap();
    identity
}

/// A live process in its own new process group (pgid == pid).
fn spawn_group(script: &str) -> Child {
    Command::new("/bin/sh")
        .args(["-c", script])
        .process_group(0)
        .spawn()
        .unwrap()
}

fn spawned_phase(pid: i32) -> CustodyPhase {
    CustodyPhase::Spawned {
        leader: ProcessIdentity::capture(pid).unwrap().unwrap(),
        session_leader: nix::unistd::getsid(Some(nix::unistd::Pid::from_raw(pid)))
            .unwrap()
            .as_raw(),
    }
}

fn write_prior_record(
    root: &Path,
    scope: &ProcessCustodyScope,
    incarnation: Uuid,
    host: ProcessIdentity,
    phase: CustodyPhase,
) -> PathBuf {
    write_prior_record_in(
        root,
        scope,
        incarnation,
        host,
        super::incarnation().unwrap().environment,
        phase,
    )
}

fn write_prior_record_in(
    root: &Path,
    scope: &ProcessCustodyScope,
    incarnation: Uuid,
    host: ProcessIdentity,
    environment: HostEnvironment,
    phase: CustodyPhase,
) -> PathBuf {
    let record = CustodyRecord {
        version: RECORD_VERSION,
        entry_id: Uuid::new_v4(),
        scope: scope.as_str().to_owned(),
        incarnation,
        host,
        environment,
        tool_call_id: Some("call-prior".to_owned()),
        phase,
    };
    let dir = root.join(scope.as_str());
    let path = dir.join(format!("{}.{RECORD_EXTENSION}", record.entry_id));
    write_record_blocking(&dir, &path, &serde_json::to_vec(&record).unwrap()).unwrap();
    path
}

fn live_members(pgid: i32) -> Vec<i32> {
    sys::group_members(pgid)
        .unwrap()
        .into_iter()
        .filter(|(pid, member)| {
            member.pgid == pgid
                && sys::is_running(&ProcessIdentity {
                    pid: *pid,
                    start: member.start,
                })
                .unwrap()
        })
        .map(|(pid, _)| pid)
        .collect()
}

fn process_running(pid: i32) -> bool {
    ProcessIdentity::capture(pid)
        .unwrap()
        .is_some_and(|identity| identity.is_running().unwrap())
}

fn record_files(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(RECORD_EXTENSION))
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("list custody dir: {error}"),
    }
}

#[tokio::test]
async fn recovery_kills_prior_incarnation_group_and_awaits_every_member() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut leader = spawn_group("sleep 60 & sleep 60; wait");
    let pgid = leader.id() as i32;
    // Wait until the background member exists so the group has two members.
    for _ in 0..500 {
        if live_members(pgid).len() >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let path = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        spawned_phase(pgid),
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(report.recovered.len(), 1);
    let recovered = &report.recovered[0];
    assert_eq!(recovered.tool_call_id.as_deref(), Some("call-prior"));
    assert!(
        matches!(
            recovered.cessation,
            ToolProcessCessation::KilledByRecovery { members } if members >= 2
        ),
        "{recovered:?}"
    );
    assert!(
        live_members(pgid).is_empty(),
        "recovery returned before every group member exited"
    );
    assert!(!path.exists(), "settled record must be removed");
    leader.wait().unwrap();
}

#[tokio::test]
async fn recovery_settles_unreleased_reservation_of_dead_host_as_never_started() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let path = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        CustodyPhase::Reserved,
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(report.recovered.len(), 1);
    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::NeverStarted
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn recovery_refuses_to_kill_a_group_its_live_prior_host_supervises() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut host = Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .spawn()
        .unwrap();
    let host_identity = ProcessIdentity::capture(host.id() as i32).unwrap().unwrap();
    let mut leader = spawn_group("exec sleep 60");
    let pgid = leader.id() as i32;
    let path = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        host_identity,
        spawned_phase(pgid),
    );

    let error = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .expect_err("a live prior host still supervises its tool");

    assert!(
        matches!(error, ProcessCustodyError::PriorIncarnationAlive { .. }),
        "{error:?}"
    );
    assert!(process_running(pgid), "the tool must not be killed");
    assert!(path.exists(), "the record must stay until settled");
    leader.kill().unwrap();
    leader.wait().unwrap();
    host.kill().unwrap();
    host.wait().unwrap();
}

#[tokio::test]
async fn recovery_never_signals_a_process_that_reused_the_leader_pid() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut unrelated = spawn_group("exec sleep 60");
    let pid = unrelated.id() as i32;
    let CustodyPhase::Spawned {
        mut leader,
        session_leader,
    } = spawned_phase(pid)
    else {
        unreachable!()
    };
    // Same pid, different start stamp: the recorded tool is gone and the
    // pid now names an unrelated process.
    leader.start = match leader.start {
        ProcessStartStamp::LinuxBoot {
            boot_id,
            start_ticks,
        } => ProcessStartStamp::LinuxBoot {
            boot_id,
            start_ticks: start_ticks.saturating_sub(1),
        },
        ProcessStartStamp::Darwin {
            start_sec,
            start_usec,
        } => ProcessStartStamp::Darwin {
            start_sec: start_sec.saturating_sub(1),
            start_usec,
        },
    };
    write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        CustodyPhase::Spawned {
            leader,
            session_leader,
        },
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::AlreadyExited
    );
    assert!(process_running(pid), "an unrelated process must survive");
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[tokio::test]
async fn current_incarnation_records_are_live_owned_and_not_recovered() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let current = incarnation().unwrap();
    let path = write_prior_record(
        root.path(),
        &scope,
        current.id,
        current.host,
        CustodyPhase::Reserved,
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert!(report.recovered.is_empty());
    assert!(path.exists());
}

#[tokio::test]
async fn corrupt_record_fails_recovery_closed() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let dir = root.path().join(scope.as_str());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{}.json", Uuid::new_v4())), b"{not json").unwrap();

    let error = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .expect_err("an unreadable record cannot prove cessation");

    assert!(
        matches!(error, ProcessCustodyError::CorruptRecord { .. }),
        "{error:?}"
    );
}

#[test]
fn invalid_scope_is_rejected() {
    let scope = ProcessCustodyScope("../escape".to_owned());
    assert!(matches!(
        scope.validate(),
        Err(ProcessCustodyError::InvalidScope(_))
    ));
}

#[tokio::test]
async fn unreleased_spawn_gate_never_runs_the_command() {
    let temp = TempDir::new().unwrap();
    let effect = temp.path().join("effect");
    let mut gate = SpawnGate::new(Uuid::new_v4()).unwrap();
    let mut command = gate
        .command(
            Path::new("/bin/sh"),
            &format!("touch '{}'", effect.display()),
        )
        .unwrap();
    let mut child = command.spawn().unwrap();
    gate.spawned();
    // The host "dies" before release: its writer closes.
    drop(gate);

    let status = child.wait().await.unwrap();

    assert_eq!(status.code(), Some(gate::GATE_NOT_RELEASED_EXIT));
    assert!(
        !effect.exists(),
        "an unreleased gate must never run the command"
    );
}

#[tokio::test]
async fn released_spawn_gate_execs_the_shell_in_place() {
    let temp = TempDir::new().unwrap();
    let pid_file = temp.path().join("pid");
    let mut gate = SpawnGate::new(Uuid::new_v4()).unwrap();
    let mut command = gate
        .command(
            Path::new("/bin/sh"),
            &format!("echo $$ > '{}'; test ! -e /dev/fd/3", pid_file.display()),
        )
        .unwrap();
    let mut child = command.spawn().unwrap();
    let spawned_pid = child.id().unwrap();
    gate.spawned();
    gate.release().unwrap();

    let status = child.wait().await.unwrap();

    assert!(
        status.success(),
        "gate descriptor must not leak to the tool"
    );
    let recorded: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(recorded, spawned_pid, "the tool keeps the recorded pid");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn record_from_another_boot_or_namespace_is_never_signalled() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut leader = spawn_group("exec sleep 60");
    let pgid = leader.id() as i32;
    let HostEnvironment::Linux {
        pid_namespace_dev,
        pid_namespace_ino,
        ..
    } = super::incarnation().unwrap().environment
    else {
        unreachable!()
    };
    // Same pids, but recorded under another boot: they name nothing here.
    write_prior_record_in(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        HostEnvironment::Linux {
            boot_id: Uuid::new_v4(),
            pid_namespace_dev,
            pid_namespace_ino,
        },
        spawned_phase(pgid),
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::PriorEnvironmentEnded
    );
    assert!(
        process_running(pgid),
        "a local process must not be signalled"
    );
    leader.kill().unwrap();
    leader.wait().unwrap();
}

#[tokio::test]
async fn spawn_gate_ignores_a_line_without_its_token() {
    use nix::unistd::write;

    let temp = TempDir::new().unwrap();
    let effect = temp.path().join("effect");
    let mut gate = SpawnGate::new(Uuid::new_v4()).unwrap();
    let mut command = gate
        .command(
            Path::new("/bin/sh"),
            &format!("touch '{}'", effect.display()),
        )
        .unwrap();
    let mut child = command.spawn().unwrap();
    gate.spawned();
    // A stray writer that does not know the token cannot release the gate.
    write(&gate.write, b"\n").unwrap();

    let status = child.wait().await.unwrap();

    assert_eq!(status.code(), Some(gate::GATE_NOT_RELEASED_EXIT));
    assert!(!effect.exists());
}

#[tokio::test]
async fn custody_bound_shell_call_records_then_settles() {
    let root = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let scope = scope();
    let dir = root.path().join(scope.as_str());
    let (custody, _) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    let tool = ShellTool::new(sh_config(project.path()));
    tool.job_manager.bind_process_custody(custody).unwrap();

    let output = tool
        .call_with_tool_call_id(
            json!({"command": format!("ls '{}'", dir.display())}),
            Some("call-live"),
        )
        .await
        .unwrap();

    let crate::builtin::ToolOutput::JsonRenderedAsText { value, .. } = output else {
        panic!("unexpected shell output shape");
    };
    let listing = value["stdout"].as_str().unwrap();
    assert!(
        listing.contains(".json"),
        "the record must exist while the tool runs: {listing:?}"
    );
    assert!(record_files(&dir).is_empty(), "settled after containment");
}

/// Child "gateway" role for
/// [`gateway_sigkill_mid_tool_is_recovered_before_new_work`]. Inert unless the
/// parent test launches this binary with the role environment.
#[tokio::test]
#[ignore = "helper role executed only as the child gateway process"]
async fn custody_gateway_child_role() {
    let Ok(root) = std::env::var(GATEWAY_ROOT_ENV) else {
        return;
    };
    let scope = ProcessCustodyScope(std::env::var(GATEWAY_SCOPE_ENV).unwrap());
    let project = PathBuf::from(std::env::var(GATEWAY_PROJECT_ENV).unwrap());
    let fifo = std::env::var(GATEWAY_FIFO_ENV).unwrap();
    let effect = std::env::var(GATEWAY_EFFECT_ENV).unwrap();
    let (custody, _) = ProcessCustody::recover_and_open(Path::new(&root), scope)
        .await
        .unwrap();
    let tool = ShellTool::new(sh_config(&project));
    tool.job_manager.bind_process_custody(custody).unwrap();
    let command = format!("echo started > '{fifo}'; sleep 2; echo effect > '{effect}'");
    let _ = tool
        .call_with_tool_call_id(
            json!({"command": command, "timeout_secs": 60}),
            Some("call-gateway"),
        )
        .await;
}

#[tokio::test]
async fn gateway_sigkill_mid_tool_is_recovered_before_new_work() {
    let root = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let scope = scope();
    let fifo = project.path().join("started.fifo");
    let effect = project.path().join("effect");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();

    let mut gateway = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            GATEWAY_CHILD_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(GATEWAY_ROOT_ENV, root.path())
        .env(GATEWAY_SCOPE_ENV, scope.as_str())
        .env(GATEWAY_PROJECT_ENV, project.path())
        .env(GATEWAY_FIFO_ENV, &fifo)
        .env(GATEWAY_EFFECT_ENV, &effect)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // Block until the tool itself reports it is running (gate released).
    let started = {
        let fifo = fifo.clone();
        tokio::time::timeout(
            Duration::from_secs(60),
            tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
        )
        .await
    };
    let started = match started {
        Ok(joined) => joined.unwrap().unwrap(),
        Err(_) => {
            let _ = gateway.kill();
            let _ = gateway.wait();
            panic!("the gateway's shell tool never started");
        }
    };
    assert_eq!(started.trim(), "started");

    // Abrupt gateway death: its in-process containment never runs.
    gateway.kill().unwrap();
    gateway.wait().unwrap();

    let dir = root.path().join(scope.as_str());
    let records = record_files(&dir);
    assert_eq!(records.len(), 1, "exactly one tool is in custody");
    let record: CustodyRecord =
        serde_json::from_slice(&std::fs::read(&records[0]).unwrap()).unwrap();
    let CustodyPhase::Spawned { leader, .. } = record.phase else {
        panic!("a released tool must be recorded as spawned");
    };
    assert!(
        process_running(leader.pid),
        "the orphaned tool outlives its gateway without custody recovery"
    );

    // Next incarnation (this test process) opens the same scope.
    let (custody, report) = ProcessCustody::recover_and_open(root.path(), scope.clone())
        .await
        .unwrap();

    // Admission is fenced on the settlement: by the time a custody handle
    // exists, the prior tool's whole group has exited.
    assert!(!process_running(leader.pid));
    assert!(live_members(leader.pid).is_empty());
    assert_eq!(report.recovered.len(), 1);
    assert_eq!(
        report.recovered[0].tool_call_id.as_deref(),
        Some("call-gateway")
    );
    assert!(matches!(
        report.recovered[0].cessation,
        ToolProcessCessation::KilledByRecovery { members } if members >= 1
    ));
    assert!(record_files(&dir).is_empty());

    // New same-scope work runs under the fresh custody.
    let tool = ShellTool::new(sh_config(project.path()));
    tool.job_manager.bind_process_custody(custody).unwrap();
    tool.call_with_tool_call_id(json!({"command": "true"}), Some("call-next"))
        .await
        .unwrap();

    // The killed tool's delayed effect never happens: wait past its sleep.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !effect.exists(),
        "the prior incarnation's tool performed its effect after recovery"
    );
}
