#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use meerkat_meta::{
    DiscoverySpec, LifecycleStage, MetaConfig, ModuleCleanupPhase, ModuleConfig, ModuleHealthState,
    ProcessBoundaryError, RestartPolicy, RuntimeBoundaryError, RuntimeMutationError,
    RuntimeOptions, UnifiedEvent, start_meta_runtime_with_options,
};

fn shell_module(id: &str, script: &str, restart_policy: RestartPolicy) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy,
    }
}

fn forced_crash_then_ready_script(state_file: &Path, success_attempt: u32) -> String {
    let template = r#"attempt_file='__STATE_FILE__'; attempt=0; if [ -f "$attempt_file" ]; then attempt=$(cat "$attempt_file"); fi; attempt=$((attempt + 1)); echo "$attempt" > "$attempt_file"; if [ "$attempt" -lt __SUCCESS_ATTEMPT__ ]; then exit 1; fi; printf '%s\n' "{\"event_id\":\"evt-forced-crash\",\"source\":\"module\",\"timestamp_ms\":42,\"event\":{\"kind\":\"module\",\"module\":\"forced-crash\",\"event_type\":\"ready\",\"payload\":{\"attempt\":$attempt,\"pid\":$$}}}"; exec sleep 30"#;
    template
        .replace("__STATE_FILE__", &state_file.display().to_string())
        .replace("__SUCCESS_ATTEMPT__", &success_attempt.to_string())
}

fn forced_crash_config(script: &str) -> MetaConfig {
    MetaConfig {
        modules: vec![shell_module(
            "forced-crash",
            script,
            RestartPolicy::OnFailure,
        )],
        discovery: DiscoverySpec {
            namespace: "phase-d".to_string(),
            modules: vec!["forced-crash".to_string()],
        },
        pre_spawn: vec![],
    }
}

fn forced_crash_mutation_config(forced_script: &str, failing_script: &str) -> MetaConfig {
    MetaConfig {
        modules: vec![
            shell_module("forced-crash", forced_script, RestartPolicy::OnFailure),
            shell_module("always-fail", failing_script, RestartPolicy::OnFailure),
        ],
        discovery: DiscoverySpec {
            namespace: "phase-d-mutation".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    }
}

/// Each attempt records its pid at `<state file>.pid<attempt>` before its
/// ready line, so an attempt whose ready event is withheld is still found.
fn respawn_probe_script(state_file: &Path) -> String {
    let template = r#"attempt_file='__STATE_FILE__'; attempt=0; if [ -f "$attempt_file" ]; then attempt=$(cat "$attempt_file"); fi; attempt=$((attempt + 1)); echo "$attempt" > "$attempt_file"; echo "$$" > "$attempt_file.pid$attempt"; printf '{"event_id":"evt-respawn-%s","source":"module","timestamp_ms":52,"event":{"kind":"module","module":"respawn-probe","event_type":"ready","payload":{"attempt":%s,"pid":%s}}}\n' "$attempt" "$attempt" "$$"; exec sleep 30"#;
    template.replace("__STATE_FILE__", &state_file.display().to_string())
}

fn respawn_probe_pid(state_file: &Path, attempt: u32) -> i64 {
    let pid_file = PathBuf::from(format!("{}.pid{attempt}", state_file.display()));
    std::fs::read_to_string(&pid_file)
        .unwrap_or_else(|err| panic!("attempt {attempt} recorded no pid: {err}"))
        .trim()
        .parse()
        .expect("pid")
}

/// `kill -0` also succeeds for an unreaped zombie child of this test process,
/// so false means terminated and reaped.
fn running_or_unreaped(pid: i64) -> bool {
    Command::new("sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .expect("run kill -0")
        .success()
}

/// True only while `pid` has not exited: `ps` reports it in a state other
/// than `Z`, so an exited child that is still unreaped does not count.
fn running_not_exited(pid: i64) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let state = stdout.trim();
    output.status.success() && !state.is_empty() && !state.starts_with('Z')
}

fn respawn_probe_config(script: &str) -> MetaConfig {
    MetaConfig {
        modules: vec![shell_module("respawn-probe", script, RestartPolicy::Never)],
        discovery: DiscoverySpec {
            namespace: "phase-d-respawn".to_string(),
            modules: vec!["respawn-probe".to_string()],
        },
        pre_spawn: vec![],
    }
}

fn single_mutation_module_config(module_id: &str, script: &str) -> MetaConfig {
    MetaConfig {
        modules: vec![shell_module(module_id, script, RestartPolicy::Never)],
        discovery: DiscoverySpec {
            namespace: "phase-d-boundary".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    }
}

#[test]
fn phase_d_req_d_001_forced_crash_restart_policy_and_backoff_are_enforced() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_file = temp.path().join("forced-crash-attempts.txt");
    let script = forced_crash_then_ready_script(&state_file, 3);

    let started = Instant::now();
    let mut runtime = start_meta_runtime_with_options(
        forced_crash_config(&script),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            on_failure_retry_budget: 2,
            supervisor_restart_backoff_ms: 120,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts after forced crash retries");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(220),
        "expected at least two restart backoffs (~240ms total), got {elapsed:?}"
    );

    let transitions = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "forced-crash")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        transitions,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Healthy,
        ]
    );

    let merged_events = runtime.merged_events();
    let module_payload = merged_events
        .iter()
        .find_map(|event| match &event.event {
            UnifiedEvent::Module(module_event) if module_event.module == "forced-crash" => {
                Some(module_event.payload.clone())
            }
            _ => None,
        })
        .expect("forced-crash ready event should be present");
    assert_eq!(module_payload["attempt"], 3);

    let pid = module_payload["pid"]
        .as_i64()
        .expect("module payload includes pid");

    let shutdown = runtime.shutdown();
    assert_eq!(shutdown.orphan_processes, 0);
    assert_eq!(
        shutdown.terminated_modules,
        vec!["forced-crash".to_string()]
    );

    let kill_status = Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        .status()
        .expect("run kill -0");
    assert!(
        !kill_status.success(),
        "module process {pid} is still alive after shutdown"
    );
}

