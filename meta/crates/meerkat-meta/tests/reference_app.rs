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
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

#[path = "support/sdk_build_lock.rs"]
mod sdk_build_lock;

#[derive(Debug, Deserialize)]
struct ScriptCheck {
    name: String,
    ok: bool,
}

#[derive(Debug, Deserialize)]
struct ScriptSummary {
    sdk: String,
    suite: String,
    passed: usize,
    failed: usize,
    checks: Vec<ScriptCheck>,
}

const H2_REFERENCE_CHECKS: [&str; 10] = [
    "reference app boots and responds",
    "health route reports gateway binding",
    "status route matches json-rpc contract",
    "capabilities route matches json-rpc contract",
    "reconcile route matches json-rpc contract",
    "spawn_member route matches json-rpc contract",
    "events subscribe route matches json-rpc contract",
    "spawn_member invalid params matches rust parity error",
    "events subscribe agent validation matches rust parity error",
    "reference flow route executes end-to-end",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn assert_command_success(
    language: &str,
    step: &str,
    program: &str,
    args: &[&str],
    extra_env: &[(&str, String)],
) {
    let mut command = Command::new(program);
    command.current_dir(repo_root()).args(args);

    for (key, value) in extra_env {
        command.env(key, value);
    }

    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn {language} {step}: {err}"));

    assert!(
        output.status.success(),
        "{language} {step} failed\nstatus: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_script(
    language: &str,
    suite: &str,
    program: &str,
    args: &[&str],
    extra_env: &[(&str, String)],
) -> ScriptSummary {
    let mut command = Command::new(program);
    command
        .current_dir(repo_root())
        .args(args)
        .env("META_RPC_GATEWAY_BIN", env!("CARGO_BIN_EXE_rpc_gateway"));

    for (key, value) in extra_env {
        command.env(key, value);
    }

    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn {language} {suite} script: {err}"));

    assert!(
        output.status.success(),
        "{language} {suite} script failed\nstatus: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice::<ScriptSummary>(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "{language} {suite} output was not valid summary JSON: {err}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_summary(
    summary: &ScriptSummary,
    expected_sdk: &str,
    expected_suite: &str,
    expected_checks: &[&str],
) {
    assert_eq!(summary.sdk, expected_sdk);
    assert_eq!(summary.suite, expected_suite);
    assert_eq!(
        summary.passed + summary.failed,
        summary.checks.len(),
        "{} {} summary should satisfy passed + failed == checks.len()",
        summary.sdk,
        summary.suite
    );
    assert_eq!(
        summary.failed, 0,
        "{} {} checks should all pass",
        summary.sdk, summary.suite
    );
    assert_eq!(
        summary.checks.len(),
        expected_checks.len(),
        "{} {} check count should match expected cardinality",
        summary.sdk,
        summary.suite
    );

    let observed_names: BTreeSet<&str> = summary
        .checks
        .iter()
        .map(|check| check.name.as_str())
        .collect();
    let expected_names: BTreeSet<&str> = expected_checks.iter().copied().collect();

    assert_eq!(
        observed_names.len(),
        summary.checks.len(),
        "{} {} checks should not contain duplicate names",
        summary.sdk,
        summary.suite
    );
    assert_eq!(
        observed_names, expected_names,
        "{} {} checks should match expected set exactly",
        summary.sdk, summary.suite
    );

    for check in &summary.checks {
        assert!(
            check.ok,
            "check {:?} should pass for {} {}",
            check.name, summary.sdk, summary.suite
        );
    }
}

fn python_meets_min_version(program: &str, min_major: u32, min_minor: u32) -> bool {
    let output = Command::new(program).arg("--version").output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let joined = format!("{stdout} {stderr}");
    let mut numbers = joined
        .split_whitespace()
        .find(|token| token.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .unwrap_or_default()
        .split('.');
    let major = numbers
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or_default();
    let minor = numbers
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or_default();
    (major, minor) >= (min_major, min_minor)
}

fn select_python_for_phase_h2() -> Option<String> {
    for candidate in ["python3.12", "python3.11", "python3.10", "python3"] {
        if python_meets_min_version(candidate, 3, 10) {
            return Some(candidate.to_string());
        }
    }
    None
}

#[test]
#[ignore] // requires Python venv + cargo build (~15s)
fn phase_h2_req_h2_001_req_h2_002_python_rpc_reference_app_parity_contracts() {
    // Builds in sdk/: one SDK build of this checkout at a time.
    let _sdk_build = sdk_build_lock::lock_sdk_build(&repo_root());
    let temp = tempfile::tempdir().expect("python phase_h2 venv tempdir");
    let venv_dir = temp.path().join("venv");
    let venv_dir_str = venv_dir.to_string_lossy().to_string();

    let python_program = select_python_for_phase_h2()
        .expect("Python phase H2 verification requires python >= 3.10 in PATH");

    assert_command_success(
        "Python",
        "venv create",
        &python_program,
        &["-m", "venv", &venv_dir_str],
        &[],
    );

    let venv_python = venv_dir.join("bin/python");
    let venv_python_str = venv_python.to_string_lossy().to_string();
    let python_sdk = repo_root().join("sdk/python");
    let python_sdk_str = python_sdk.to_string_lossy().to_string();

    assert_command_success(
        "Python",
        "package install",
        &venv_python_str,
        &[
            "-m",
            "pip",
            "install",
            "fastapi",
            "httpx",
            "uvicorn",
            &python_sdk_str,
        ],
        &[],
    );

    let summary = run_script(
        "Python",
        "h2_reference_flow",
        &venv_python_str,
        &["sdk/python/scripts/h2_reference_flow.py"],
        &[],
    );
    assert_summary(
        &summary,
        "python",
        "h2_reference_flow",
        &H2_REFERENCE_CHECKS,
    );
}
