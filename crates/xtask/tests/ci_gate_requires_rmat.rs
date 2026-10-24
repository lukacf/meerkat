#![allow(clippy::expect_used, clippy::panic, clippy::disallowed_methods)]

//! Pinning tests for the CI workflow contract.
//!
//! Pull-request CI (ci.yml) is Cargo-only on GitHub-hosted runners: the lanes
//! are selected from the changed paths by scripts/ci-cargo-lanes.mjs, which
//! fails closed, and the aggregate "CI gate" enforces, on pull requests, a
//! 1080-second lane execution budget and a 1200-second push-to-terminal
//! runaway ceiling, runner queue excluded. Nightly owns the full workspace test lanes, the
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

/// The YAML rules the frontmatter parser uses (meerkat-skills): YAML 1.2
/// booleans, so the bare `on:` key is the string "on"; `<<` an ordinary key;
/// duplicate keys and unknown tags refused.
fn workflow_yaml_options() -> serde_saphyr::Options {
    let mut options = serde_saphyr::Options::default();
    options.strict_booleans = true;
    options.merge_keys = serde_saphyr::MergeKeyPolicy::AsOrdinary;
    options.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::Error;
    options.reject_unsupported_tags = true;
    options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::PassThrough;
    options
}

fn read_workflow(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_saphyr::from_str_with_options(&text, workflow_yaml_options())
        .unwrap_or_else(|e| panic!("cannot parse {} as YAML: {e}", path.display()))
}