#[test]
fn phase_d_req_d_002_shutdown_lifecycle_is_clean_after_forced_crash_recovery() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_file = temp.path().join("forced-crash-attempts.txt");
    let script = forced_crash_then_ready_script(&state_file, 3);
    let mut runtime = start_meta_runtime_with_options(
        forced_crash_config(&script),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            on_failure_retry_budget: 2,
            supervisor_restart_backoff_ms: 25,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts after retries");

    let shutdown = runtime.shutdown();
    assert_eq!(shutdown.orphan_processes, 0);
    assert_eq!(
        shutdown.terminated_modules,
        vec!["forced-crash".to_string()]
    );
    assert!(!runtime.is_running());
    assert!(runtime.loaded_modules().is_empty());

    let stages = runtime
        .lifecycle_events()
        .iter()
        .map(|event| event.stage.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        stages,
        vec![
            LifecycleStage::MobStarted,
            LifecycleStage::ModulesStarted,
            LifecycleStage::MergedStreamStarted,
            LifecycleStage::ShutdownRequested,
            LifecycleStage::ShutdownComplete,
        ]
    );
}

#[test]
fn phase_d_mutation_spawn_member_uses_supervisor_retry_backoff_and_transitions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_file = temp.path().join("forced-crash-mutation-attempts.txt");
    let forced_script = forced_crash_then_ready_script(&state_file, 3);
    let mut runtime = start_meta_runtime_with_options(
        forced_crash_mutation_config(&forced_script, "exit 1"),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            on_failure_retry_budget: 2,
            supervisor_restart_backoff_ms: 110,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts with no discovered modules");

    let started = Instant::now();
    runtime
        .spawn_member("forced-crash", Duration::from_secs(1))
        .expect("spawn member succeeds after retries");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(180),
        "spawn_member should include two restart backoffs, got {elapsed:?}"
    );

    let transitions = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "forced-crash")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        transitions,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Healthy,
        ]
    );

    let merged_events = runtime.merged_events();
    let payload = merged_events
        .iter()
        .find_map(|event| match &event.event {
            UnifiedEvent::Module(module_event) if module_event.module == "forced-crash" => {
                Some(module_event.payload.clone())
            }
            _ => None,
        })
        .expect("forced-crash ready event");
    assert_eq!(payload["attempt"], 3);

    let pid = payload["pid"].as_i64().expect("pid in payload");
    let shutdown = runtime.shutdown();
    assert_eq!(shutdown.orphan_processes, 0);
    let kill_status = Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        .status()
        .expect("run kill -0");
    assert!(!kill_status.success(), "module process {pid} still alive");
}

