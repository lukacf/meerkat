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
        spawner: ToolProcessSpawner::ShellCall,
        run_id: None,
        phase,
    };
    let dir = root.join(scope.as_str());
    let path = dir.join(format!("{}.{RECORD_EXTENSION}", record.entry_id));
    let temp = temp_path(&dir, record.entry_id, record.incarnation);
    write_record_blocking(&dir, &path, &temp, &serde_json::to_vec(&record).unwrap()).unwrap();
    path
}

/// Rewrite a written record with a run id (for interrupted-run evidence).
fn set_run_id(path: &Path, run_id: meerkat_core::RunId) {
    let mut record: CustodyRecord = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    record.run_id = Some(run_id);
    std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
}

fn live_members(pgid: i32) -> Vec<i32> {
    observed_members(pgid)
        .unwrap()
        .into_iter()
        .filter(|member| member.is_running().unwrap())
        .map(|member| member.pid)
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
        ToolProcessCessation::GroupReassigned
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

/// Spawn a group whose leader exits while a background member keeps the
/// group alive. Returns the leader identity (captured while it ran), its
/// session, and the reaped leader handle.
fn leaderless_group() -> (ProcessIdentity, i32) {
    let mut leader = Command::new("/bin/sh")
        .args(["-c", "sleep 60 & read line"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = leader.id() as i32;
    let CustodyPhase::Spawned {
        leader: identity,
        session_leader,
    } = spawned_phase(pgid)
    else {
        unreachable!()
    };
    for _ in 0..500 {
        if live_members(pgid).len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(leader.stdin.take());
    leader.wait().unwrap();
    assert!(
        !process_running(pgid),
        "the leader has exited and been reaped"
    );
    assert!(
        !live_members(pgid).is_empty(),
        "the background member remains"
    );
    (identity, session_leader)
}

fn kill_test_group(pgid: i32) {
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pgid),
        nix::sys::signal::Signal::SIGKILL,
    );
}

#[tokio::test]
async fn recovery_kills_a_leaderless_prior_group_through_its_members() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let (leader, session_leader) = leaderless_group();
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

    assert!(
        matches!(
            report.recovered[0].cessation,
            ToolProcessCessation::KilledByRecovery { members } if members >= 1
        ),
        "{report:?}"
    );
    assert!(live_members(leader.pid).is_empty());
}

#[tokio::test]
async fn recovery_never_touches_a_live_group_of_the_current_incarnation() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    // Same session, member started after the recorded leader: the start and
    // session checks alone would accept this group as the prior tool's.
    let (leader, session_leader) = leaderless_group();
    register_live_group(leader.pid, Some(leader.start));
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

    let result = ProcessCustody::recover_and_open(root.path(), scope).await;
    let survivors = live_members(leader.pid);
    release_live_group(leader.pid, Some(leader.start));
    kill_test_group(leader.pid);

    let (_custody, report) = result.unwrap();
    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::GroupReassigned
    );
    assert!(
        !survivors.is_empty(),
        "a live current-incarnation group must never be signalled"
    );
}

#[tokio::test]
async fn a_stale_live_group_entry_never_shields_a_newer_orphan() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let (leader, session_leader) = leaderless_group();
    // An entry left for the same group id by an older leader (for example a
    // cancelled call whose group ended) must not protect the recorded group.
    let older = match leader.start {
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
    register_live_group(leader.pid, Some(older));
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

    let result = ProcessCustody::recover_and_open(root.path(), scope).await;
    release_live_group(leader.pid, Some(older));
    kill_test_group(leader.pid);

    let (_custody, report) = result.unwrap();
    assert!(
        matches!(
            report.recovered[0].cessation,
            ToolProcessCessation::KilledByRecovery { .. }
        ),
        "{report:?}"
    );
}

fn registered(pgid: i32) -> bool {
    live_groups().contains_key(&pgid)
}