/// Every step's `run` body and `uses:` reference, for content checks that do
/// not depend on how a YAML emitter would render the job.
fn job_step_text(job: &serde_json::Map<String, serde_json::Value>) -> String {
    job.get("steps")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|step| {
            let run = step
                .get("run")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let uses = step
                .get("uses")
                .and_then(serde_json::Value::as_str)
                .map(|uses| format!("uses: {uses}"));
            run.into_iter().chain(uses)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn job_names(doc: &serde_json::Value, path: &Path) -> Vec<String> {
    let jobs = doc
        .get("jobs")
        .and_then(|j| j.as_object())
        .unwrap_or_else(|| panic!("{} must have a jobs mapping", path.display()));
    let mut defined: Vec<String> = jobs.keys().cloned().collect();
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
            "bazel-graph",
            "changes",
            "clippy",
            "clippy-rest",
            "closure-check",
            "closure-tests",
            "closure-tests-rest",
            "example-web",
            "feature-check",
            "feature-check-rest",
            "fmt-governance",
            "gate",
            "governance-gates",
            "governed-jsonl",
            "integration",
            "main-unit",
            "main-unit-archive-build",
            "main-unit-archive-run",
            "main-unit-rest",
            "mcp-confinement-positive",
            "ratchets",
            "rustdoc",
            "sdk-host",
            "stagger",
            "tlc-audits",
            "unit",
            "wasm-check",
            "wasm-timers",
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
    // The ripgrep tombstone scans run on every pull request, with ripgrep
    // installed first: each scan refuses to run without it.
    for gate in [
        "make legacy-surface-gate",
        "make session-control-gate",
        "make deprecated-backend-gate",
    ] {
        assert!(ci.contains(gate), "PR CI must run `{gate}`");
    }
    assert!(
        ci.contains("sudo apt-get install -y ripgrep") && ci.contains("rg --version"),
        "PR CI installs ripgrep and proves it is on PATH before the tombstone scans"
    );
    assert!(ci.contains("name: Enforce lane execution budget"));
    // The budget measures what the code controls: lane execution (started to
    // completed) plus the classification it waits for. Runner queue wait is a
    // warning, and a generous push-to-terminal ceiling still fails a stuck CI.
    // The owner's cap is 20 minutes from the start of the pre-push hook to a
    // green gate, and the hook takes about a minute.
    assert!(
        ci.contains("CI_MAX_EXEC_SECONDS: \"1080\""),
        "the lane execution budget is 1080 seconds"
    );
    assert!(
        ci.contains("CI_MAX_TERMINAL_SECONDS: \"1200\""),
        "the push-to-terminal runaway ceiling is 1200 seconds"
    );
    assert!(
        ci.contains("title=CI runner queue wait") && ci.contains("budget_verdict="),
        "runner queue wait is reported as a typed warning, not budgeted"
    );
    assert!(
        !ci.contains("CI_MAX_SECONDS:"),
        "no single push-to-terminal budget that charges runner queue to the code"
    );
    // Each lane is timed from the start of the attempt it ran in: a re-run
    // lane gets a fresh clock, a carried-over lane keeps its own attempt's
    // clock, so re-running only the gate cannot launder an overrun. The run's
    // fixed created_at would make every re-run fail.
    assert!(
        ci.contains("/attempts/{n}\"") && ci.contains("run_started_at"),
        "the budget reads each attempt's start"
    );
    assert!(
        !ci.contains("--jq .created_at"),
        "the budget must not measure from the run's fixed creation time"
    );
    assert!(
        ci.contains("slowest lane"),
        "the budget reports the slowest lane"
    );
    assert!(
        ci.contains("format('pr-{0}', github.event.pull_request.number)"),
        "one concurrency group per pull request"
    );
    // A newer main commit supersedes an older commit's first attempt, but a
    // release commit, every re-run attempt and a dispatch keep one group per
    // commit and are never cancelled: the release workflow needs a
    // successful exact-main run on the release commit.
    assert!(
        ci.contains("format('main-{0}-superseded', github.ref_name)"),
        "first attempts of main pushes share a superseding group"
    );
    assert!(
        ci.contains("format('main-{0}-{1}', github.ref_name, github.sha)"),
        "release commits, re-runs and dispatches keep one group per commit"
    );
    // Release integration branches are never superseded either: every merge
    // there needs its own completed verdict.
    let supersedable = "github.event_name == 'push' && github.run_attempt == 1 && !contains(github.event.head_commit.message, 'chore: release v') && !startsWith(github.ref, 'refs/heads/release/')";
    assert_eq!(
        ci.matches(supersedable).count(),
        2,
        "the superseding group and its cancel-in-progress use the same condition, \
         both exempting release/** pushes"
    );

    let jobs = doc
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .expect("ci workflow jobs mapping");
    let gate = jobs
        .get("gate")
        .and_then(serde_json::Value::as_object)
        .expect("gate job");
    assert_eq!(
        gate.get("name").and_then(serde_json::Value::as_str),
        Some("CI gate"),
        "branch protection requires the exact context name `CI gate`"
    );
    assert_eq!(
        gate.get("if").and_then(serde_json::Value::as_str),
        Some("${{ !cancelled() }}"),
        "a superseded run must surface as cancelled, not as a failed CI gate"
    );
    let gate_needs: Vec<&str> = gate
        .get("needs")
        .and_then(serde_json::Value::as_array)
        .expect("gate needs list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for lane in [
        "changes",
        "fmt-governance",
        "ratchets",
        "governance-gates",
        "tlc-audits",
        "clippy",
        "unit",
        "integration",
        "closure-tests",
        "governed-jsonl",
        "main-unit",
        "main-unit-archive-build",
        "main-unit-archive-run",
        "closure-check",
        "rustdoc",
        "wasm-check",
        "sdk-host",
        "example-web",
        "wasm-timers",
        "bazel-graph",
    ] {
        assert!(gate_needs.contains(&lane), "the CI gate must bind `{lane}`");
    }
    // Fail closed: a build-relevant change must have run clippy, unit, and
    // the closure check; a classifier error or an empty plan fails the gate.
    for contract in [
        "require_success \"Change classification\"",
        "require_ran \"Bounded TLC audits\"",
        "require_ran \"Typed governance gates\"",
        "require_ran \"Clippy\"",
        "require_ran \"Unit tests\"",
        "require_ran \"Main unit tests\"",
        "require_ran \"Closure check\"",
        "require_ran \"Bazel graph check\"",
        "require_ran \"Example web suites\"",
        "require_ran \"WASM timer ownership\"",
        "require_ran \"Integration tests\"",
        "require_ran \"Closure tests\"",
        "require_ran \"Governed JSONL\"",
        // Every tests/*.rs binary of an integration suite, unless the suite
        // names its targets (the gpt-live replay suite runs one).
        "test_flags=(--test '*')",
        "\"${test_flags[@]}\" --profile ci-pr",
        // The pre-push hook's cargo-test coverage for the test scope: every
        // row lists exactly its planned binaries before it runs them.
        "closure_test_matrix",
        "node scripts/ci-verify-row-binaries.mjs",
        "\"${test_flags[@]}\" --profile \"${PROFILE}\" \"${filter_args[@]}\"",
        "a build-relevant change produced no lanes",
        "neither a unit lane nor a deferred package list",
        "unit tests deferred to the push-to-main run",
    ] {
        assert!(ci.contains(contract), "CI gate must enforce `{contract}`");
    }
    for lane in [
        "--no-deps ${TARGETS} --all-features -- -D warnings",
        "\"--all-targets\"|\"--lib\"|\"--bins\"|\"--lib --bins\") ;;",
        "nextest run ${PACKAGES} ${TARGETS} --profile ci-pr",
        "\"--lib\"|\"--bins\"|\"--lib --bins\") ;;",
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
        "bazel query //...",
        "bazel build --nobuild //...",
        "hashFiles('MODULE.bazel.lock')",
        "npx playwright install --with-deps chromium",
        "MEERKAT_WEB_WASM_OPT: \"0\"",
        "npm run test:offline",
        "wasm-pack test --headless --chrome --release crates/meerkat-web-runtime --test browser_contract --test wasm_timers",
        "wasm-pack test --node --release crates/meerkat-web-runtime --test wasm_timers_node",
        "node --test tests/e2e_wasm_runtime.test.mjs tests/wasm_mob_comms.test.mjs",
        "npm run test:packed",
    ] {
        assert!(ci.contains(lane), "PR CI must run `{lane}`");
    }
    let fmt_governance = jobs
        .get("fmt-governance")
        .and_then(serde_json::Value::as_object)
        .expect("fmt-governance job");
    assert!(
        fmt_governance.get("if").is_none(),
        "docs-only changes must not skip the always-run format and governance lane"
    );
}

/// A pull request runs what fits the cap from the start of the pre-push hook
/// to a green CI gate and leaves the lanes that compile the meerkat-mob chain
/// or sweep the reverse-dependency closure to the push, whose CI must be
/// green before a release tag. The classifier is told the event, the
/// push-only lanes follow its flags, and the gate refuses a push plan that
/// left anything to a later push.
#[test]
fn pull_requests_leave_the_chain_and_closure_lanes_to_the_push() {
    let ci_yml = workflow_yml_path("ci.yml");
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    let doc = read_workflow(&ci_yml);
    let jobs = doc
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .expect("ci workflow jobs mapping");
    let job_if = |job: &str| -> String {
        jobs.get(job)
            .and_then(|job| job.get("if"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    assert!(
        ci.contains("node scripts/ci-cargo-lanes.mjs --format github --event \"${EVENT_NAME}\""),
        "the change classification must plan for the event it runs in"
    );
    for (job, flag) in [
        ("closure-check", "closure_check"),
        ("rustdoc", "rustdoc"),
        ("governed-jsonl", "governed_jsonl"),
        ("mcp-confinement-positive", "mcp_confinement_positive"),
    ] {
        assert_eq!(
            job_if(job),
            format!("${{{{ needs.changes.outputs.{flag} == 'true' }}}}"),
            "{job} runs exactly when the plan selects it"
        );
    }
    // The workspace rustdoc runs two parallel legs: default features and
    // --all-features.
    let rustdoc_legs = jobs
        .get("rustdoc")
        .and_then(|job| job.pointer("/strategy/matrix/features"))
        .and_then(serde_json::Value::as_array)
        .expect("the rustdoc job's features matrix");
    assert_eq!(
        rustdoc_legs,
        &vec![
            serde_json::Value::from("default"),
            serde_json::Value::from("all")
        ],
        "the rustdoc lane runs a default-features leg and an all-features leg"
    );
    // Each of these lanes runs in two waves (long rows first): the job holds
    // the first wave, its -rest copy the rest.
    for (job, rows) in [
        ("closure-tests", "closure_test"),
        ("feature-check", "feature_check"),
    ] {
        assert!(
            job_if(job).contains(&format!("needs.changes.outputs.{rows}_first_count != '0'")),
            "{job} runs the first-wave rows the plan names"
        );
        assert!(
            job_if(&format!("{job}-rest"))
                .contains(&format!("needs.changes.outputs.{rows}_rest_count != '0'")),
            "{job}-rest runs the rest of the rows the plan names"
        );
    }
    // The push lints every target of every changed package, the chain heads
    // a pull request left to it included, and a Clippy failure is a red CI
    // gate: neither job (one per wave) nor its lint step may soft-fail.
    for name in ["clippy", "clippy-rest"] {
        let clippy = jobs
            .get(name)
            .and_then(serde_json::Value::as_object)
            .unwrap_or_else(|| panic!("{name} job"));
        assert!(
            clippy.get("continue-on-error").is_none(),
            "the {name} job never soft-fails"
        );
        let lint_step = clippy
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("{name} steps"))
            .iter()
            .find(|step| {
                step.get("run")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|run| run.contains("./scripts/repo-cargo clippy"))
            })
            .unwrap_or_else(|| panic!("the {name} lint step"));
        assert!(
            lint_step.get("continue-on-error").is_none(),
            "the {name} lint step never soft-fails"
        );
    }
    for contract in [
        "a push plan left lanes to a later push",
        "forbid_failure \"Closure check\" \"${CLOSURE_CHECK_RESULT}\"",
        "forbid_failure \"Rustdoc\" \"${RUSTDOC_RESULT}\"",
        // Every crate documented with rustdoc's lints denied, once per leg.
        "RUSTDOCFLAGS: -Dwarnings",
        "default) ./scripts/repo-cargo doc --workspace --no-deps --keep-going ;;",
        "all) ./scripts/repo-cargo doc --workspace --no-deps --keep-going --all-features ;;",
        "forbid_failure \"Governed JSONL\" \"${GOVERNED_JSONL_RESULT}\"",
        "forbid_failure \"MCP confinement positive\" \"${MCP_CONFINEMENT_POSITIVE_RESULT}\"",
        "left to the push (main or release/**): ${DEFERRED_TO_PUSH}",
    ] {
        assert!(ci.contains(contract), "CI gate must enforce `{contract}`");
    }
}

/// A push to main diffs against the last main commit whose CI run completed
/// green with an attestation (scripts/ci-push-base.mjs, from HEAD's first
/// parent), and plans the whole workspace without one, so a superseded,
/// cancelled or failed main run never loses its changes' push-only lanes.
#[test]
fn main_pushes_diff_against_the_last_green_attested_main_commit() {
    let ci_yml = workflow_yml_path("ci.yml");
    let ci = std::fs::read_to_string(&ci_yml)
        .unwrap_or_else(|e| panic!("read {}: {e}", ci_yml.display()));
    for contract in [
        "decider=\"${RUNNER_TEMP}/push-base-tools/ci-push-plan-base.mjs\"",
        "if git show \"${BASE_SHA}:scripts/ci-push-plan-base.mjs\" > \"${decider}\" 2>/dev/null \\",
        "&& found_base=\"$(node \"${decider}\" --repository \"${GITHUB_REPOSITORY}\" --payload-base \"${BASE_SHA}\")\"; then",
        "--base \"${plan_base}\" --head HEAD \"${plan_args[@]}\"",
        "no qualifying green, attested main CI run before HEAD (reason above); planning the whole workspace.",
    ] {
        assert!(
            ci.contains(contract),
            "the change classification must carry `{contract}`"
        );
    }
    assert!(
        !ci.contains("--base \"${classification_base}\" --head HEAD \"${plan_args[@]}\""),
        "a push to main must not plan against its payload base"
    );
    assert!(
        !ci.contains("HEAD^1:scripts/ci-push-base.mjs"),
        "ci.yml must run the parent's finder only through scripts/ci-push-plan-base.mjs"
    );
    assert!(
        !ci.contains("node scripts/ci-push-plan-base.mjs"),
        "ci.yml must run the payload base's copy of the decider, never the pushed commit's"
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
        .and_then(serde_json::Value::as_object)
        .expect("cargo workflow jobs mapping");
    let fmt_governance = jobs
        .get("fmt-governance")
        .and_then(serde_json::Value::as_object)
        .expect("fmt-governance job");
    assert!(
        fmt_governance.get("if").is_none(),
        "docs-only changes must not skip the always-run governance authority"
    );

    let machine_verify = jobs
        .get("machine-verify")
        .and_then(serde_json::Value::as_object)
        .expect("machine-verify job");
    let machine_condition = machine_verify
        .get("if")
        .and_then(serde_json::Value::as_str)
        .expect("machine-verify must have a path-gated condition");
    assert!(machine_condition.contains("machine_authority_changed"));
    let machine_steps = machine_verify
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .expect("machine-verify steps");
    assert!(machine_steps.iter().any(|step| {
        step.get("run")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|run| run.contains("make machine-verify"))
    }));
    assert!(
        cargo.lines().any(|line| {
            let line = line.trim();
            line.starts_with("uses: actions/setup-java@") && line.ends_with(" # v5")
        }),
        "cargo.yml installs Java with actions/setup-java, pinned to v5's commit"
    );
    assert!(cargo.contains("uses: ./.github/actions/setup-tlc-ci"));
    assert!(cargo.contains("scripts/machine-authority-changed"));
    assert!(cargo.contains("machine_authority_changed:"));

    let gate_needs = jobs
        .get("gate")
        .and_then(|gate| gate.get("needs"))
        .and_then(serde_json::Value::as_array)
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

    // Schedule plus manual dispatch. Read as YAML 1.2, the bare `on:` key is
    // the string "on" (only YAML 1.1 readers make it the boolean true).
    let triggers = doc
        .get("on")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("{} must have an `on:` mapping", nightly_yml.display()));
    assert!(
        triggers.contains_key("schedule"),
        "nightly must run on a cron"
    );
    assert!(triggers.contains_key("workflow_dispatch"));

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
fn nightly_holds_a_bounded_number_of_hosted_slots() {
    // The account's 40 concurrent hosted jobs are shared with pull-request
    // and main CI. Nightly runs the BuildBuddy graph alone first, then a few
    // sequential chains: every other job needs exactly one job, no job is
    // needed by two others, and each runs under !cancelled() so a red job
    // never skips the rest of its chain.
    const ROOT: &str = "gcp-buildbuddy";
    const MAX_CHAINS: usize = 5;
    let doc = read_workflow(&workflow_yml_path("nightly.yml"));
    let jobs = doc
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .expect("nightly jobs mapping");
    let mut needed_by: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut chain_heads = 0;
    for (name, job) in jobs {
        let name = name.as_str();
        if name == ROOT {
            assert!(job.get("needs").is_none(), "{ROOT} runs first");
            // A graph-only dispatch (the pre-tag check) runs exactly this job.
            // Only the self-hosted graph's default-off repository gate skips
            // it on other runs (the chains below still run); a graph-only
            // dispatch always calls it, so a disabled graph fails there
            // instead of finishing green with nothing run.
            assert_eq!(
                job.get("if").and_then(serde_json::Value::as_str),
                Some("${{ vars.MEERKAT_GCP_BUILDBUDDY_ENABLED == 'true' || inputs.graph_only }}"),
                "{ROOT} runs on every nightly run while the self-hosted graph is enabled and on every graph-only dispatch"
            );
            continue;
        }
        let need = job
            .get("needs")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("nightly job {name} must need exactly one job (a chain)"));
        assert_eq!(
            job.get("if").and_then(serde_json::Value::as_str),
            Some("${{ !cancelled() && !inputs.graph_only }}"),
            "nightly job {name} must run under !cancelled() so a red predecessor does not skip it, and skip only on a graph-only dispatch"
        );
        *needed_by.entry(need.to_string()).or_default() += 1;
        if need == ROOT {
            chain_heads += 1;
        }
    }
    for (need, count) in &needed_by {
        if need != ROOT {
            assert_eq!(
                *count, 1,
                "{need} starts two nightly jobs: chains must not fan out"
            );
        }
        assert!(jobs.contains_key(need.as_str()), "unknown job {need}");
    }
    assert!(
        (1..=MAX_CHAINS).contains(&chain_heads),
        "nightly must run at most {MAX_CHAINS} chains after {ROOT} (found {chain_heads})"
    );
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
    // (Read as YAML 1.2, the bare `on` key is the string "on".)
    let doc = read_workflow(&buildbuddy_yml);
    let triggers = doc
        .get("on")
        .and_then(|t| t.as_object())
        .unwrap_or_else(|| panic!("{} must have an `on:` mapping", buildbuddy_yml.display()));
    let mut names: Vec<&str> = triggers.keys().map(String::as_str).collect();
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
    // The self-hosted graph is default off behind one repository variable:
    // every job in buildbuddy.yml that starts, syncs or scales GCP carries it,
    // and while it is off a tag validates on its exact-tree CI attestation,
    // with Cargo only for a legacy one
    // (scripts/test-gcp-buildbuddy-workflow-gate.sh plans every route).
    const GATE_ON: &str = "vars.MEERKAT_GCP_BUILDBUDDY_ENABLED == 'true'";
    const GATE_OFF: &str = "vars.MEERKAT_GCP_BUILDBUDDY_ENABLED != 'true'";
    let job_if = |workflow: &serde_json::Value, job: &str| -> String {
        workflow
            .get("jobs")
            .and_then(|jobs| jobs.get(job))
            .and_then(|job| job.get("if"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    for job in [
        "control-plane-up",
        "executors-up",
        "executors-down",
        "control-plane-down",
    ] {
        let condition = job_if(&doc, job);
        assert!(
            condition.contains(GATE_ON) && !condition.contains("||"),
            "buildbuddy.yml job {job} must run only while the self-hosted graph is enabled; if: {condition:?}"
        );
    }
    let release_doc = read_workflow(&release_yml);
    assert!(
        job_if(&release_doc, "release_validate_buildbuddy_full").contains(GATE_ON),
        "the tag runs the full BuildBuddy graph only while the self-hosted graph is enabled"
    );
    assert!(
        job_if(&release_doc, "release_validate_cargo").contains(GATE_OFF),
        "Cargo validates a legacy-attested tag only while the self-hosted graph is disabled"
    );
}

/// Bounded TLC runs on pull requests that touch machine authority: the
/// hand-written audits model check the generated machines against their
/// invariants and goals, which the drift ratchet alone does not. The lane
/// runs every audit shard of the canonical TLC lane on a machine-authority
/// change, inside the PR budget, with TLC from the shared setup-tlc-ci action
/// (its contents are pinned by scripts/tests/xtask_scripts_dogma_gates.sh),
/// and the CI gate requires it on such a change.
#[test]
fn ci_runs_the_bounded_tlc_audits_on_machine_authority_changes() {
    let ci_yml = workflow_yml_path("ci.yml");
    let doc = read_workflow(&ci_yml);
    let jobs = doc
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .expect("ci workflow jobs mapping");
    let job = jobs
        .get("tlc-audits")
        .and_then(serde_json::Value::as_object)
        .expect("tlc-audits job");
    assert_eq!(
        job.get("if").and_then(serde_json::Value::as_str),
        Some("${{ needs.changes.outputs.machine_authority == 'true' }}"),
        "the TLC audit lane runs exactly on machine-authority changes"
    );
    let parts: Vec<&str> = job
        .get("strategy")
        .and_then(|strategy| strategy.get("matrix"))
        .and_then(|matrix| matrix.get("part"))
        .and_then(serde_json::Value::as_array)
        .expect("tlc-audits matrix parts")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        parts,
        ["audits-a", "audits-b", "audits-c"],
        "every audit shard runs"
    );
    assert_eq!(
        job.get("timeout-minutes")
            .and_then(serde_json::Value::as_u64),
        Some(20),
        "a TLC shard times out at 20 minutes, well over its expected 5-8"
    );
    let job_text = job_step_text(job);
    for required in [
        "crates/xtask/tests/machine_verify_all_tlc_test.sh",
        "--part \"${{ matrix.part }}\"",
        "uses: ./.github/actions/setup-tlc-ci",
    ] {
        assert!(
            job_text.contains(required),
            "tlc-audits must contain `{required}`; job:\n{job_text}"
        );
    }
    let ci = std::fs::read_to_string(&ci_yml).expect("read ci.yml");
    assert!(
        ci.contains("require_ran \"Bounded TLC audits\" \"${TLC_AUDITS_RESULT}\""),
        "the CI gate must require the TLC audits on a machine-authority change"
    );
}

/// The typed governance gates run in CI on the agent gate's governance paths
/// (machine authority or a dogma document), where the pre-push hook's
/// changed-path ratchets ran them, in a job of their own, and the CI gate
/// requires them on such a change.
#[test]
fn ci_runs_the_typed_governance_gates_on_governance_changes() {
    let ci_yml = workflow_yml_path("ci.yml");
    let doc = read_workflow(&ci_yml);
    let jobs = doc
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .expect("ci workflow jobs mapping");
    let job = jobs
        .get("governance-gates")
        .and_then(serde_json::Value::as_object)
        .expect("governance-gates job");
    assert_eq!(
        job.get("if").and_then(serde_json::Value::as_str),
        Some("${{ needs.changes.outputs.governance_gates == 'true' }}"),
        "the governance gates run exactly on governance changes"
    );
    let job_text = job_step_text(job);
    assert!(
        job_text.contains(
            "make rmat-audit seam-inventory runtime-authority-bypass machine-authority-docs-gate"
        ),
        "governance-gates must run every typed governance gate; job:\n{job_text}"
    );
    let ci = std::fs::read_to_string(&ci_yml).expect("read ci.yml");
    for contract in [
        "governance_gates: ${{ steps.plan.outputs.governance_gates }}",
        "GOVERNANCE_GATES: ${{ needs.changes.outputs.governance_gates }}",
        "require_ran \"Typed governance gates\" \"${GOVERNANCE_GATES_RESULT}\"",
    ] {
        assert!(ci.contains(contract), "CI must carry `{contract}`");
    }
}
