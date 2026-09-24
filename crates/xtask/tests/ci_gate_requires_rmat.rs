#![allow(clippy::expect_used, clippy::panic)]

//! Pinning tests for the CI workflow contract.
//!
//! Pull-request CI (ci.yml) is Cargo-only on GitHub-hosted runners: the lanes
//! are selected from the changed paths by scripts/ci-cargo-lanes.mjs, which
//! fails closed, and the aggregate "CI gate" enforces a 20-minute
//! push-to-terminal budget. Nightly owns the full workspace test lanes, the
//! dense Mob topology stress, bounded TLC, and the whole BuildBuddy/Bazel
//! graph; the release workflow re-runs that graph on the tag. The full
//! GitHub-hosted Cargo workflow (cargo.yml) remains a diagnostic fallback.
//! These tests ratchet the load-bearing invariants: the typed governance
//! gates (rmat-audit set) bind the diagnostic and nightly lanes, the
//! fail-closed lane selection stays on the PR hot path, and BuildBuddy is
//! never called from PR CI.

use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
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
fn ci_runs_fail_closed_cargo_lanes_on_hosted_runners() {
    let ci_yml = workflow_yml_path("ci.yml");
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    let doc = read_workflow(&ci_yml);

    assert_eq!(
        job_names(&doc, &ci_yml),
        vec![
            "changes",
            "clippy",
            "closure-check",
            "fmt-governance",
            "gate",
            "main-unit",
            "ratchets",
            "sdk-host",
            "unit",
            "wasm-check",
        ],
        "{} should expose the changed-path Cargo lanes and the aggregating gate",
        ci_yml.display(),
    );
    assert!(
        ci.contains("scripts/ci-cargo-lanes.mjs"),
        "CI must select its lanes through the fail-closed changed-path classifier"
    );
    for forbidden in [
        "uses: ./.github/workflows/buildbuddy.yml",
        "uses: ./.github/workflows/cargo.yml",
        "uses: ./.github/workflows/mob-dense-topology.yml",
        "buildbuddy",
        "self-hosted",
    ] {
        assert!(
            !ci.to_lowercase().contains(forbidden),
            "PR CI must stay Cargo-only on hosted runners; found `{forbidden}`"
        );
    }
    assert!(
        !ci.contains("github.actor"),
        "CI must not route by actor: one lane for everyone"
    );
    assert!(ci.contains("name: Enforce push-to-terminal budget"));
    assert!(
        ci.contains("CI_MAX_SECONDS: \"1200\""),
        "the push-to-terminal budget is 1200 seconds from run creation"
    );
    assert!(
        ci.contains("format('pr-{0}', github.event.pull_request.number)"),
        "one concurrency group per pull request"
    );
    assert!(
        ci.contains("format('main-{0}-{1}', github.ref_name, github.sha)"),
        "one concurrency group per pushed commit, so main runs never cancel each other"
    );
    assert!(
        ci.contains("cancel-in-progress: ${{ github.event_name == 'pull_request' }}"),
        "only pull-request runs are cancelled by a newer push"
    );

    let jobs = doc
        .get("jobs")
        .and_then(serde_yaml::Value::as_mapping)
        .expect("ci workflow jobs mapping");
    let gate = jobs
        .get(serde_yaml::Value::String("gate".to_string()))
        .and_then(serde_yaml::Value::as_mapping)
        .expect("gate job");
    assert_eq!(
        gate.get("name").and_then(serde_yaml::Value::as_str),
        Some("CI gate"),
        "branch protection requires the exact context name `CI gate`"
    );
    assert_eq!(
        gate.get("if").and_then(serde_yaml::Value::as_str),
        Some("${{ !cancelled() }}"),
        "a superseded run must surface as cancelled, not as a failed CI gate"
    );
    let gate_needs: Vec<&str> = gate
        .get("needs")
        .and_then(serde_yaml::Value::as_sequence)
        .expect("gate needs list")
        .iter()
        .filter_map(serde_yaml::Value::as_str)
        .collect();
    for lane in [
        "changes",
        "fmt-governance",
        "ratchets",
        "clippy",
        "unit",
        "main-unit",
        "closure-check",
        "wasm-check",
        "sdk-host",
    ] {
        assert!(gate_needs.contains(&lane), "the CI gate must bind `{lane}`");
    }
    // Fail closed: a build-relevant change must have run clippy, unit, and
    // the closure check; a classifier error or an empty plan fails the gate.
    for contract in [
        "require_success \"Change classification\"",
        "require_ran \"Clippy\"",
        "require_ran \"Unit tests\"",
        "require_ran \"Main unit tests\"",
        "require_ran \"Closure check\"",
        "a build-relevant change produced no lanes",
        "neither a unit lane nor a deferred package list",
        "unit tests deferred to the push-to-main run",
    ] {
        assert!(ci.contains(contract), "CI gate must enforce `{contract}`");
    }
    for lane in [
        "--no-deps --all-targets --all-features -- -D warnings",
        "--lib --bins --profile ci-pr",
        "unit_shard_matrix",
        "main_unit_shard_matrix",
        "CLOSURE_CHECK_TARGETS: lib",
        "--all-features",
        "make fmt-check",
        "make docs-check",
        "make semver-breaks-selftest",
        "make verify-version-parity",
        "make verify-lock-consistency",
        "make ci-lanes-selftest",
        "make verify-schema-freshness",
        "make verify-sdk-codegen-freshness",
        "make machine-check-drift",
        "make wasm-check",
        "schema_version: 4",
        "validation_backend: \"github-hosted-cargo\"",
    ] {
        assert!(ci.contains(lane), "PR CI must run `{lane}`");
    }
    let fmt_governance = jobs
        .get(serde_yaml::Value::String("fmt-governance".to_string()))
        .and_then(serde_yaml::Value::as_mapping)
        .expect("fmt-governance job");
    assert!(
        fmt_governance.get("if").is_none(),
        "docs-only changes must not skip the always-run format and governance lane"
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
            "dense-topology",
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
        // Moved off the PR hot path by the Cargo-only PR CI.
        "make test-unit",
        "make test-int",
        "make e2e-fast",
        "make machine-verify",
        "test-sdk-python test-sdk-typescript",
        "uses: ./.github/workflows/mob-dense-topology.yml",
        "uses: ./.github/workflows/buildbuddy.yml",
        "mode: full-fresh",
    ] {
        assert!(nightly.contains(lane), "nightly must run `{lane}`");
    }
}