fn await_release(pgid: i32) -> bool {
    for _ in 0..500 {
        if !registered(pgid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn tracked_groups_are_released_once_proven_exited() {
    let mut leader = spawn_group("sleep 60 & exec sleep 60");
    let pgid = leader.id() as i32;
    track_owned_process_group(pgid);
    assert!(registered(pgid));
    kill_test_group(pgid);
    leader.wait().unwrap();
    assert!(
        await_release(pgid),
        "the release watcher must retire the entry after the group exits"
    );
}

#[tokio::test]
async fn a_cancelled_spawned_reservation_is_released_once_its_group_exits() {
    let root = TempDir::new().unwrap();
    let (custody, _) = ProcessCustody::recover_and_open(root.path(), scope())
        .await
        .unwrap();
    let mut leader = spawn_group("exec sleep 60");
    let pgid = leader.id() as i32;
    let mut reservation = custody
        .reserve(ToolProcessSpawner::ShellCall, Some("call-cancelled"), None)
        .await
        .unwrap();
    reservation.record_spawned(pgid).await.unwrap();
    assert!(registered(pgid));
    // The call future is cancelled after spawn: the reservation is dropped
    // while the group is still alive.
    drop(reservation);
    assert!(registered(pgid), "a live group stays registered");
    kill_test_group(pgid);
    leader.wait().unwrap();
    assert!(await_release(pgid));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn another_users_process_holding_a_recorded_pid_is_classified_not_errored() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    // pid 1 exists in every pid namespace. Run as a normal user it belongs to
    // root and probes as Foreign; run as root it is Observed, and a bumped
    // stamp makes it another process. Either way it is never ours.
    let own = ProcessIdentity::capture(std::process::id() as i32)
        .unwrap()
        .unwrap();
    let recorded_start = match sys::probe(1).unwrap() {
        ProcessProbe::Observed(init) => init.start,
        ProcessProbe::Absent | ProcessProbe::Foreign => own.start,
    };
    let foreign = ProcessIdentity {
        pid: 1,
        start: match recorded_start {
            ProcessStartStamp::LinuxBoot {
                boot_id,
                start_ticks,
            } => ProcessStartStamp::LinuxBoot {
                boot_id,
                start_ticks: start_ticks.wrapping_add(1),
            },
            other => other,
        },
    };
    assert!(!foreign.is_running().unwrap());
    write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        foreign,
        CustodyPhase::Spawned {
            leader: foreign,
            session_leader: 1,
        },
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::GroupReassigned
    );
}

#[test]
fn a_record_removed_between_listing_and_reading_is_already_settled() {
    let root = TempDir::new().unwrap();
    let missing = root.path().join(format!("{}.json", Uuid::new_v4()));
    assert!(matches!(
        read_listed_record(&missing).unwrap(),
        ListedRecord::Gone
    ));
}

#[tokio::test]
async fn recovery_deletes_only_earlier_incarnations_temp_files() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let dir = root.path().join(scope.as_str());
    std::fs::create_dir_all(&dir).unwrap();
    let current = super::incarnation().unwrap().id;
    let ended = Uuid::new_v4();
    // A settled record proves `ended`'s host is gone.
    write_prior_record(
        root.path(),
        &scope,
        ended,
        dead_identity(),
        CustodyPhase::Reserved,
    );
    let live_write = temp_path(&dir, Uuid::new_v4(), current);
    let unknown_host_write = temp_path(&dir, Uuid::new_v4(), Uuid::new_v4());
    let interrupted = temp_path(&dir, Uuid::new_v4(), ended);
    for temp in [&live_write, &unknown_host_write, &interrupted] {
        std::fs::write(temp, b"{}").unwrap();
    }

    ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert!(
        live_write.exists(),
        "a current-incarnation write may be in flight"
    );
    assert!(
        unknown_host_write.exists(),
        "a host not proven gone (for example a concurrent host) may be writing"
    );
    assert!(!interrupted.exists());
}

#[tokio::test]
async fn unknown_record_version_fails_closed_unless_its_environment_ended() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let dir = root.path().join(scope.as_str());
    std::fs::create_dir_all(&dir).unwrap();
    let current = super::incarnation().unwrap().environment;
    let entry = Uuid::new_v4();
    let path = dir.join(format!("{entry}.json"));
    let newer = |environment: HostEnvironment| {
        serde_json::json!({
            "version": 99,
            "entry_id": entry,
            "incarnation": Uuid::new_v4(),
            "environment": environment,
            "phase": "something_new",
        })
    };
    std::fs::write(&path, serde_json::to_vec(&newer(current)).unwrap()).unwrap();

    let error = ProcessCustody::recover_and_open(root.path(), scope.clone())
        .await
        .expect_err("an uninterpretable same-environment record cannot prove cessation");
    assert!(
        matches!(
            error,
            ProcessCustodyError::UnsupportedRecordVersion { version: 99, .. }
        ),
        "{error:?}"
    );

    let ended = match current {
        HostEnvironment::Linux {
            pid_namespace_dev,
            pid_namespace_ino,
            ..
        } => HostEnvironment::Linux {
            boot_id: Uuid::new_v4(),
            pid_namespace_dev,
            pid_namespace_ino,
        },
        HostEnvironment::Darwin { .. } => HostEnvironment::Darwin {
            boot_session: Some(Uuid::new_v4()),
        },
    };
    std::fs::write(&path, serde_json::to_vec(&newer(ended)).unwrap()).unwrap();
    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::PriorEnvironmentEnded
    );
    assert!(!path.exists());
}

