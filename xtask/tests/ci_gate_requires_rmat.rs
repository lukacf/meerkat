#![allow(clippy::expect_used, clippy::panic)]

//! CI-gate self-test (wave-b B-10, Risk #6 mitigation).
//!
//! Guards against the silent-regression failure mode where
//! `rmat-audit` / `seam-inventory` / `audit-generated-headers` /
//! `machine-codegen-drift` / `wasm-contract-tests` are runnable locally
//! via `make ci` but NOT required by the GitHub-side `gate` job. In
//! that shape a merge can land with a failing governance lane and
//! nobody notices until the wave-b invariants break in production.
//!
//! This test parses `.github/workflows/ci.yml`, inspects
//! `jobs.gate.needs`, and fails if any of the load-bearing lanes is
//! missing. If wave-b ever drops one of these lanes by accident, this
//! test fires at merge time and the invariant holds.
//!
//! The expected set is intentionally duplicated here rather than
//! derived from a shared constant — this test is the enforcement point
//! for the GitHub-side contract; coupling it to a shared constant
//! would let both ends of the contract drift together.

use std::path::PathBuf;

/// Every governance lane that wave-b requires to block merges. Adding
/// to this list expands the gate; removing requires a reviewer to
/// decide whether the invariant being dropped is tolerable.
const REQUIRED_GOVERNANCE_LANES: &[&str] = &[
    "fmt-lint",
    "machine-authority",
    "test-unit",
    "integration-fast",
    "e2e-fast",
    "test-minimal",
    "test-feature-matrix-lib",
    "test-feature-matrix-surface-checks",
    "wasm-check",
    "wasm-contract-tests",
    "rmat-audit",
    "seam-inventory",
    "audit",
];

fn ci_yml_path() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at `xtask/` at test time.
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push(".github/workflows/ci.yml");
    path
}

#[test]
fn gate_job_requires_every_governance_lane() {
    let ci_yml = ci_yml_path();
    let text = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", ci_yml.display()));
    let doc: serde_yaml::Value = serde_yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse {} as YAML: {e}", ci_yml.display()));

    let gate = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("gate"))
        .unwrap_or_else(|| panic!("{} has no jobs.gate", ci_yml.display()));

    let needs = gate.get("needs").unwrap_or_else(|| {
        panic!("{} has no jobs.gate.needs", ci_yml.display());
    });

    let declared: Vec<String> = match needs {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        serde_yaml::Value::String(single) => vec![single.clone()],
        other => panic!(
            "{} jobs.gate.needs has unexpected shape: {other:?}",
            ci_yml.display()
        ),
    };

    let mut missing: Vec<&str> = Vec::new();
    for required in REQUIRED_GOVERNANCE_LANES {
        if !declared.iter().any(|d| d == required) {
            missing.push(required);
        }
    }

    assert!(
        missing.is_empty(),
        "jobs.gate.needs in {} is missing required governance lane(s): {missing:?}. Declared: {declared:?}. Add them to the gate.needs array in .github/workflows/ci.yml.",
        ci_yml.display(),
    );
}

/// Cross-check: every lane named in gate.needs must also correspond to
/// a defined job in the workflow. A needs entry with no corresponding
/// job silently succeeds (skipped) on the GitHub side — another flavour
/// of silent-regression failure.
#[test]
fn gate_needs_only_references_defined_jobs() {
    let ci_yml = ci_yml_path();
    let text = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", ci_yml.display()));
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse ci.yml");
    let jobs = doc
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .expect("jobs mapping");
    let defined: Vec<String> = jobs
        .keys()
        .filter_map(|k| k.as_str().map(str::to_string))
        .collect();

    let needs = jobs
        .get("gate")
        .and_then(|g| g.get("needs"))
        .expect("gate.needs");
    let declared: Vec<String> = match needs {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => panic!("gate.needs must be a sequence"),
    };

    let mut dangling: Vec<String> = Vec::new();
    for entry in &declared {
        if !defined.iter().any(|d| d == entry) {
            dangling.push(entry.clone());
        }
    }

    assert!(
        dangling.is_empty(),
        "jobs.gate.needs references undefined jobs: {dangling:?}. Defined jobs: {defined:?}.",
    );
}

/// Cross-check: the `gate` job's in-body results loop (the `for job in ...`
/// iteration that reports per-job outcomes) must enumerate the same set
/// of lanes as `needs`. A lane in `needs` but not in the loop causes its
/// failure to be invisible in the gate step's log even though the gate
/// still fails; a lane in the loop but not in `needs` references an
/// `always()` context that never receives the dependency's result.
///
/// We match by looking for `<job>) result="${{ needs.<job>.result }}"` in
/// the script body — the shape enforced by the existing gate step.
#[test]
fn gate_results_loop_enumerates_every_needed_lane() {
    let ci_yml = ci_yml_path();
    let text = std::fs::read_to_string(&ci_yml).expect("read ci.yml");
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse ci.yml");
    let gate = doc
        .get("jobs")
        .and_then(|j| j.get("gate"))
        .expect("jobs.gate");
    let needs = gate.get("needs").expect("gate.needs");
    let declared: Vec<String> = match needs {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => panic!("gate.needs must be a sequence"),
    };

    // The `run:` step body is one YAML scalar string. Find it by walking
    // steps and grabbing the first multi-line `run`.
    let run_body = gate
        .get("steps")
        .and_then(|s| s.as_sequence())
        .and_then(|seq| seq.iter().find_map(|step| step.get("run")?.as_str()))
        .expect("gate job must have a run step")
        .to_string();

    let mut missing_from_loop: Vec<&String> = Vec::new();
    for lane in &declared {
        // The canonical shape in the existing gate body is
        //   <lane>) result="${{ needs.<lane>.result }}" ;;
        let case_fragment = format!("{lane}) result=\"");
        let needs_ref = format!("needs.{lane}.result");
        if !run_body.contains(&case_fragment) || !run_body.contains(&needs_ref) {
            missing_from_loop.push(lane);
        }
    }

    assert!(
        missing_from_loop.is_empty(),
        "jobs.gate.needs lanes not referenced in the gate results loop: {missing_from_loop:?}. Each lane must appear once as a case arm (`<lane>) result=\"${{{{ needs.<lane>.result }}}}\"`) so its result is reported."
    );
}
