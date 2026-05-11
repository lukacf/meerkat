#![allow(clippy::expect_used, clippy::panic)]

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

#[test]
fn ci_exposes_only_default_cargo_and_owner_gated_gcp_buildbuddy() {
    let ci_yml = workflow_yml_path("ci.yml");
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    let doc = read_workflow(&ci_yml);
    let jobs = doc
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .expect("jobs mapping");
    let mut defined: Vec<_> = jobs.keys().filter_map(|k| k.as_str()).collect();
    defined.sort_unstable();

    assert_eq!(
        defined,
        vec![
            "cargo",
            "gate",
            "gcp-buildbuddy",
            "unauthorized-gcp-buildbuddy"
        ],
        "{} should expose only the default Cargo lane, the owner-gated GCP BuildBuddy lane, and the aggregating gate",
        ci_yml.display(),
    );
    assert!(ci.contains("github.actor == 'lukacf'"));
    assert!(ci.contains("uses: ./.github/workflows/cargo.yml"));
    assert!(ci.contains("uses: ./.github/workflows/buildbuddy.yml"));
    assert!(!ci.contains("buildbuddy-hosted"));
}

#[test]
fn cargo_workflow_is_a_single_changed_path_gate() {
    let cargo_yml = workflow_yml_path("cargo.yml");
    let cargo = std::fs::read_to_string(&cargo_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", cargo_yml.display()));
    let doc = read_workflow(&cargo_yml);
    let jobs = doc
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .expect("jobs mapping");
    let mut defined: Vec<_> = jobs.keys().filter_map(|k| k.as_str()).collect();
    defined.sort_unstable();

    assert_eq!(defined, vec!["cargo"]);
    assert!(cargo.contains("scripts/cargo-agent-gate"));
    assert!(cargo.contains("--committed"));
    assert!(!cargo.contains("buildbuddy"));
}

#[test]
fn buildbuddy_workflow_is_gcp_only() {
    let buildbuddy_yml = workflow_yml_path("buildbuddy.yml");
    let buildbuddy = std::fs::read_to_string(&buildbuddy_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", buildbuddy_yml.display()));
    let doc = read_workflow(&buildbuddy_yml);
    let jobs = doc
        .get("jobs")
        .and_then(|j| j.as_mapping())
        .expect("jobs mapping");
    let mut defined: Vec<_> = jobs.keys().filter_map(|k| k.as_str()).collect();
    defined.sort_unstable();

    assert_eq!(
        defined,
        vec![
            "audit-submit",
            "control-plane-up",
            "executors-down",
            "executors-up",
            "feature-submit",
            "gate",
            "governance-submit",
            "minimal-feature-submit",
            "native-submit",
            "prebuild-submit",
            "static-submit",
            "wasm-check-submit",
            "wasm-feature-changes",
            "wasm-sdk-submit",
        ],
    );
    assert!(buildbuddy.contains("MEERKAT_BAZEL_BACKEND: gcp-buildbuddy"));
    assert!(buildbuddy.contains("CI_STARTED_AT_EPOCH:"));
    assert!(buildbuddy.contains("Check GCP CI duration SLO"));
    assert!(buildbuddy.contains("run-buildbuddy-ci-lane-batch"));
    assert!(buildbuddy.contains("profile: submitter"));
    assert!(!buildbuddy.contains("buildbuddy-hosted"));
    assert!(!buildbuddy.contains("profile: native"));
}