#[test]
fn darwin_records_without_boot_identity_parse_and_are_unknown_not_ended() {
    let legacy: HostEnvironment = serde_json::from_str(r#"{"kind":"darwin"}"#).unwrap();
    assert_eq!(legacy, HostEnvironment::Darwin { boot_session: None });
    let current = HostEnvironment::Darwin {
        boot_session: Some(Uuid::new_v4()),
    };
    assert_eq!(legacy.relation_to(&current), EnvironmentRelation::Unknown);
    let other_boot = HostEnvironment::Darwin {
        boot_session: Some(Uuid::new_v4()),
    };
    assert_eq!(other_boot.relation_to(&current), EnvironmentRelation::Ended);
    assert_eq!(current.relation_to(&current), EnvironmentRelation::Same);
}

#[tokio::test]
async fn recovery_removes_an_emptied_scope_directory() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let dir = root.path().join(scope.as_str());
    write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        CustodyPhase::Reserved,
    );

    let (custody, _) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    assert!(!dir.exists());

    // A later reservation recreates it.
    let reservation = custody
        .reserve(ToolProcessSpawner::ShellCall, Some("call"), None)
        .await
        .unwrap();
    assert_eq!(record_files(&dir).len(), 1);
    // Outside a runtime the unreleased reservation is removed inline.
    std::thread::spawn(move || drop(reservation))
        .join()
        .unwrap();
    assert!(record_files(&dir).is_empty());
}

