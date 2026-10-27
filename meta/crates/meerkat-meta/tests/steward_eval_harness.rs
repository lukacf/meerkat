//! End-to-end plumbing test for the Steward calibration seam
//! (docs/design/agent-memory-architecture.md §8.5/§11, P3):
//! `scripts/memory-evals --stage steward --mode mock` driving the real
//! `steward-eval` binary. Scripted replies run through the production
//! parse → shell-sanitation → staged-validator path, so the §10.2 lattice
//! law — including the quarantine-laundering reject — gates for real
//! without credentials.

#![allow(clippy::panic)]
use std::path::Path;
use std::process::Command;

#[test]
fn memory_evals_mock_mode_runs_steward_eval_end_to_end() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/memory-evals");
    if !script.is_file() {
        // Packaged crate builds have no repo scripts tree; nothing to test.
        return;
    }
    let steward_eval = env!("CARGO_BIN_EXE_steward-eval");

    let output = Command::new(&script)
        .args(["--stage", "steward", "--mode", "mock"])
        .env("MEMORY_EVALS_STEWARD_EVAL", steward_eval)
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
        "mock steward lane must exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("MOCK steward scorecard"),
        "scorecard header missing:\n{stdout}"
    );
    // The merge fixture's scripted batch must validate clean.
    assert!(
        stdout.contains("[ok       ] steward-merge-duplicates"),
        "merge fixture must pass in mock mode:\n{stdout}"
    );
    // The laundering fixture's scripted attack must be rejected by the
    // staged validator on the transitive provenance ceiling — deterministic
    // §10.2 law, gating in every mode.
    assert!(
        stdout.contains("[ok       ] steward-quarantine-laundering-rejected"),
        "laundering fixture must be validator-rejected:\n{stdout}"
    );
    assert!(
        stdout.contains("untrusted/quarantined"),
        "the reject must cite the transitive taint ceiling:\n{stdout}"
    );
    assert!(
        stdout.contains("0 deterministic failure(s)"),
        "deterministic validator law must hold:\n{stdout}"
    );
    assert!(
        stdout.contains("judgment scoring informational, exit 0"),
        "mock lane must be non-gating for judgment:\n{stdout}"
    );
}
