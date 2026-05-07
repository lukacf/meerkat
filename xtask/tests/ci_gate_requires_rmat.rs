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

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn workflow_yml_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push(".github/workflows");
    path.push(name);
    path
}

fn repo_path(path: &str) -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.push(path);
    root
}

fn read_workflow(path: &Path) -> serde_yaml::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse {} as YAML: {e}", path.display()))
}

fn assert_machine_authority_detector(paths: &[&str], should_match: bool) {
    let script = repo_path("scripts/machine-authority-changed");
    let output = Command::new(&script)
        .args(paths)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", script.display()));
    assert_eq!(
        output.status.success(),
        should_match,
        "machine-authority detector mismatch for {paths:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn gate_needs(doc: &serde_yaml::Value, path: &Path) -> Vec<String> {
    let gate = doc
        .get("jobs")
        .and_then(|jobs| jobs.get("gate"))
        .unwrap_or_else(|| panic!("{} has no jobs.gate", path.display()));

    let needs = gate.get("needs").unwrap_or_else(|| {
        panic!("{} has no jobs.gate.needs", path.display());
    });

    match needs {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        serde_yaml::Value::String(single) => vec![single.clone()],
        other => panic!(
            "{} jobs.gate.needs has unexpected shape: {other:?}",
            path.display()
        ),
    }
}

#[test]
fn gate_job_requires_every_governance_lane() {
    let ci_yml = ci_yml_path();
    let ci_doc = read_workflow(&ci_yml);
    let wrapper_needs = gate_needs(&ci_doc, &ci_yml);
    assert_eq!(
        wrapper_needs,
        vec!["bazel".to_string(), "dogma-cleanup-gate".to_string()],
        "{} jobs.gate.needs must gate the GCP Bazel reusable workflow and the dogma cleanup review boundary",
        ci_yml.display(),
    );

    for workflow in ["cargo.yml", "buildbuddy.yml"] {
        let workflow_path = workflow_yml_path(workflow);
        let doc = read_workflow(&workflow_path);
        let declared = gate_needs(&doc, &workflow_path);
        let mut missing: Vec<&str> = Vec::new();
        for required in REQUIRED_GOVERNANCE_LANES {
            if !declared.iter().any(|d| d == required) {
                missing.push(required);
            }
        }

        assert!(
            missing.is_empty(),
            "jobs.gate.needs in {} is missing required governance lane(s): {missing:?}. Declared: {declared:?}. Add them to the gate.needs array in the reusable workflow.",
            workflow_path.display(),
        );
    }
}

#[test]
fn dogma_gate_reruns_when_pr_signal_changes() {
    let ci_yml = ci_yml_path();
    let text = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    for action in [
        "opened",
        "synchronize",
        "reopened",
        "edited",
        "labeled",
        "unlabeled",
        "ready_for_review",
    ] {
        assert!(
            text.contains(&format!("- {action}")),
            "{} pull_request trigger must include `{action}` so dogma cleanup gate decisions are recomputed when mutable PR signal changes",
            ci_yml.display(),
        );
    }
}

/// Cross-check: every lane named in gate.needs must also correspond to
/// a defined job in the workflow. A needs entry with no corresponding
/// job silently succeeds (skipped) on the GitHub side — another flavour
/// of silent-regression failure.
#[test]
fn gate_needs_only_references_defined_jobs() {
    for workflow in ["ci.yml", "cargo.yml", "buildbuddy.yml"] {
        let path = workflow_yml_path(workflow);
        let doc = read_workflow(&path);
        let jobs = doc
            .get("jobs")
            .and_then(|j| j.as_mapping())
            .expect("jobs mapping");
        let defined: Vec<String> = jobs
            .keys()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect();
        let declared = gate_needs(&doc, &path);

        let mut dangling: Vec<String> = Vec::new();
        for entry in &declared {
            if !defined.iter().any(|d| d == entry) {
                dangling.push(entry.clone());
            }
        }

        assert!(
            dangling.is_empty(),
            "{} jobs.gate.needs references undefined jobs: {dangling:?}. Defined jobs: {defined:?}.",
            path.display(),
        );
    }
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
    for workflow in ["ci.yml", "cargo.yml", "buildbuddy.yml"] {
        let path = workflow_yml_path(workflow);
        let doc = read_workflow(&path);
        let gate = doc
            .get("jobs")
            .and_then(|j| j.get("gate"))
            .expect("jobs.gate");
        let declared = gate_needs(&doc, &path);

        // The `run:` step body is one YAML scalar string. Find it by walking
        // steps and grabbing the first multi-line `run`.
        let run_body = gate
            .get("steps")
            .and_then(|s| s.as_sequence())
            .and_then(|seq| seq.iter().find_map(|step| step.get("run")?.as_str()))
            .expect("gate job must have a run step")
            .to_string();

        if workflow == "buildbuddy.yml" && run_body.contains("needs.*.result") {
            continue;
        }
        if workflow == "ci.yml" && declared.iter().all(|lane| run_body.contains(lane)) {
            continue;
        }

        let mut missing_from_loop: Vec<&String> = Vec::new();
        for lane in &declared {
            // The canonical shape in the explicit gate body is
            //   <lane>) result="${{ needs.<lane>.result }}" ;;
            let case_fragment = format!("{lane}) result=\"");
            let needs_ref = format!("needs.{lane}.result");
            if !run_body.contains(&case_fragment) || !run_body.contains(&needs_ref) {
                missing_from_loop.push(lane);
            }
        }

        assert!(
            missing_from_loop.is_empty(),
            "{} jobs.gate.needs lanes not referenced in the gate results loop: {missing_from_loop:?}. Each lane must appear once as a case arm (`<lane>) result=\"${{{{ needs.<lane>.result }}}}\"`) so its result is reported.",
            path.display(),
        );
    }
}

#[test]
fn machine_authority_jobs_use_changed_path_detector() {
    let ci = std::fs::read_to_string(ci_yml_path()).expect("read ci.yml");
    assert!(
        ci.contains("machine_authority_base_sha")
            && ci.contains("machine_authority_head_sha")
            && ci.contains("github.event.pull_request.base.sha")
            && ci.contains("github.event.before"),
        "top-level CI workflow must pass a diff range into reusable backends"
    );

    for workflow in ["cargo.yml", "buildbuddy.yml"] {
        let path = workflow_yml_path(workflow);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert!(
            text.contains("scripts/machine-authority-changed --base")
                && text.contains("steps.machine-changes.outputs.changed == 'true'")
                && text
                    .contains("No machine-authority inputs changed; skipping TLA+ machine checks."),
            "{} machine-authority job must gate the expensive TLA+ lane with the changed-path detector",
            path.display(),
        );
    }
}

#[test]
fn machine_authority_changed_detector_classifies_paths() {
    assert_machine_authority_detector(&["README.md"], false);
    assert_machine_authority_detector(&["specs/machines/auth/model.tla"], true);
    assert_machine_authority_detector(&["specs/compositions/schedule_bundle/model.tla"], true);
    assert_machine_authority_detector(
        &["meerkat-machine-schema/src/catalog/dsl/auth_machine.rs"],
        true,
    );
    assert_machine_authority_detector(&["meerkat-runtime/src/meerkat_machine/dsl.rs"], true);
    assert_machine_authority_detector(
        &["meerkat-core/src/generated/terminal_surface_mapping.rs"],
        true,
    );
    assert_machine_authority_detector(&["meerkat-runtime/src/generated/meerkat_mob_seam.rs"], true);
    assert_machine_authority_detector(&["meerkat-mob/src/generated/mod.rs"], true);
    assert_machine_authority_detector(&[".github/workflows/cargo.yml"], true);
    assert_machine_authority_detector(&["scripts/machine-authority-changed"], true);
}

#[test]
fn required_rmat_lanes_run_effect_authority_audit() {
    let cargo_workflow_path = workflow_yml_path("cargo.yml");
    let cargo_workflow = std::fs::read_to_string(&cargo_workflow_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", cargo_workflow_path.display()));
    assert!(
        cargo_workflow.contains("./scripts/audit-effect-authority.sh"),
        "{} rmat-audit job must run scripts/audit-effect-authority.sh before the xtask RMAT audit",
        cargo_workflow_path.display(),
    );

    let buildbuddy_lane_path = repo_path("scripts/buildbuddy-bazel-poc");
    let buildbuddy_lane = std::fs::read_to_string(&buildbuddy_lane_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", buildbuddy_lane_path.display()));
    assert!(
        buildbuddy_lane.contains("//:audit_effect_authority_test"),
        "{} rmat-audit-rbe lane must include the Bazel effect-authority audit target",
        buildbuddy_lane_path.display(),
    );
}