#[test]
fn phase_d_mutation_spawn_member_failure_surfaces_error_warning_and_transitions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_file = temp.path().join("forced-crash-unused.txt");
    let forced_script = forced_crash_then_ready_script(&state_file, 3);
    let mut runtime = start_meta_runtime_with_options(
        forced_crash_mutation_config(&forced_script, "exit 1"),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            on_failure_retry_budget: 1,
            supervisor_restart_backoff_ms: 70,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts");

    let started = Instant::now();
    let error = runtime
        .spawn_member("always-fail", Duration::from_secs(1))
        .expect_err("spawn_member should fail after retry budget exhaustion");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(50),
        "spawn_member failure should include one restart backoff, got {elapsed:?}"
    );
    assert!(
        matches!(error, RuntimeMutationError::Runtime(_)),
        "expected runtime error, got {error:?}"
    );
    assert!(
        !runtime
            .loaded_modules()
            .contains(&"always-fail".to_string())
    );

    let transitions = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "always-fail")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        transitions,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Failed,
            ModuleHealthState::Stopped,
        ]
    );

    let warning_present = runtime.merged_events().iter().any(|event| {
        matches!(
            &event.event,
            UnifiedEvent::Module(module_event)
                if module_event.module == "always-fail"
                    && module_event.event_type == "supervisor.warning"
        )
    });
    assert!(
        warning_present,
        "supervisor.warning event should be emitted"
    );
    runtime.shutdown();
}

#[test]
fn phase_d_mutation_reconcile_uses_supervisor_and_propagates_partial_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_file = temp.path().join("forced-crash-reconcile-attempts.txt");
    let forced_script = forced_crash_then_ready_script(&state_file, 2);
    let mut runtime = start_meta_runtime_with_options(
        forced_crash_mutation_config(&forced_script, "exit 1"),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            on_failure_retry_budget: 1,
            supervisor_restart_backoff_ms: 45,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts");

    let started = Instant::now();
    let error = runtime
        .reconcile_modules(
            vec!["forced-crash".to_string(), "always-fail".to_string()],
            Duration::from_secs(1),
        )
        .expect_err("reconcile should fail when one module exhausts retry budget");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(80),
        "reconcile should include supervisor backoff delays, got {elapsed:?}"
    );
    assert!(
        matches!(error, RuntimeMutationError::Runtime(_)),
        "expected runtime error, got {error:?}"
    );

    let loaded = runtime.loaded_modules();
    assert!(loaded.contains(&"forced-crash".to_string()));
    assert!(!loaded.contains(&"always-fail".to_string()));

    let forced_crash_transitions = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "forced-crash")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        forced_crash_transitions,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Healthy,
        ]
    );

    let fail_warning_present = runtime.merged_events().iter().any(|event| {
        matches!(
            &event.event,
            UnifiedEvent::Module(module_event)
                if module_event.module == "always-fail"
                    && module_event.event_type == "supervisor.warning"
        )
    });
    assert!(fail_warning_present);

    let forced_payload = runtime
        .merged_events()
        .iter()
        .find_map(|event| match &event.event {
            UnifiedEvent::Module(module_event) if module_event.module == "forced-crash" => {
                Some(module_event.payload.clone())
            }
            _ => None,
        })
        .expect("forced-crash ready payload present");
    let pid = forced_payload["pid"].as_i64().expect("pid");

    let shutdown = runtime.shutdown();
    assert_eq!(shutdown.orphan_processes, 0);
    let kill_status = Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        .status()
        .expect("run kill -0");
    assert!(!kill_status.success(), "module process {pid} still alive");
}

