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
    passed: usize,
    failed: usize,
    checks: Vec<ScriptCheck>,
}

const PARITY_CHECKS: [&str; 8] = [
    "typed client status success",
    "typed client capabilities success",
    "typed client invalid params exact json-rpc error",
    "typed client unloaded module exact json-rpc error",
    "console route helper encodes auth token",
    "module-authoring helper normalizes schema",
    "persisted events parse rust internal tags",
    "identity-first agent identity rejects invalid shape",
];

const PRODUCTIZATION_CHECKS: [&str; 12] = [
    "async client status typed result",
    "async client capabilities typed result",
    "async client reconcile typed result",
    "async client spawn_member typed result",
    "async client events subscribe typed shape",
    "async client rpc errors surface typed metadata",
    "async factory fromGatewayBin status success",
    "async factory fromGatewayBin transport errors surface",
    "async factory fromHttp status success",
    "async factory fromHttp transport errors surface",
    "console route helpers expose modules and experience routes",
    "module authoring helpers support base structures and decorators",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Resolve a program name to an absolute path using a given PATH string.
fn resolve_program(program: &str, path: &str) -> String {
    for dir in path.split(':') {
        let candidate = PathBuf::from(dir).join(program);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    program.to_string()
}

fn assert_command_success(
    language: &str,
    step: &str,
    program: &str,
    args: &[&str],
    extra_env: &[(&str, String)],
) {
    let resolved = extra_env
        .iter()
        .find(|(k, _)| *k == "PATH")
        .map(|(_, v)| resolve_program(program, v))
        .unwrap_or_else(|| program.to_string());
    let mut command = Command::new(&resolved);
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
    let resolved = extra_env
        .iter()
        .find(|(k, _)| *k == "PATH")
        .map(|(_, v)| resolve_program(program, v))
        .unwrap_or_else(|| program.to_string());
    let mut command = Command::new(&resolved);
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
    expected_checks: &[&str],
    suite: &str,
) {
    assert_eq!(summary.sdk, expected_sdk);
    assert_eq!(
        summary.passed + summary.failed,
        summary.checks.len(),
        "{} {suite} summary should satisfy passed + failed == checks.len()",
        summary.sdk
    );
    assert_eq!(
        summary.failed, 0,
        "{} {suite} checks should all pass",
        summary.sdk
    );
    assert_eq!(
        summary.checks.len(),
        expected_checks.len(),
        "{} {suite} check count should match expected cardinality",
        summary.sdk
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
        "{} {suite} checks should not contain duplicate names",
        summary.sdk
    );
    assert_eq!(
        observed_names, expected_names,
        "{} {suite} checks should match expected set exactly",
        summary.sdk
    );

    for check in &summary.checks {
        assert!(
            check.ok,
            "check {:?} should pass for {} {suite}",
            check.name, summary.sdk
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

fn select_python_for_phase_g() -> Option<String> {
    for candidate in ["python3.12", "python3.11", "python3.10", "python3"] {
        if python_meets_min_version(candidate, 3, 10) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Build a PATH string that includes Node.js bin directories.
///
/// `cargo nextest` may strip NVM/Homebrew entries from the inherited `PATH`,
/// causing bare `npm` / `node` / `tsc` lookups to fail.  This helper
/// prepends well-known Node install locations so that both the top-level
/// command *and* any subprocesses it spawns (e.g. `tsc`, `node`) can be found.
fn node_augmented_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();

    let mut extra_dirs: Vec<String> = Vec::new();

    // NVM: check NVM_DIR or the default ~/.nvm.
    let home = std::env::var("HOME").unwrap_or_default();
    let nvm_dir = std::env::var("NVM_DIR").unwrap_or_else(|_| format!("{home}/.nvm"));
    let nvm_root = PathBuf::from(&nvm_dir).join("versions/node");
    if let Ok(entries) = std::fs::read_dir(&nvm_root) {
        let mut versions: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
            .map(|e| e.path())
            .collect();
        versions.sort();
        if let Some(latest) = versions.last() {
            let bin = latest.join("bin");
            if bin.exists() {
                extra_dirs.push(bin.to_string_lossy().to_string());
            }
        }
    }

    // Homebrew (macOS).
    let brew = PathBuf::from("/opt/homebrew/bin");
    if brew.exists() {
        extra_dirs.push(brew.to_string_lossy().to_string());
    }

    if extra_dirs.is_empty() {
        return current;
    }

    extra_dirs.push(current);
    extra_dirs.join(":")
}

#[test]
#[ignore]
fn phase_g_req_g_001_req_g_002_sdk_productization_contracts() {
    // Builds in sdk/: one SDK build of this checkout at a time.
    let _sdk_build = sdk_build_lock::lock_sdk_build(&repo_root());
    let node_path = node_augmented_path();
    let path_env = [("PATH", node_path.clone())];

    assert_command_success(
        "TypeScript",
        "validation",
        "npm",
        &["--prefix", "sdk/typescript", "run", "--silent", "validate"],
        &path_env,
    );

    let ts_parity = run_script(
        "TypeScript",
        "parity",
        "node",
        &["sdk/typescript/scripts/parity.cjs"],
        &path_env,
    );
    assert_summary(&ts_parity, "typescript", &PARITY_CHECKS, "parity");

    let ts_productization = run_script(
        "TypeScript",
        "productization",
        "node",
        &["sdk/typescript/scripts/productization.cjs"],
        &path_env,
    );
    assert_summary(
        &ts_productization,
        "typescript",
        &PRODUCTIZATION_CHECKS,
        "productization",
    );

    let temp = tempfile::tempdir().expect("python phase_g venv tempdir");
    let venv_dir = temp.path().join("venv");
    let venv_dir_str = venv_dir.to_string_lossy().to_string();

    let python_program = select_python_for_phase_g()
        .expect("Python phase G verification requires python >= 3.10 in PATH");

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
        &["-m", "pip", "install", &python_sdk_str],
        &[],
    );

    let py_parity = run_script(
        "Python",
        "parity",
        &venv_python_str,
        &["sdk/python/scripts/parity.py"],
        &[],
    );
    assert_summary(&py_parity, "python", &PARITY_CHECKS, "parity");

    let py_productization = run_script(
        "Python",
        "productization",
        &venv_python_str,
        &["sdk/python/scripts/productization.py"],
        &[],
    );
    assert_summary(
        &py_productization,
        "python",
        &PRODUCTIZATION_CHECKS,
        "productization",
    );
}
