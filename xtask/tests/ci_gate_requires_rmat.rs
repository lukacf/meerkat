#![allow(clippy::expect_used, clippy::panic)]

//! Pinning tests for the CI workflow contract.
//!
//! CI runs a single Cargo lane on free GitHub-hosted runners (the GCP
//! BuildBuddy lane was retired 2026-07-03). These tests ratchet the load-
//! bearing invariants: the typed governance gates (rmat-audit set) bind every
//! run, the per-push lane plus the nightly workflow cover the full `make ci`
//! target set, and the retired BuildBuddy workflow stays inert until deleted.

use std::path::{Path, PathBuf};

fn workflow_yml_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push(".github/workflows");
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
fn ci_runs_a_single_free_cargo_lane() {
    let ci_yml = workflow_yml_path("ci.yml");
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    let doc = read_workflow(&ci_yml);

    assert_eq!(
        job_names(&doc, &ci_yml),
        vec!["cargo", "gate"],
        "{} should expose only the Cargo lane and the aggregating gate",
        ci_yml.display(),
    );
    assert!(ci.contains("uses: ./.github/workflows/cargo.yml"));
    assert!(
        !ci.contains("uses: ./.github/workflows/buildbuddy.yml"),
        "the retired GCP BuildBuddy lane must not be routed from CI"
    );
    assert!(
        !ci.contains("github.actor"),
        "CI must not route by actor — one lane for everyone"
    );
}

#[test]
fn cargo_workflow_covers_the_full_per_push_gate_set() {
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
            "int",
            "ratchets",
            "sdk-web",
            "unit",
            "wasm-check",
        ],
    );

    // The typed governance gates must bind every CI run — this is the
    // original intent of this test module and must survive lane reshuffles.
    for gate in [
        "make rmat-audit",
        "make seam-inventory",
        "make runtime-authority-bypass",
        "make machine-authority-docs-gate",
        "make audit-generated-headers",
    ] {
        assert!(
            cargo.contains(gate),
            "cargo lane must run `{gate}` on every push"
        );
    }

    // Full-workspace verification (the changed-crates-only gate missed
    // dependent-crate breakage; do not reintroduce it as the only test gate).
    // Unit/int run as sharded nextest partitions of the same workspace-wide
    // lanes the `unit`/`int` cargo aliases define; clippy covers the whole
    // workspace with all features (test-target lints run nightly).
    for lane in [
        "clippy --workspace --all-features",
        "repo-cargo unit --partition",
        "repo-cargo int --partition",
        "make e2e-fast",
        "make verify-schema-freshness",
        "make verify-sdk-codegen-freshness",
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
fn buildbuddy_workflow_is_retired_and_inert() {
    let ci_yml = workflow_yml_path("ci.yml");
    let cargo_yml = workflow_yml_path("cargo.yml");
    let nightly_yml = workflow_yml_path("nightly.yml");
    let buildbuddy_yml = workflow_yml_path("buildbuddy.yml");
    if !buildbuddy_yml.exists() {
        // Fully deleted — the retirement completed; nothing to pin.
        return;
    }

    // workflow_call-only: without a caller the workflow can never run.
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
        "retired buildbuddy.yml must stay workflow_call-only (inert without a caller)"
    );
    for caller in [&ci_yml, &cargo_yml, &nightly_yml] {
        let text = std::fs::read_to_string(caller)
            .unwrap_or_else(|e| panic!("read {}: {e}", caller.display()));
        // Prose may mention the file (retirement notes); a `uses:` reference
        // is what would revive it.
        assert!(
            !text.lines().any(|line| {
                line.trim_start().starts_with("uses:") && line.contains("buildbuddy.yml")
            }),
            "{} must not call the retired BuildBuddy workflow",
            caller.display()
        );
    }
}