#[test]
fn phase_d_respawn_terminate_failure_keeps_existing_child_tracked_and_aborts_swap() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_file = temp.path().join("respawn-attempts.txt");
    let script = respawn_probe_script(&state_file);
    let mut runtime = start_meta_runtime_with_options(
        respawn_probe_config(&script),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            supervisor_test_force_terminate_failure: true,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts");

    let initial_payload = runtime
        .merged_events()
        .iter()
        .find_map(|event| match &event.event {
            UnifiedEvent::Module(module_event) if module_event.module == "respawn-probe" => {
                Some(module_event.payload.clone())
            }
            _ => None,
        })
        .expect("initial respawn-probe payload");
    assert_eq!(initial_payload["attempt"], 1);
    let initial_pid = initial_payload["pid"].as_i64().expect("initial pid");
    assert_eq!(respawn_probe_pid(&state_file, 1), initial_pid);

    let error = runtime
        .spawn_member("respawn-probe", Duration::from_secs(1))
        .expect_err("respawn should fail when existing child termination fails");
    let message = match error {
        RuntimeMutationError::Runtime(RuntimeBoundaryError::Process(ProcessBoundaryError::Io(
            message,
        ))) => message,
        other => panic!("unexpected error shape: {other:?}"),
    };
    assert!(message.contains("failed to terminate existing child before respawn"));

    // Attempt 2 started; the refused swap keeps the original child running
    // and terminates and reaps the replacement.
    assert_eq!(
        std::fs::read_to_string(&state_file)
            .expect("attempt file")
            .trim(),
        "2",
        "the replacement attempt started"
    );
    let replacement_pid = respawn_probe_pid(&state_file, 2);
    assert_ne!(replacement_pid, initial_pid);
    assert!(
        running_not_exited(initial_pid),
        "the original child {initial_pid} keeps running after the refused swap"
    );
    assert!(
        !running_or_unreaped(replacement_pid),
        "the replacement {replacement_pid} is terminated and reaped when the swap is refused"
    );

    let merged_events = runtime.merged_events();
    assert!(
        !merged_events
            .iter()
            .any(|event| event.event_id == "evt-respawn-2"),
        "replacement event should not be inserted when swap is aborted"
    );

    let shutdown = runtime.shutdown();
    assert_eq!(shutdown.orphan_processes, 0);
    assert_eq!(
        shutdown.terminated_modules,
        vec!["respawn-probe".to_string()]
    );
    assert_eq!(
        shutdown.physical_cleanup.phase,
        ModuleCleanupPhase::Settled,
        "{:?}",
        shutdown.physical_cleanup
    );
    assert!(
        !running_or_unreaped(initial_pid),
        "existing child {initial_pid} should still be tracked and terminated on shutdown"
    );
}

#[test]
fn phase_d_boundary_normalize_cleanup_terminate_failure_is_propagated() {
    let mut runtime = start_meta_runtime_with_options(
        single_mutation_module_config("bad-normalize", r#"printf '%s\n' 'not-json'"#),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            supervisor_test_force_terminate_failure: true,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts");

    let error = runtime
        .spawn_member("bad-normalize", Duration::from_secs(1))
        .expect_err("invalid boundary output should fail");
    let message = match error {
        RuntimeMutationError::Runtime(RuntimeBoundaryError::Process(ProcessBoundaryError::Io(
            message,
        ))) => message,
        other => panic!("unexpected error shape: {other:?}"),
    };
    assert!(message.contains("cleanup terminate failed after normalize error"));

    let warning_present = runtime.merged_events().iter().any(|event| {
        matches!(
            &event.event,
            UnifiedEvent::Module(module_event)
                if module_event.module == "bad-normalize"
                    && module_event.event_type == "supervisor.warning"
        )
    });
    assert!(warning_present);
    runtime.shutdown();
}

#[test]
fn phase_d_boundary_timeout_cleanup_terminate_failure_is_propagated() {
    let mut runtime = start_meta_runtime_with_options(
        single_mutation_module_config("bad-timeout", "sleep 0.08"),
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            supervisor_test_force_terminate_failure: true,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts");

    let error = runtime
        .spawn_member("bad-timeout", Duration::from_millis(5))
        .expect_err("timeout path should fail");
    let message = match error {
        RuntimeMutationError::Runtime(RuntimeBoundaryError::Process(ProcessBoundaryError::Io(
            message,
        ))) => message,
        other => panic!("unexpected error shape: {other:?}"),
    };
    assert!(message.contains("cleanup terminate failed after timeout"));

    // The forced cleanup failure is reported; the dropped process owner still
    // kills and reaps the module (see tests/module_startup_cleanup.rs).
    thread::sleep(Duration::from_millis(120));
    runtime.shutdown();
}
