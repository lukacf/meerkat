#![allow(clippy::expect_used, clippy::panic)]

//! Pinning tests for the CI workflow contract.
//!
//! CI runs one authoritative GCP BuildBuddy/RBE lane. The GitHub-hosted Cargo
//! workflow remains a diagnostic fallback, while nightly owns low-churn heavy
//! coverage. These tests ratchet the load-bearing invariants: the typed
//! governance gates (rmat-audit set) bind every run, BuildBuddy stays on the
//! hot path, and the aggregate gate enforces the 20-minute terminal budget.

use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root
}

fn repository_path(path: &str) -> PathBuf {
    repository_root().join(path)
}

fn workflow_yml_path(name: &str) -> PathBuf {
    let mut path = repository_path(".github/workflows");
    path.push(name);
    path
}

fn read_workflow(path: &Path) -> serde_yaml::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse {} as YAML: {e}", path.display()))
}

fn job_names(doc: &serde_yaml::Value, path: &Path) -> Vec<String> {
    let jobs = doc
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .unwrap_or_else(|| panic!("{} must have a jobs mapping", path.display()));
    let mut defined: Vec<String> = jobs
        .keys()
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect();
    defined.sort_unstable();
    defined
}

#[test]
fn ci_runs_one_authoritative_buildbuddy_lane() {
    let ci_yml = workflow_yml_path("ci.yml");
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    let doc = read_workflow(&ci_yml);

    assert_eq!(
        job_names(&doc, &ci_yml),
        vec!["gate", "gcp-buildbuddy"],
        "{} should expose only BuildBuddy and the aggregating gate",
        ci_yml.display(),
    );
    assert!(
        ci.contains("uses: ./.github/workflows/buildbuddy.yml"),
        "CI must route through the authoritative GCP BuildBuddy lane"
    );
    assert!(
        !ci.contains("uses: ./.github/workflows/cargo.yml"),
        "the diagnostic Cargo workflow must not duplicate authoritative CI"
    );
    assert!(
        !ci.contains("github.actor"),
        "CI must not route by actor — one lane for everyone"
    );
    assert!(ci.contains("name: Enforce push-to-terminal budget"));
    assert!(ci.contains("MAX_SECONDS: \"1200\""));
    assert!(
        ci.contains("id-token: write"),
        "the caller must grant the OIDC permission requested by the reusable BuildBuddy workflow"
    );
}

