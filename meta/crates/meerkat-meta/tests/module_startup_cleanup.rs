//! Module subprocesses are reaped on every path that stops owning them.
//!
//! Each check runs once, right after the call that releases the process:
//! termination and reaping complete before that call returns. `kill -0` still
//! succeeds for an unreaped zombie child of this test process, so "gone" means
//! terminated and reaped, not merely signalled. Handshakes with module scripts
//! use blocking FIFO reads, not polling.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use meerkat_meta::{
    DiscoverySpec, LocalJsonMemoryBackendConfig, MemoryBackendConfig, MetaConfig, ModuleConfig,
    ProcessBoundaryError, RestartPolicy, RuntimeBoundaryError, RuntimeMutationError,
    RuntimeOptions, start_meta_runtime_with_options,
};

fn shell_module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn config(modules: Vec<ModuleConfig>, discovered: &[&str]) -> MetaConfig {
    MetaConfig {
        modules,
        discovery: DiscoverySpec {
            namespace: "startup-cleanup".to_string(),
            modules: discovered.iter().map(ToString::to_string).collect(),
        },
        pre_spawn: vec![],
    }
}

fn ready_line(id: &str) -> String {
    format!(
        r#"{{"event_id":"evt-{id}","source":"module","timestamp_ms":1,"event":{{"kind":"module","module":"{id}","event_type":"ready","payload":{{}}}}}}"#
    )
}

/// Records its pid before emitting a valid ready event, then keeps running.
/// The pid is on disk before the runtime can observe the ready line.
fn long_lived_script(id: &str, pid_file: &Path) -> String {
    format!(
        "echo $$ >> '{}'; printf '%s\\n' '{}'; exec sleep 30",
        pid_file.display(),
        ready_line(id)
    )
}

fn pids(pid_file: &Path) -> Vec<u32> {
    std::fs::read_to_string(pid_file)
        .expect("module recorded its pid before the observed event")
        .lines()
        .map(|line| line.trim().parse().expect("pid"))
        .collect()
}

fn running_or_unreaped(pid: u32) -> bool {
    Command::new("sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .expect("run kill -0")
        .success()
}

fn fifo(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let status = Command::new("mkfifo").arg(&path).status().expect("mkfifo");
    assert!(status.success());
    path
}

/// Blocks until the module writes one line into the FIFO.
fn read_fifo_line(path: PathBuf) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        std::fs::File::open(path)
            .expect("open fifo")
            .read_to_string(&mut text)
            .expect("read fifo");
        text.trim().to_string()
    })
}

/// Bootstrap builds the memory backend before it starts any module process,
/// so a malformed memory state refuses the runtime with no module started.
#[test]
fn failed_bootstrap_starts_no_module_before_its_memory_backend() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let state = temp.path().join("memory.json");
    std::fs::write(&state, b"{ not json").unwrap();
    let error = start_meta_runtime_with_options(
        config(
            vec![shell_module(
                "early",
                &long_lived_script("early", &pid_file),
            )],
            &["early"],
        ),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions {
            memory_backend: Some(MemoryBackendConfig::LocalJson(
                LocalJsonMemoryBackendConfig {
                    state_path: state.display().to_string(),
                    health_check_endpoint: None,
                },
            )),
            ..RuntimeOptions::default()
        },
    )
    .expect_err("malformed memory state fails bootstrap");
    assert!(format!("{error:?}").contains("MemoryBackend"), "{error:?}");
    assert!(
        !pid_file.exists(),
        "no module process starts before the memory backend is built"
    );
}

#[test]
fn a_module_that_closes_stdout_without_exiting_does_not_block_start() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let script = format!(
        "echo $$ > '{}'; exec 1>&-; exec sleep 30",
        pid_file.display()
    );
    let mut runtime = start_meta_runtime_with_options(
        config(vec![shell_module("silent", &script)], &[]),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions::default(),
    )
    .expect("runtime starts without discovered modules");
    let started = Instant::now();
    let error = runtime
        .spawn_member("silent", Duration::from_secs(5))
        .expect_err("closed output is not a ready event");
    // The module sleeps 30s; waiting for its exit would exceed the 5s start
    // timeout by far.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "start waited for the module to exit: {:?}",
        started.elapsed()
    );
    assert!(matches!(
        error,
        RuntimeMutationError::Runtime(RuntimeBoundaryError::Process(
            ProcessBoundaryError::EmptyOutput
        ))
    ));
    let pid = pids(&pid_file)[0];
    assert!(
        !running_or_unreaped(pid),
        "module {pid} survived its failed start"
    );
    assert_eq!(runtime.shutdown().orphan_processes, 0);
}

