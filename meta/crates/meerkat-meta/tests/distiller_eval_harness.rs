//! End-to-end plumbing test for the Distiller calibration seam
//! (docs/design/agent-memory-architecture.md §8.4/§11, P2):
//! `scripts/memory-evals --stage distiller --mode mock` driving the real
//! `distiller-eval` binary. The mock extracts nothing (the doctrine's
//! preferred output), so noop fixtures score ok, extraction fixtures are
//! informational MOCK-MISSes, and the deterministic quarantine verdicts —
//! which come from the real `TaintLlmWriteGate` law inside the binary —
//! gate for real.

#![allow(clippy::panic)]
use std::path::Path;
use std::process::Command;

#[test]
fn memory_evals_mock_mode_runs_distiller_eval_end_to_end() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/memory-evals");
    if !script.is_file() {
        // Packaged crate builds have no repo scripts tree; nothing to test.
        return;
    }
    let distiller_eval = env!("CARGO_BIN_EXE_distiller-eval");

    let output = Command::new(&script)
        .args(["--stage", "distiller", "--mode", "mock"])
        .env("MEMORY_EVALS_DISTILLER_EVAL", distiller_eval)
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
        "mock distiller lane must exit 0\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("MOCK distiller scorecard"),
        "scorecard header missing:\n{stdout}"
    );
    // The noop fixture must score ok even under the extract-nothing mock.
    assert!(
        stdout.contains("[ok       ] distiller-noop-chitchat"),
        "noop fixture must pass in mock mode:\n{stdout}"
    );
    // The tainted fixture's quarantine verdict is deterministic gate law and
    // must hold in mock mode too.
    assert!(
        stdout.contains("distiller-tainted-evidence-quarantine"),
        "tainted fixture did not run:\n{stdout}"
    );
    assert!(
        stdout.contains("0 deterministic failure(s)"),
        "deterministic quarantine verdicts must hold:\n{stdout}"
    );
    assert!(
        stdout.contains("extraction scoring informational, exit 0"),
        "mock lane must be non-gating for extraction:\n{stdout}"
    );
}
