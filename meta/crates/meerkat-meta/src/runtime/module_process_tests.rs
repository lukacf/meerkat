//! Terminate paths that the public forced-failure option does not reach:
//! the replacement child of an aborted respawn, and shutdown. Each check runs
//! once, right after the call that releases the process.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::module_process::test_seam;
use super::*;
use crate::types::DiscoverySpec;

fn shell_module(id: &str, script: String) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script],
        restart_policy: RestartPolicy::Never,
    }
}

/// Appends its pid before emitting a ready event, then keeps running.
fn long_lived(id: &str, pid_file: &Path) -> ModuleConfig {
    shell_module(
        id,
        format!(
            r#"echo $$ >> '{}'; printf '%s\n' '{{"event_id":"evt-{id}","source":"module","timestamp_ms":1,"event":{{"kind":"module","module":"{id}","event_type":"ready","payload":{{}}}}}}'; exec sleep 30"#,
            pid_file.display()
        ),
    )
}

fn config(module: ModuleConfig) -> MetaConfig {
    MetaConfig {
        discovery: DiscoverySpec {
            namespace: "module-process".to_string(),
            modules: vec![module.id.clone()],
        },
        modules: vec![module],
        pre_spawn: vec![],
    }
}

fn pids(pid_file: &Path) -> Vec<u32> {
    std::fs::read_to_string(pid_file)
        .unwrap()
        .lines()
        .map(|line| line.trim().parse().unwrap())
        .collect()
}

fn running_or_unreaped(pid: u32) -> bool {
    Command::new("sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .unwrap()
        .success()
}

/// Step retained cleanup until its owners confirm every reap; bounded.
fn progress_until_settled(runtime: &mut MetaRuntimeHandle) -> ModuleCleanupObservation {
    let mut reaped = Vec::new();
    for _ in 0..100 {
        let mut observation = runtime.progress_module_cleanup();
        reaped.append(&mut observation.reaped);
        if observation.phase == ModuleCleanupPhase::Settled {
            observation.reaped = reaped;
            return observation;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("retained module cleanup never settled");
}

#[test]
fn an_aborted_respawn_retains_its_replacement_when_terminating_it_fails() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pids");
    let mut runtime = start_meta_runtime_with_options(
        config(long_lived("respawn", &pid_file)),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions::default(),
    )
    .expect("runtime starts");
    let error = {
        let _failing = test_seam::fail_terminations();
        runtime
            .spawn_member("respawn", Duration::from_secs(5))
            .expect_err("the existing child cannot be terminated")
    };
    assert!(format!("{error:?}").contains("failed to terminate replacement child"));
    let started = pids(&pid_file);
    assert_eq!(started.len(), 2, "original and replacement both started");
    // The unconfirmed replacement is still owned and reported, never settled.
    let observation = runtime.module_cleanup_observation();
    assert_eq!(observation.phase, ModuleCleanupPhase::Failed);
    assert_eq!(observation.outstanding.len(), 1);
    assert_eq!(observation.outstanding[0].process_id, started[1]);
    assert!(!observation.outstanding[0].child_reaped);
    assert!(observation.outstanding[0].failure.is_some());
    assert!(
        running_or_unreaped(started[0]),
        "existing child stays owned"
    );
    let settled = progress_until_settled(&mut runtime);
    assert_eq!(settled.reaped.len(), 1);
    assert_eq!(settled.reaped[0].process_id, started[1]);
    assert!(
        !running_or_unreaped(started[1]),
        "replacement {} reaped by its retained owner",
        started[1]
    );
    let report = runtime.shutdown();
    assert_eq!(report.orphan_processes, 0);
    assert_eq!(report.physical_cleanup.phase, ModuleCleanupPhase::Settled);
    assert_eq!(report.physical_cleanup.reaped[0].process_id, started[0]);
    assert!(!running_or_unreaped(started[0]));
}

#[test]
fn shutdown_retains_a_child_whose_termination_fails_until_its_reap_is_confirmed() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pids");
    let mut runtime = start_meta_runtime_with_options(
        config(long_lived("live", &pid_file)),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions::default(),
    )
    .expect("runtime starts");
    let pid = pids(&pid_file)[0];
    let report = {
        let _failing = test_seam::fail_terminations();
        runtime.shutdown()
    };
    assert_eq!(
        report.orphan_processes, 1,
        "the failed termination is reported"
    );
    // A failed termination is retained custody, not settlement.
    assert_eq!(report.physical_cleanup.phase, ModuleCleanupPhase::Failed);
    assert!(report.physical_cleanup.reaped.is_empty());
    assert_eq!(report.physical_cleanup.outstanding[0].process_id, pid);
    assert_eq!(
        runtime.module_cleanup_observation().phase,
        ModuleCleanupPhase::Failed
    );
    let settled = progress_until_settled(&mut runtime);
    assert_eq!(settled.reaped.len(), 1);
    assert_eq!(settled.reaped[0].process_id, pid);
    assert!(
        !running_or_unreaped(pid),
        "module {pid} reaped by its owner"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn release_wait_errors_are_unconfirmed_unless_the_platform_reports_no_child() {
    use super::module_process::{ReleaseWaitFailure, classify_release_wait_error};
    let no_child = std::io::Error::from_raw_os_error(rustix::io::Errno::CHILD.raw_os_error());
    assert_eq!(
        classify_release_wait_error(&no_child),
        ReleaseWaitFailure::NoWaitableChild
    );
    for other in [
        std::io::Error::from_raw_os_error(rustix::io::Errno::INVAL.raw_os_error()),
        std::io::Error::from_raw_os_error(rustix::io::Errno::INTR.raw_os_error()),
        std::io::Error::other("wait failed"),
    ] {
        assert_eq!(
            classify_release_wait_error(&other),
            ReleaseWaitFailure::Unconfirmed,
            "{other}"
        );
    }
}