#[tokio::test]
async fn a_killed_tool_of_an_in_flight_run_is_kept_as_interrupted_run_evidence() {
    use meerkat_core::tool_process::{InterruptedToolEvidence, InterruptedToolSettlement};

    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut leader = spawn_group("exec sleep 60");
    let pgid = leader.id() as i32;
    let run_id = meerkat_core::RunId::new();
    let path = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        spawned_phase(pgid),
    );
    set_run_id(&path, run_id.clone());
    // A never-started reservation of the same run has no possible effect.
    let never_started = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        CustodyPhase::Reserved,
    );
    set_run_id(&never_started, run_id.clone());

    let (custody, report) = ProcessCustody::recover_and_open(root.path(), scope.clone())
        .await
        .unwrap();
    leader.wait().unwrap();

    assert_eq!(report.recovered.len(), 2);
    assert!(path.exists(), "the killed tool's record stays as evidence");
    assert!(
        !never_started.exists(),
        "a never-started entry is not evidence"
    );
    let calls = custody.interrupted_calls().await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].run_id, run_id);
    assert_eq!(calls[0].tool_call_id.as_deref(), Some("call-prior"));
    assert!(matches!(
        calls[0].cessation,
        ToolProcessCessation::KilledByRecovery { .. }
    ));
    assert_eq!(calls[0].settlement, InterruptedToolSettlement::Pending);

    // Evidence is never re-settled by a later recovery.
    let (_again, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    assert!(report.recovered.is_empty());

    let settled = meerkat_core::tool_process::InterruptedRunInputs {
        inputs: vec![
            meerkat_core::tool_process::InterruptedRunInput {
                kind: meerkat_core::tool_process::InterruptedInputKind::Prompt,
                request: Some(meerkat_core::tool_process::InterruptedRequest {
                    content: meerkat_core::types::ContentInput::from("run the tool"),
                    created_at: meerkat_core::types::message_timestamp_now(),
                    identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                    render_metadata: None,
                }),
            },
            meerkat_core::tool_process::InterruptedRunInput {
                kind: meerkat_core::tool_process::InterruptedInputKind::Peer,
                request: None,
            },
        ],
    };
    custody
        .mark_inputs_settled(&[calls[0].entry_id], &settled)
        .await
        .unwrap();
    assert_eq!(
        custody.interrupted_calls().await.unwrap()[0].settlement,
        InterruptedToolSettlement::InputsSettled(settled)
    );
    custody.acknowledge(&[calls[0].entry_id]).await.unwrap();
    assert!(custody.interrupted_calls().await.unwrap().is_empty());
    assert!(!path.exists());
}

#[tokio::test]
async fn realm_sweep_settles_every_scope_and_leaves_live_hosts_alone() {
    let root = TempDir::new().unwrap();
    let orphaned = scope();
    let served = scope();
    let mut orphan_leader = spawn_group("exec sleep 60");
    let orphan_pgid = orphan_leader.id() as i32;
    write_prior_record(
        root.path(),
        &orphaned,
        Uuid::new_v4(),
        dead_identity(),
        spawned_phase(orphan_pgid),
    );
    let mut host = Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .spawn()
        .unwrap();
    let host_identity = ProcessIdentity::capture(host.id() as i32).unwrap().unwrap();
    let mut served_leader = spawn_group("exec sleep 60");
    let served_pgid = served_leader.id() as i32;
    write_prior_record(
        root.path(),
        &served,
        Uuid::new_v4(),
        host_identity,
        spawned_phase(served_pgid),
    );

    let report = ProcessCustody::sweep(root.path()).await.unwrap();

    let outcome = |scope: &ProcessCustodyScope| {
        report
            .scopes
            .iter()
            .find(|swept| swept.scope == scope.as_str())
            .map(|swept| &swept.outcome)
            .unwrap()
    };
    assert!(matches!(
        outcome(&orphaned),
        Ok(settled) if matches!(
            settled.recovered[0].cessation,
            ToolProcessCessation::KilledByRecovery { .. }
        )
    ));
    assert!(matches!(
        outcome(&served),
        Err(ProcessCustodyError::PriorIncarnationAlive { .. })
    ));
    assert!(live_members(orphan_pgid).is_empty(), "the orphan is killed");
    assert!(
        process_running(served_pgid),
        "a live host's tool is left alone"
    );
    orphan_leader.wait().unwrap();
    for child in [&mut served_leader, &mut host] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
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
            None,
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
            None,
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
    tool.call_with_tool_call_id(json!({"command": "true"}), Some("call-next"), None)
        .await
        .unwrap();

    // The killed tool's delayed effect never happens: wait past its sleep.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !effect.exists(),
        "the prior incarnation's tool performed its effect after recovery"
    );
}

/// The environment of a host in another pid namespace of this boot, if this
/// host records its namespace identity.
fn foreign_namespace_environment() -> Option<HostEnvironment> {
    match super::incarnation().unwrap().environment {
        HostEnvironment::Linux {
            boot_id,
            pid_namespace_dev: Some(dev),
            pid_namespace_ino: Some(ino),
        } => Some(HostEnvironment::Linux {
            boot_id,
            pid_namespace_dev: Some(dev),
            pid_namespace_ino: Some(ino.wrapping_add(1)),
        }),
        _ => None,
    }
}