#[test]
fn machine_authority_classifier_protects_required_gate_owners() {
    let classifier_path = repository_path("scripts/machine-authority-changed");
    let root = repository_root();

    for owner in [
        ".github/workflows/ci.yml",
        "scripts/tests/xtask_scripts_dogma_gates.sh",
    ] {
        let output = std::process::Command::new(&classifier_path)
            .current_dir(&root)
            .args(["--", owner])
            .output()
            .unwrap_or_else(|e| panic!("run {}: {e}", classifier_path.display()));
        assert!(
            output.status.success(),
            "machine-authority classifier must protect required gate owner `{owner}`: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn cargo_diagnostic_workflow_preserves_the_full_gate_set() {
    let cargo_yml = workflow_yml_path("cargo.yml");
    let cargo = std::fs::read_to_string(&cargo_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", cargo_yml.display()));
    let doc = read_workflow(&cargo_yml);

    assert_eq!(
        job_names(&doc, &cargo_yml),
        vec![
            "audit",
            "changes",
            "clippy",
            "e2e-fast",
            "fmt-governance",
            "gate",
            "int-archives",
            "int-else",
            "int-mob",
            "int-rest",
            "locks",
            "machine-verify",
            "ratchets",
            "sdk-host",
            "sdk-web",
            "unit",
            "unit-archive",
            "wasm-check",
            "wasm-contract",
        ],
    );

    // The typed governance gates must bind every CI run — this is the
    // original intent of this test module and must survive lane reshuffles.
    for gate in [
        "make rmat-audit",
        "make seam-inventory",
        "make runtime-authority-bypass",
        "make storage-ambient-gate",
        "make sync-meerkat-dogma-skill-docs",
        "make machine-authority-docs-gate",
        "make audit-generated-headers",
    ] {
        assert!(
            cargo.contains(gate),
            "cargo lane must run `{gate}` on every push"
        );
    }

    let jobs = doc
        .get("jobs")
        .and_then(serde_yaml::Value::as_mapping)
        .expect("cargo workflow jobs mapping");
    let fmt_governance = jobs
        .get(serde_yaml::Value::String("fmt-governance".to_string()))
        .and_then(serde_yaml::Value::as_mapping)
        .expect("fmt-governance job");
    assert!(
        fmt_governance.get("if").is_none(),
        "docs-only changes must not skip the always-run governance authority"
    );

    let machine_verify = jobs
        .get(serde_yaml::Value::String("machine-verify".to_string()))
        .and_then(serde_yaml::Value::as_mapping)
        .expect("machine-verify job");
    let machine_condition = machine_verify
        .get("if")
        .and_then(serde_yaml::Value::as_str)
        .expect("machine-verify must have a path-gated condition");
    assert!(machine_condition.contains("machine_authority_changed"));
    let machine_steps = machine_verify
        .get("steps")
        .and_then(serde_yaml::Value::as_sequence)
        .expect("machine-verify steps");
    assert!(machine_steps.iter().any(|step| {
        step.get("run")
            .and_then(serde_yaml::Value::as_str)
            .is_some_and(|run| run.contains("make machine-verify"))
    }));
    assert!(cargo.contains("actions/setup-java@v5"));
    assert!(cargo.contains("tlaplus/releases/download/v1.8.0/tla2tools.jar"));
    assert!(cargo.contains("scripts/machine-authority-changed"));
    assert!(cargo.contains("machine_authority_changed:"));

    let gate_needs = jobs
        .get(serde_yaml::Value::String("gate".to_string()))
        .and_then(|gate| gate.get("needs"))
        .and_then(serde_yaml::Value::as_sequence)
        .expect("gate needs list");
    assert!(
        gate_needs
            .iter()
            .any(|need| need.as_str() == Some("machine-verify")),
        "the required aggregate gate must bind bounded TLC verification"
    );

    // Full-workspace verification (the changed-crates-only gate missed
    // dependent-crate breakage; do not reintroduce it as the only test gate).
    // Unit and integration execution jobs consume portable Nextest archives
    // built once per compatible Cargo profile. The archive contract self-test
    // pins the complete build scopes and fail-closed partitions; clippy covers
    // the whole workspace with all features (test-target lints run nightly).
    for lane in [
        "clippy --workspace --all-features",
        "uses: ./.github/actions/build-nextest-archive",
        "uses: ./.github/actions/run-nextest-archive",
        "family: unit",
        "family: int-heavy",
        "family: int-mob",
        "family: int-everything-else",
        "scripts/test-ci-nextest-archive.sh",
        "make e2e-fast",
        "make verify-schema-freshness",
        "make verify-sdk-codegen-freshness",
        "make machine-verify",
        "make audit",
    ] {
        assert!(cargo.contains(lane), "cargo lane must run `{lane}`");
    }

    assert!(
        !cargo.contains("buildbuddy"),
        "the cargo lane must not depend on BuildBuddy"
    );
    assert!(
        !cargo.contains("self-hosted"),
        "the cargo lane runs on free GitHub-hosted runners only"
    );
}

#[test]
fn nightly_covers_the_deferred_heavy_lanes() {
    let nightly_yml = workflow_yml_path("nightly.yml");
    let nightly = std::fs::read_to_string(&nightly_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", nightly_yml.display()));
    let doc = read_workflow(&nightly_yml);

    // schedule + manual dispatch (serde_yaml parses the bare `on:` key as a
    // boolean, so assert on the raw text).
    assert!(nightly.contains("schedule:"), "nightly must run on a cron");
    assert!(nightly.contains("workflow_dispatch"));
    drop(doc);

    // Together with the per-push cargo lane this must cover the complete
    // `make ci` target set — these are the targets deliberately moved off
    // the hot path, not dropped.
    for lane in [
        "make lint",
        "make lint-feature-matrix",
        "make test-feature-matrix",
        "make test-minimal",
        "make test-surface-modularity",
        "make e2e-system",
        "make test-sdk-web",
        "make check-rust-release-packaging",
    ] {
        assert!(nightly.contains(lane), "nightly must run `{lane}`");
    }
}

#[test]
fn buildbuddy_workflow_is_authoritative_and_single_caller() {
    let ci_yml = workflow_yml_path("ci.yml");
    let cargo_yml = workflow_yml_path("cargo.yml");
    let nightly_yml = workflow_yml_path("nightly.yml");
    let buildbuddy_yml = workflow_yml_path("buildbuddy.yml");
    // The implementation stays workflow_call-only so ci.yml remains the one
    // top-level policy owner and attestation boundary.
    // (YAML 1.1 parses the bare `on` key as boolean true.)
    let doc = read_workflow(&buildbuddy_yml);
    let triggers = doc
        .get("on")
        .or_else(|| doc.get(serde_yaml::Value::Bool(true)))
        .and_then(|t| t.as_mapping())
        .unwrap_or_else(|| panic!("{} must have an `on:` mapping", buildbuddy_yml.display()));
    let mut names: Vec<&str> = triggers.keys().filter_map(|k| k.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["workflow_call"],
        "buildbuddy.yml must stay workflow_call-only behind the CI policy owner"
    );
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    assert!(
        ci.lines().any(|line| {
            line.trim_start().starts_with("uses:") && line.contains("buildbuddy.yml")
        }),
        "{} must call the authoritative BuildBuddy workflow",
        ci_yml.display()
    );
    for caller in [&cargo_yml, &nightly_yml] {
        let text = std::fs::read_to_string(caller)
            .unwrap_or_else(|e| panic!("read {}: {e}", caller.display()));
        assert!(
            !text.lines().any(|line| {
                line.trim_start().starts_with("uses:") && line.contains("buildbuddy.yml")
            }),
            "{} must not duplicate the authoritative BuildBuddy caller",
            caller.display()
        );
    }
    let buildbuddy = std::fs::read_to_string(&buildbuddy_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", buildbuddy_yml.display()));
    assert!(buildbuddy.contains("MAX_SECONDS: \"1200\""));
    assert!(!buildbuddy.contains("queue: max"));
}