/// A start whose cleanup kill fails is not released: the runtime keeps the
/// process in its retained cleanup and reports it outstanding, and its later
/// progress reaps it.
#[test]
fn a_failed_cleanup_after_start_timeout_retains_then_reaps_the_module() {
    let temp = tempfile::tempdir().unwrap();
    let pid_fifo = fifo(temp.path(), "pid");
    let script = format!("echo $$ > '{}'; exec sleep 30", pid_fifo.display());
    let mut runtime = start_meta_runtime_with_options(
        config(vec![shell_module("slow", &script)], &[]),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions {
            supervisor_test_force_terminate_failure: true,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts without discovered modules");
    let pid = read_fifo_line(pid_fifo);
    let error = runtime
        .spawn_member("slow", Duration::from_millis(500))
        .expect_err("no ready event before the timeout");
    assert!(
        format!("{error:?}").contains("cleanup terminate failed after timeout"),
        "{error:?}"
    );
    let pid: u32 = pid.join().unwrap().parse().unwrap();
    assert_eq!(
        runtime.module_cleanup_observation().outstanding.len(),
        1,
        "the unconfirmed process stays owned"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observation = runtime.progress_module_cleanup();
    while observation.phase != meerkat_meta::ModuleCleanupPhase::Settled
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
        observation = runtime.progress_module_cleanup();
    }
    assert_eq!(
        observation.phase,
        meerkat_meta::ModuleCleanupPhase::Settled,
        "{observation:?}"
    );
    assert!(
        !running_or_unreaped(pid),
        "module {pid} leaked after its retained cleanup settled"
    );
    assert_eq!(runtime.shutdown().orphan_processes, 0);
}

/// After a failed start the runtime must not keep the module's stdout open:
/// a descendant that inherited it then gets EPIPE. A reader thread left
/// blocked on the pipe would keep it open instead.
///
/// The probe writes from a subshell of its own. Some shells (macOS /bin/sh)
/// keep a builtin printf's output buffered after the write failed, and flush
/// it into the next redirection; the subshell takes that buffer with it, so
/// the outcome FIFO only ever receives `open` or `closed`.
#[test]
fn a_failed_start_leaves_no_reader_holding_the_module_output() {
    let temp = tempfile::tempdir().unwrap();
    let go = fifo(temp.path(), "go");
    let outcome = fifo(temp.path(), "outcome");
    let script = format!(
        "(trap '' PIPE; read _ < '{go}'; if (printf 'late\\n') 2>/dev/null; then echo open > '{outcome}'; else echo closed > '{outcome}'; fi) & exec sleep 30",
        go = go.display(),
        outcome = outcome.display()
    );
    let mut runtime = start_meta_runtime_with_options(
        config(vec![shell_module("holder", &script)], &[]),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions::default(),
    )
    .expect("runtime starts without discovered modules");
    let outcome = read_fifo_line(outcome);
    let error = runtime
        .spawn_member("holder", Duration::from_millis(500))
        .expect_err("no ready event before the timeout");
    assert!(matches!(
        error,
        RuntimeMutationError::Runtime(RuntimeBoundaryError::Process(
            ProcessBoundaryError::Timeout { .. }
        ))
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .open(&go)
        .and_then(|mut fifo| fifo.write_all(b"go\n"))
        .expect("signal descendant");
    assert_eq!(outcome.join().unwrap(), "closed");
    runtime.shutdown();
}

#[test]
fn dropping_the_runtime_handle_reaps_its_live_modules() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let runtime = start_meta_runtime_with_options(
        config(
            vec![shell_module("live", &long_lived_script("live", &pid_file))],
            &["live"],
        ),
        vec![],
        Duration::from_secs(5),
        RuntimeOptions::default(),
    )
    .expect("runtime starts");
    let pid = pids(&pid_file)[0];
    assert!(running_or_unreaped(pid));
    drop(runtime);
    assert!(
        !running_or_unreaped(pid),
        "module {pid} survived the dropped handle"
    );
}