#[test]
fn buildbuddy_workflow_is_called_only_by_nightly_and_release() {
    let ci_yml = workflow_yml_path("ci.yml");
    let cargo_yml = workflow_yml_path("cargo.yml");
    let nightly_yml = workflow_yml_path("nightly.yml");
    let release_yml = workflow_yml_path("release.yml");
    let buildbuddy_yml = workflow_yml_path("buildbuddy.yml");
    // The implementation stays workflow_call-only: nightly and the release
    // workflow are its callers, PR CI never is.
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
        "buildbuddy.yml must stay workflow_call-only behind its callers"
    );
    let calls_buildbuddy = |path: &Path| {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        text.lines()
            .any(|line| line.trim_start().starts_with("uses:") && line.contains("buildbuddy.yml"))
    };
    for caller in [&nightly_yml, &release_yml] {
        assert!(
            calls_buildbuddy(caller),
            "{} must call the BuildBuddy workflow in full-fresh mode",
            caller.display()
        );
    }
    for non_caller in [&ci_yml, &cargo_yml] {
        assert!(
            !calls_buildbuddy(non_caller),
            "{} must not call the BuildBuddy workflow",
            non_caller.display()
        );
    }
    let buildbuddy = std::fs::read_to_string(&buildbuddy_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", buildbuddy_yml.display()));
    assert!(
        buildbuddy.contains("MAX_SECONDS: \"3000\""),
        "the full-fresh graph is sized at 3000 seconds from control-plane start"
    );
    assert!(!buildbuddy.contains("queue: max"));
    let release = std::fs::read_to_string(&release_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", release_yml.display()));
    assert!(
        release.contains("release_validate_buildbuddy_full:"),
        "the tag path must gate on the full BuildBuddy graph"
    );
    assert!(
        release.contains(".schema_version == 4") && release.contains("github-hosted-cargo"),
        "require_ci_green must accept the Cargo PR CI attestation"
    );
}
