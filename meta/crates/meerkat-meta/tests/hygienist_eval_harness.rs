//! End-to-end plumbing test for the Hygienist calibration seam
//! (docs/design/agent-memory-architecture.md §8.6/§11, P4):
//! `scripts/memory-evals --stage hygienist --mode mock` driving the real
//! `hygienist-eval` binary. Scripted replies run through the production
//! parse → §8.6 validator → replacement-construction path, so the
//! quarantine hard-block, role law, and the §8.4 ordering invariant gate
//! for real without credentials.

#![allow(clippy::panic)]
use std::path::Path;
use std::process::Command;

#[test]
fn memory_evals_mock_mode_runs_hygienist_eval_end_to_end() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/memory-evals");
    if !script.is_file() {
        // Packaged crate builds have no repo scripts tree; nothing to test.
        return;
    }
    let hygienist_eval = env!("CARGO_BIN_EXE_hygienist-eval");

    let output = Command::new(&script)
        .args(["--stage", "hygienist", "--mode", "mock"])
        .env("MEMORY_EVALS_HYGIENIST_EVAL", hygienist_eval)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // No python3 on this host; the harness itself is exercised in CI.
            return;
        }
        Err(err) => panic!("failed to run scripts/memory-evals: {err}"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mock hygienist lane must exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("MOCK hygienist scorecard"),
        "scorecard header missing:\n{stdout}"
    );
    // The quarantine fixture's scripted revision touches a range referenced
    // by quarantined-record evidence — the §8.6 hard-block must reject it
    // deterministically in every mode.
    assert!(
        stdout.contains("[ok       ] quarantine-referenced-range-blocked"),
        "quarantine hard-block fixture must be validator-rejected:\n{stdout}"
    );
    // The ordering fixture runs before the §8.4 harvest has sequenced —
    // the ordering invariant must block it deterministically.
    assert!(
        stdout.contains("[ok       ] ordering-invariant-unmet-blocked"),
        "ordering-invariant fixture must be blocked:\n{stdout}"
    );
    assert!(
        stdout.contains("0 deterministic failure(s)"),
        "deterministic §8.6 validator law must hold:\n{stdout}"
    );
    assert!(
        stdout.contains("judgment scoring informational, exit 0"),
        "mock lane must be non-gating for judgment:\n{stdout}"
    );
}