/// Write the incarnation lock file of a foreign host (unlocked).
fn write_incarnation_lock(root: &Path, incarnation: Uuid) -> PathBuf {
    let path = incarnation_lock_path(root, incarnation);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"").unwrap();
    path
}

#[tokio::test]
async fn a_foreign_namespace_record_without_an_incarnation_lock_fails_closed() {
    let Some(foreign) = foreign_namespace_environment() else {
        return;
    };
    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut leader = spawn_group("exec sleep 60");
    let pgid = leader.id() as i32;
    let path = write_prior_record_in(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        foreign,
        spawned_phase(pgid),
    );

    let error = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .expect_err("a record written before incarnation locks cannot be verified");

    assert!(
        matches!(
            error,
            ProcessCustodyError::ForeignPidNamespace {
                liveness: ForeignIncarnationLiveness::Unverifiable,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(path.exists(), "the record is left for the operator");
    assert!(
        process_running(pgid),
        "a local process must not be signalled"
    );
    leader.kill().unwrap();
    leader.wait().unwrap();
}

#[tokio::test]
async fn a_foreign_namespace_record_whose_host_holds_its_lock_fails_closed() {
    let Some(foreign) = foreign_namespace_environment() else {
        return;
    };
    let root = TempDir::new().unwrap();
    let scope = scope();
    let incarnation = Uuid::new_v4();
    let lock = write_incarnation_lock(root.path(), incarnation);
    // The live sibling host's lock.
    let _held = try_incarnation_lock(&lock).unwrap().expect("lock free");
    let path = write_prior_record_in(
        root.path(),
        &scope,
        incarnation,
        dead_identity(),
        foreign,
        CustodyPhase::Reserved,
    );

    let error = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .expect_err("a live foreign host owns the record");

    assert!(
        matches!(
            error,
            ProcessCustodyError::ForeignPidNamespace {
                liveness: ForeignIncarnationLiveness::Running,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(path.exists(), "a live host's record is left alone");
    assert!(lock.exists());
}

#[tokio::test]
async fn a_foreign_namespace_record_whose_lock_was_released_is_settled() {
    let Some(foreign) = foreign_namespace_environment() else {
        return;
    };
    let root = TempDir::new().unwrap();
    let scope = scope();
    let incarnation = Uuid::new_v4();
    // The owner died: its lock file remains, released by the kernel.
    write_incarnation_lock(root.path(), incarnation);
    let mut leader = spawn_group("exec sleep 60");
    let pgid = leader.id() as i32;
    let path = write_prior_record_in(
        root.path(),
        &scope,
        incarnation,
        dead_identity(),
        foreign,
        spawned_phase(pgid),
    );

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::ForeignIncarnationEnded
    );
    assert!(
        !path.exists(),
        "a record without a run is settled and removed"
    );
    assert!(
        process_running(pgid),
        "pids of another namespace are never signalled"
    );
    leader.kill().unwrap();
    leader.wait().unwrap();
}

#[tokio::test]
async fn recovery_holds_this_incarnations_lock_for_the_process_lifetime() {
    let root = TempDir::new().unwrap();
    let (custody, _) = ProcessCustody::recover_and_open(root.path(), scope())
        .await
        .unwrap();
    drop(custody);
    let lock = incarnation_lock_path(root.path(), super::incarnation().unwrap().id);
    assert!(lock.exists());
    assert!(
        try_incarnation_lock(&lock).unwrap().is_none(),
        "this host's incarnation lock stays held"
    );
}

#[tokio::test]
async fn the_sweep_reaps_only_ended_unreferenced_incarnation_locks() {
    let root = TempDir::new().unwrap();
    let current = super::incarnation().unwrap();
    // Held by this process, as every custody-opening host does.
    let (_custody, _) = ProcessCustody::recover_and_open(root.path(), scope())
        .await
        .unwrap();
    let ended = write_incarnation_lock(root.path(), Uuid::new_v4());
    let live = write_incarnation_lock(root.path(), Uuid::new_v4());
    let _live_held = try_incarnation_lock(&live).unwrap().expect("lock free");
    // An ended incarnation still named by a record that cannot be settled
    // (an unknown format in this environment) keeps its lock.
    let referenced_owner = Uuid::new_v4();
    let referenced = write_incarnation_lock(root.path(), referenced_owner);
    let stuck_scope = scope();
    let stuck_dir = root.path().join(stuck_scope.as_str());
    std::fs::create_dir_all(&stuck_dir).unwrap();
    let entry = Uuid::new_v4();
    std::fs::write(
        stuck_dir.join(format!("{entry}.json")),
        serde_json::to_vec(&serde_json::json!({
            "version": 99,
            "entry_id": entry,
            "incarnation": referenced_owner,
            "environment": current.environment,
        }))
        .unwrap(),
    )
    .unwrap();

    ProcessCustody::sweep(root.path()).await.unwrap();

    assert!(!ended.exists(), "an ended, unreferenced lock is reaped");
    assert!(live.exists(), "a held lock is kept");
    assert!(
        referenced.exists(),
        "a lock an unsettled record needs is kept"
    );
    assert!(incarnation_lock_path(root.path(), current.id).exists());
}

const UNSHARE_ROOT_ENV: &str = "MEERKAT_TEST_CUSTODY_UNSHARE_ROOT";
const UNSHARE_SCOPE_ENV: &str = "MEERKAT_TEST_CUSTODY_UNSHARE_SCOPE";
const UNSHARE_CHILD_TEST: &str =
    "builtin::shell::custody::tests::custody_foreign_namespace_host_role";

/// Host role run inside a new pid namespace by
/// [`a_host_in_another_pid_namespace_is_proven_ended_by_its_lock`]: open
/// custody (taking the incarnation lock), run a tool in custody, report
/// ready, and wait to be killed with its namespace.
#[tokio::test]
#[ignore = "helper role executed only inside a new pid namespace"]
async fn custody_foreign_namespace_host_role() {
    let (Some(root), Some(scope)) = (
        std::env::var_os(UNSHARE_ROOT_ENV).map(PathBuf::from),
        std::env::var(UNSHARE_SCOPE_ENV).ok(),
    ) else {
        return;
    };
    let (custody, _) = ProcessCustody::recover_and_open(&root, ProcessCustodyScope(scope))
        .await
        .unwrap();
    let (prepared, mut command) = custody
        .prepare_spawn(
            ToolProcessSpawner::ShellCall,
            Some("call-foreign"),
            None,
            OsStr::new("/bin/sh"),
            &[OsString::from("-c"), OsString::from("exec sleep 600")],
        )
        .await
        .unwrap();
    let child = command.spawn().unwrap();
    let _guard = prepared.spawned(&child).await.unwrap();
    std::fs::write(root.join("ready.fifo"), "ready\n").unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn a_host_in_another_pid_namespace_is_proven_ended_by_its_lock() {
    // Unprivileged user and pid namespaces may be unavailable (for example
    // restricted by AppArmor); the test then has nothing to exercise.
    let unshare = |args: &[&str]| Command::new("unshare").args(args).output();
    match unshare(&["-Urpf", "--kill-child", "true"]) {
        Ok(output) if output.status.success() => {}
        _ => return,
    }
    let root = TempDir::new().unwrap();
    let scope = scope();
    let status = Command::new("mkfifo")
        .arg(root.path().join("ready.fifo"))
        .status()
        .unwrap();
    assert!(status.success());
    let mut host = Command::new("unshare")
        .args(["-Urpf", "--kill-child"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            UNSHARE_CHILD_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(UNSHARE_ROOT_ENV, root.path())
        .env(UNSHARE_SCOPE_ENV, scope.as_str())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let fifo = root.path().join("ready.fifo");
    let ready = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
    )
    .await;
    let ready = match ready {
        Ok(joined) => joined.unwrap().unwrap(),
        Err(_) => {
            let _ = host.kill();
            let _ = host.wait();
            panic!("the foreign-namespace host never became ready");
        }
    };
    assert_eq!(ready.trim(), "ready");

    // Alive in its own namespace: its lock is held.
    let error = ProcessCustody::recover_and_open(root.path(), scope.clone())
        .await
        .expect_err("a live foreign host owns its records");
    assert!(
        matches!(
            error,
            ProcessCustodyError::ForeignPidNamespace {
                liveness: ForeignIncarnationLiveness::Running,
                ..
            }
        ),
        "{error:?}"
    );

    // Tear the namespace down with its host.
    host.kill().unwrap();
    host.wait().unwrap();
    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    assert_eq!(report.recovered.len(), 1);
    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::ForeignIncarnationEnded
    );
}

#[tokio::test]
async fn a_live_hosts_record_is_never_rewritten_even_when_its_group_is_gone() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let mut host = Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .spawn()
        .unwrap();
    let host_identity = ProcessIdentity::capture(host.id() as i32).unwrap().unwrap();
    // The recorded group has already exited: only the live host may settle
    // (or delete) its record.
    let mut finished = spawn_group("exit 0");
    let phase = spawned_phase(finished.id() as i32);
    finished.wait().unwrap();
    let path = write_prior_record(root.path(), &scope, Uuid::new_v4(), host_identity, phase);
    let before = std::fs::read(&path).unwrap();

    let report = ProcessCustody::sweep(root.path()).await.unwrap();

    assert!(matches!(
        report.scopes[0].outcome,
        Err(ProcessCustodyError::PriorIncarnationAlive { .. })
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a live host's record is never rewritten"
    );
    host.kill().unwrap();
    host.wait().unwrap();
}

#[test]
fn the_scope_settlement_lock_excludes_every_other_open_file() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("scope");
    assert!(
        lock_scope_dir(&dir).unwrap().is_none(),
        "a missing scope holds no records"
    );
    std::fs::create_dir_all(&dir).unwrap();
    let held = lock_scope_dir(&dir).unwrap().expect("scope locked");
    // flock(2) excludes every other open file description, whichever process
    // holds it.
    let lock_file = dir.join(SCOPE_LOCK_FILE);
    assert!(
        try_incarnation_lock(&lock_file).unwrap().is_none(),
        "a second holder acquired the scope lock"
    );
    drop(held);
    assert!(try_incarnation_lock(&lock_file).unwrap().is_some());
}

#[test]
fn a_scope_replaced_while_waiting_is_locked_afresh() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("scope");
    std::fs::create_dir_all(&dir).unwrap();
    let lock_file = dir.join(SCOPE_LOCK_FILE);
    // A waiter's lock on the lock file as it was when it started waiting.
    let stale = lock_scope_dir(&dir).unwrap().expect("scope locked");
    // Meanwhile the scope was removed and recreated.
    std::fs::remove_file(&lock_file).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    assert!(
        !scope_lock_is_current(&lock_file, &stale).unwrap_or(false),
        "a lock on the replaced scope's file does not hold the current scope"
    );
    drop(stale);
    let fresh = lock_scope_dir(&dir).unwrap().expect("current scope locked");
    assert!(scope_lock_is_current(&lock_file, &fresh).unwrap());
    // The removed scope is gone for a waiter that finds no directory.
    drop(fresh);
    remove_scope_dir_if_empty(&dir);
    assert!(!dir.exists());
    assert!(lock_scope_dir(&dir).unwrap().is_none());
}

#[tokio::test]
async fn an_exit_marker_from_another_pid_namespace_settles_without_a_lock() {
    let Some(foreign) = foreign_namespace_environment() else {
        return;
    };
    let root = TempDir::new().unwrap();
    let scope = scope();
    // No incarnation lock (written before locks): the marker guards no
    // process, so nothing needs observing.
    let path = write_prior_record_in(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        foreign,
        CustodyPhase::Exited,
    );
    set_run_id(&path, meerkat_core::RunId::new());

    let (_custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::ExitedBeforeCommit
    );
}

#[tokio::test]
async fn a_tool_that_exits_inside_a_run_keeps_a_marker_until_the_run_commits() {
    use meerkat_core::tool_process::InterruptedToolEvidence;

    let root = TempDir::new().unwrap();
    let scope = scope();
    let dir = root.path().join(scope.as_str());
    let (custody, _) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    let spawn_and_finish = |run_id: meerkat_core::RunId| {
        let custody = Arc::clone(&custody);
        async move {
            let (prepared, mut command) = custody
                .prepare_spawn(
                    ToolProcessSpawner::ShellCall,
                    Some("call-done"),
                    Some(&run_id),
                    OsStr::new("/bin/sh"),
                    &[OsString::from("-c"), OsString::from("exit 0")],
                )
                .await
                .unwrap();
            command.kill_on_drop(true);
            let mut child = command.spawn().unwrap();
            let guard = prepared.spawned(&child).await.unwrap();
            child.wait().await.unwrap();
            guard
        }
    };

    // Exits before its run commits: a marker stays.
    let run = meerkat_core::RunId::new();
    spawn_and_finish(run.clone()).await.settle().await;
    let markers = record_files(&dir);
    assert_eq!(markers.len(), 1, "the finished tool leaves a marker");
    let marker: CustodyRecord =
        serde_json::from_slice(&std::fs::read(&markers[0]).unwrap()).unwrap();
    assert!(matches!(marker.phase, CustodyPhase::Exited));
    assert_eq!(marker.run_id.as_ref(), Some(&run));
    // Markers of this incarnation are not interrupted-run evidence.
    assert!(custody.interrupted_calls().await.unwrap().is_empty());
    custody.run_ended(&run).await.unwrap();
    assert!(record_files(&dir).is_empty(), "the commit drops the marker");

    // Its run ends (commits, fails or is cancelled) while it is still live:
    // no marker is left behind.
    let later = meerkat_core::RunId::new();
    let guard = spawn_and_finish(later.clone()).await;
    custody.run_ended(&later).await.unwrap();
    guard.settle().await;
    assert!(record_files(&dir).is_empty());

    // A retained group (a hook whose members may outlive it) that exits
    // after its run ended leaves no marker either.
    let failed = meerkat_core::RunId::new();
    let guard = spawn_and_finish(failed.clone()).await;
    custody.run_ended(&failed).await.unwrap();
    guard.settle_when_exited();
    let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !record_files(&dir).is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "the ended run's retained record is removed"
    );
}

#[tokio::test]
async fn a_prior_incarnations_exit_marker_becomes_interrupted_run_evidence() {
    use meerkat_core::tool_process::InterruptedToolEvidence;

    let root = TempDir::new().unwrap();
    let scope = scope();
    let run_id = meerkat_core::RunId::new();
    let path = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        CustodyPhase::Exited,
    );
    set_run_id(&path, run_id.clone());

    let (custody, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();

    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::ExitedBeforeCommit
    );
    let calls = custody.interrupted_calls().await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].run_id, run_id);
    assert_eq!(calls[0].cessation, ToolProcessCessation::ExitedBeforeCommit);
}

#[tokio::test]
async fn reopening_an_open_scope_reuses_its_handle_without_recovering_again() {
    let root = TempDir::new().unwrap();
    let scope = scope();
    let (first, _) = ProcessCustody::recover_and_open(root.path(), scope.clone())
        .await
        .unwrap();
    let path = write_prior_record(
        root.path(),
        &scope,
        Uuid::new_v4(),
        dead_identity(),
        CustodyPhase::Reserved,
    );

    let (again, report) = ProcessCustody::recover_and_open(root.path(), scope.clone())
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&first, &again), "one handle per open scope");
    assert!(report.recovered.is_empty());
    assert!(path.exists());

    drop((first, again));
    let (_reopened, report) = ProcessCustody::recover_and_open(root.path(), scope)
        .await
        .unwrap();
    assert_eq!(
        report.recovered[0].cessation,
        ToolProcessCessation::NeverStarted
    );
}
