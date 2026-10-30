#!/usr/bin/env node
// Changed-path classifier for the GitHub-hosted Cargo PR CI (ci.yml).
//
// Reads the changed paths of a push or pull request and emits the lane plan:
// which workspace packages get a clippy lane and a unit lane, how they are
// packed into parallel shards, the reverse-dependency closure for the
// closure-check lane, and the boolean flags for the conditional lanes.
//
// The plan fails closed. Any path that can change what Cargo compiles
// (Rust sources, manifests, the lock, Cargo/nextest configuration, the
// toolchain pin, the build wrapper scripts, this classifier, or ci.yml)
// yields at least one clippy and one unit lane. A Rust-relevant path that
// cannot be attributed to one workspace package, or a diff whose base cannot
// be established, escalates to the whole workspace instead of to nothing.
// The old `changed-paths` BuildBuddy mode reported unrun lanes as passed for
// three weeks; this script exits non-zero rather than emit an empty plan for
// a build-relevant change.
//
// usage:
//   ci-cargo-lanes.mjs [--base <rev> --head <rev>] [--max-shards N]
//                      [--workspace-shards N] [--closure-test-rows N]
//                      [--lib-row-runner LABEL]
//                      [--event pull_request|push|workflow_dispatch]
//                      [--base-lock FILE] [--base-tree DIR]
//                      [--reuse-release-evidence <commit>:<run id>:<attempt>]
//                      [--format json|github] [--] [changed-path ...]
//   ci-cargo-lanes.mjs --paths-from-stdin [...]
//
// Without explicit paths, --base/--head, or stdin, the plan is the whole
// workspace (no diff base means no evidence of what did not change).
//
// Pull request versus push. --event pull_request plans what a pull request
// runs; any other event (the default) plans the push lanes, which are the
// whole set. Every Rust-relevant pull request still lints and type-checks its
// changed packages (Clippy), runs their unit tests outside the meerkat-mob
// compile chain and the integration suites that do not compile meerkat-mob,
// and runs the ratchets, the TLC audits, the Bazel graph check and the web
// lanes whose own inputs changed. A pull request leaves to the push (to
// `main` or a `release/**` branch, whose CI must be green before a release
// tag) every lane that needs the meerkat-mob chain or a reverse-dependency
// sweep: the closure-tests rows, the closure check, the feature-combination
// checks, the integration suites whose package compiles meerkat-mob, the
// governed JSONL and confinement-positive lanes (unless it touches their own
// inputs, GUARD_LANE_INPUTS, or changes their definition,
// GUARD_LANE_DEFINITIONS, or its diff is unknown), the feature suites marked
// `pullRequest: false`, the test targets of the mob-chain packages (their
// pull-request Clippy lints lib and bin targets only; their tests run on
// main), the whole Clippy of the chain heads in MOB_CHAIN_CLIPPY_HEADS when
// the change invalidates meerkat-mob's build (always in workspace mode), and
// the browser example and WASM timer lanes that only the reverse-dependency
// closure selected (wasm-check runs for every crate the wasm build compiles).
// On 4-vCPU hosted runners each of those needs 9-30 minutes
// (meerkat-mob alone compiles for 5-11 minutes whenever it misses the cache),
// so no pull request fits the 20-minute cap from the start of the pre-push
// hook to a green CI gate with them. The plan lists what a pull request left
// to the push (deferred_to_push), and the push plan runs all of it.
// The workspace rustdoc (`doc --workspace --no-deps` with rustdoc's lints
// denied) is left to the push the same way: it documents meerkat-mob and
// every crate built on it.
//
// A push runs no unit lane of its own: the rows a pull request's unit lane
// would run (the same shards, so the same `-p` sets, because Cargo unifies
// features across the packages of one run, and the same feature suites)
// run verbatim in the main unit lane, with the rest of the workspace packed
// around them (unit_rows_on_main). Both lanes run the same nextest command.
//
// Reader rows. Some tests read repository files that belong to no crate
// (scripts/ci-non-rust-readers.json, kept complete by its guard): docs/,
// .claude/skills/, CHANGELOG.md, specs/, fixtures. A change to such a file
// alone selects no package, so the plan adds the exact targets that read
// it (reader_rows, run in the integration lane): `-p` the reader with the
// inventory's features, its lib, bin, test or example target (the lib and
// bins for a reader in library, binary or build-script code, whose test
// targets are the push's closure-tests rows', as they are for a pull
// request's code change; the nested-Cargo canaries run only there too) and
// a filter on the tests the inventory names. A pull request plans them
// where no other lane of its plan runs them, except the readers that
// compile meerkat-mob (left to the push and named) and the readers keyed
// only by the whole checkout (live audits that every path selects, left
// to the push). A push runs its readers' test targets in the closure-tests
// rows, which build each package with the features a whole-workspace build
// resolves, and every package's lib and bins in the main unit lane; a test
// target that needs other features (Cargo drops a target whose required
// features are off) gets a reader row with them, and a push without a main
// unit lane (no Rust-relevant path) gets reader rows for the lib, bin and
// example targets. A reader that needs a live-provider feature
// (LIVE_ONLY_FEATURES) runs in no CI lane: the plan names it
// (reader_not_run) instead of planning or deferring it. Documentation
// inside a crate directory is no build input, except documentation the
// crate compiles in (include_str!/include_bytes! in library, binary or
// build-script code, which the inventory lists): that selects the crate
// like its code.
//
// Long rows first. The rows of the Clippy, main unit, closure-tests and
// feature-check lanes take 1-17 minutes on hosted runners, and a push makes
// about a hundred jobs ready at once for an account-wide pool of about 40
// runners, which take queued jobs in no reliable order: a long row picked
// late set the run's wall time. Each of those lanes is planned in two waves
// (waves): the rows that took WAVE_FIRST_SECONDS or more on a green run, or
// were never measured, start right after this classification; the rest
// also wait for ci.yml's stagger job, a short sleep that lets the first
// wave take runners first. The measurements are ROW_DURATIONS
// (--row-durations), regenerated from green runs with
// scripts/ci-row-durations.mjs; a stale entry only moves a row between
// waves, never out of the plan. A row is measured under its rowKey, its name
// without the member count of a packed row, so a repack that moves line
// counts keeps the row's measurement while its anchor stays the same.
//
// Cargo.lock. A lock change that is the only global path is scoped: the
// package entries the diff adds, removes or changes (version, source,
// checksum or dependency list) are walked up the head lock's own dependency
// lists (every feature, every target, the members' dev dependencies: wider
// than any one build), and the workspace members that reach one of them
// become the changed packages. A header change, a change to any other lock
// table, a dependency the lock cannot resolve, an unreadable or unparseable
// lock, or a diff with no changed entry fails closed into the whole
// workspace. The scope narrows push plans as well as pull requests, and the
// lock does not record one effect: Cargo's feature unification. A changed
// entry can turn on a feature of a dependency whose own entry is unchanged;
// in a build that unifies features across members (a `--workspace` build,
// such as the hook-parity rows below) that changes the dependency for every
// member, members that reach no changed entry included. Those members are
// not replanned; nightly's whole-workspace lanes test them. Measured on
// this lock, one external entry's change reaches 1
// to 52 of the 53 members. --base-lock names the base lock file for an
// explicit path list (self-tests); with --base/--head the base side is read
// from the base revision.
//
// Shards are packed by an estimated lane cost, not by crate count: a crate's
// Rust line count (its lib-test binary compiles every inline test) plus a
// term per workspace crate in its build closure (each top-level crate's
// test binary links the whole graph; the cold hosted-runner shards that
// bundled several such crates ran 18-25 minutes while the line-count
// balanced shards of leaf crates ran 7-9). --max-shards bounds a
// changed-package plan, --workspace-shards a whole-workspace plan.
//
// Budget model. A pull-request lane models at
//   minutes = 1 + (lines + 8000 * build_closure) / 35000
// The constants came from earlier hosted 4-vCPU runs. Those historical source
// sizes and recursive closures do not calibrate today's build-closure input;
// current estimates require comparison with hosted results. On d1dda1482 the
// runtime unit lane models 15.1 minutes, not a measured completion guarantee.
// The pull-request unit plan must stay under PR_UNIT_BUDGET_MINUTES, a
// surrogate packing cap and not a measured timing. It is 20, since the
// runtime lane models 18.3 minutes at 8ef2d8b939 (17.1 when the cap became
// 18). Measured, the runtime lane alone took 790 and 839 s on the green
// release pull-request runs 40055499505 and 40073579948, about 0.78 of its
// modelled time at a model of about 18 minutes; earlier pull-request runs
// took 359-715 s (39337274765, 39360046041, 39388235216 and 39394366439).
// At that ratio a 19.5-minute lane (runtime+3 at 8ef2d8b939) projected to
// about 915-945 s of execution path, under the gate's 1080 s. The ratio did
// not hold for runtime: on pull request run 40399556131 its lane alone took
// 1114 and 1173 s at a model of 18.3 minutes (a ratio of 1.01-1.07),
// 448-488 s of it running tests the model does not count, and its execution
// path reached 1146 and 1207 s. Runtime's lane therefore runs as two hash
// partitions (PARTITIONED_UNIT_PACKAGES below). The cap sits above the
// gate's 18-minute execution budget because it is a model threshold; the
// gate itself is unchanged.
// The enforced limits are the CI gate's: 1080 s of lane execution and 1200 s
// push-to-terminal, runner queue excluded (CI_MAX_EXEC_SECONDS and
// CI_MAX_TERMINAL_SECONDS in ci.yml); ordinary hosted runs supply the
// measured timing.
// Every unit lane that ever exceeded the push-to-terminal budget of its time
// (1200 s, since replaced by the gates above) compiled meerkat-mob's 442k
// lines: mob's own lane, and the lanes of crates
// that depend on mob and rebuild it under their own feature unification
// (rkat, rpc, rest, mcp-server, mob-mcp, mob-pack, ...). Those
// crates' unit tests therefore run on the push-to-main run (no budget) and
// on nightly; the chain is computed from cargo metadata, not listed here.
// The pull-request unit lane runs none of them. Clippy of a changed crate
// always runs in the pull request; for a mob-chain crate it lints the lib and
// bin targets there (run 38859112297: `--all-targets` of meerkat-mob took
// 1226 s, its 54 test targets and lib test most of it) and every target on
// the push.
//
// Hook-parity rows. The deterministic workspace lanes of
// scripts/pre-push-unit.sh (the whole workspace's lib and tests/*.rs
// binaries, HeadCanonical and e2e-fast) ran in the pre-push hook before
// every push; the hook now runs them only on demand (its `cargo-test` hook is
// in the manual stage), and the push runs that coverage as required rows
// (closure_test_rows, the closure-tests job) for the test scope: the
// reverse-dependency closure of the changed packages, every member when a
// global input changed. A pull request runs none of them (deferred_to_push).
// Every row but HeadCanonical
// selects the whole workspace (`--workspace`) and names only its targets, so
// Cargo resolves every crate's features exactly as the hook's single
// workspace build does and builds only those targets' graphs: the row
// compiles the hook's own units (cargo's unit graph: identical for
// `--workspace --test ...` and `--workspace --lib`; a package-scoped row
// compiled 31-39 dependency units with fewer features, and so does
// `--workspace --exclude`). The rows are
//   lib        one `--workspace --lib` row: it builds every member's lib
//              test (nothing narrower keeps the resolution) and runs the
//              scope's through a package() filter. Only with
//              --lib-row-runner, on that runner: a hosted 4-vCPU runner
//              needs about 64 min to build it (nightly's workspace unit
//              lane: 4940 s, 1071 s of it running the tests). Without one
//              the plan leaves the lane script's kind(lib) lane
//              (hook_retained_lanes): every member's lib tests run in the
//              push-to-main unit lanes (package-scoped features) and under
//              the workspace feature set in nightly's workspace unit lane
//              (`make test-unit`).
//   tests      every tests/*.rs target name the hook built, with every
//              member's target of that name, `--test NAME` by name, with
//              the hook's fast selection (ci-integration), split by
//              CLOSURE_TESTS_PER_ROW names
//   HeadCanonical and e2e-fast, from the hook's own lane commands
// Each row lists and runs one shared selection, and checks that nextest
// selects exactly the binaries the plan names before it runs.
// scripts/ci-hook-coverage.mjs reads the lanes from the lane script
// and the executing step from ci.yml, credits only whole-workspace rows (and
// HeadCanonical's own command), and the plan fails when any lane binary in
// scope has no row and is not a lib binary of a lane the plan leaves to the
// main and nightly unit lanes. A pull-request plan is checked the same way,
// against the rows its push runs.
// The test scope also holds the readers of changed non-Rust inputs
// (scripts/ci-non-rust-readers.json, kept complete by its guard): a test
// that reads docs/, specs/, sdks/, a fixture outside its crate or the whole
// checkout ran in the hook on every push, so a change to what it reads
// selects its package's rows, Rust change or not. Readers join the scope
// themselves; their dependents do not read the file.
//
// Release projections. A cargo-release version bump changes every crate's
// version, so the whole workspace is planned and every compile misses the
// cache: v0.9.1's main run took about 60 minutes plus a re-run. When the
// commit's diff from its first parent is only the forward patch projection
// scripts/release-hook.sh writes (scripts/release-projection-only.mjs, the
// pre-push hook's own fail-closed classifier) and the parent has a completed,
// successful, workspace-mode exact-main CI run (scripts/ci-release-evidence.mjs
// finds it from the parent's own copy; the change classification passes it as
// --reuse-release-evidence), the plan is mode "release-projection": no
// Clippy, unit, check, feature, wasm, SDK-suite or example lane, because the
// parent's run executed them over the whole workspace. What a projection can
// break still runs: format + governance (version parity, Cargo.lock and
// example lock consistency, MODULE.bazel.lock recorded inputs, release
// packaging and configuration, docs-check, the semver self-test, these
// classifier self-tests), the generation ratchets forced on (schema, SDK
// codegen and wrapper freshness: the schema emitter compiles meerkat-contracts
// at the new version), the Bazel graph check (BUILD files and the module lock
// changed), and the readers of the changed files: the hook-parity rows of
// every package scripts/ci-non-rust-readers.json names for them and the
// integration suites their paths select, exactly as an ordinary plan would
// run them, plus a unit row (`--lib --bins`) for each reader package and for
// meerkat-contracts, which owns ContractVersion::CURRENT: an ordinary plan
// leaves lib tests to the push-to-main unit lanes, which a projection does
// not run (it reuses the parent's run). The SDK and example suites are reused; what a
// version bump can break in them is a test or source that pins a version,
// and the evidence finder refuses reuse when anything outside the
// projection pins the old or new version (scripts/release-version-pins.mjs).
// No workspace compile check is planned: the bump changes no Rust
// source and no dependency selection, only version strings
// (CARGO_PKG_VERSION, ContractVersion::CURRENT), which version parity proves
// agree. The CI gate re-verifies the evidence before it passes and records it
// in the exact-tree attestation, and the release workflow's require_ci_green
// verifies it again. Without evidence, or for any other diff, the plan is the
// ordinary one.
//
// A commit whose message (or whose merged branch head's message) carries the
// trailer `CI-Plan: workspace` is planned as the whole workspace, whatever it
// changed: the explicit way to give a release's parent the workspace-mode
// evidence a release projection reuses. It only ever adds lanes.

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  HOOK_SCRIPT,
  checkHookCoverage,
  hookModel,
  parseFilter,
  profileFilter,
  workspaceFeatures,
} from "./ci-hook-coverage.mjs";
import { loadNonRustReaders, readerPackagesFor, readerSelectionsFor, WHOLE_CHECKOUT } from "./ci-non-rust-readers.mjs";
import {
  REUSE_CLASSIFIER,
  RELEASE_PROJECTION_MODE,
  attestationArtifactName,
  parseEvidenceArgument,
  releaseProjectionBase,
} from "./ci-release-evidence.mjs";
import {
  normalizePath,
  packageDir,
  packageDirs,
  packageForFile,
  packagesForFile,
  readMetadata,
  root,
  workspacePackages,
} from "./rust-test-selector.mjs";

const NULL_SHA = "0000000000000000000000000000000000000000";
// Cost model constants (see the header comment).
const LINK_COST_PER_DEP = 8000;
const COST_UNITS_PER_MINUTE = 35000;
const LANE_SETUP_MINUTES = 1;
export const PR_UNIT_BUDGET_MINUTES = 20;
// Packages whose unit tests run long get a unit lane of their own, so their
// test time does not stack on other crates' builds; the line-and-closure
// model counts only the build. xtask's lib and bin tests (the governance
// audits over the live workspace) ran 298-441 s on runs 38859112297 and
// 38931641220, and the pull-request lane that packed xtask with three other
// crates took 933 s, 488 s of it building. The lane counts toward the
// plan's shard cap.
export const UNIT_SOLO_PACKAGES = ["xtask"];
// Packages whose unit lane of their own runs as `partitions` rows: each row
// builds what the one row would (the same `-p` set and targets) and runs one
// `--partition hash:k/n` share of its tests, so the rows together run every
// test once and the CI gate times each row as a lane. For these the test
// run, which the model does not count, put the lane over the gate's 1080 s:
// meerkat-runtime's lane alone took 1114 and 1173 s on pull request run
// 40399556131 (625-649 s of compile, then 2747 tests in 448-488 s), and
// 938 s on run 40354257839 before that change. Its test run waits on the
// five SQLite batch-bound rows of about 215 s each (.config/nextest.toml),
// which four test threads run in two rounds. A hash partition is fixed by
// the test name: of the five, two partitions take two and three, three
// partitions would put four in one. Every row compiles the crate itself: an
// ARCHIVED_UNIT_LANES archive builds it once, but runs on the push only and
// would add a second job's setup and an artifact transfer to the path. The
// lane counts once toward the plan's shard cap.
export const PARTITIONED_UNIT_PACKAGES = [{ package: "meerkat-runtime", partitions: 2 }];
const HEAVY_ANCHOR = "meerkat-mob";

// Hook-parity rows (see the header). Every row runs on the hook's test-thread
// stack: scripts/pre-push-unit.sh exports 8 MiB for all of its lanes, the
// workspace floor of .cargo/config.toml and the hosts' worker stack budget,
// which every closure-row test fits in a debug build (#301).
export const HOOK_TEST_STACK = 8 * 1024 * 1024;
export const CLOSURE_TESTS_PER_ROW = 12;
// Nested-Cargo canaries: the tests .config/nextest.toml puts in this test
// group each build a fixture crate with a nested `cargo` and run one at a time
// (max-threads = 1). In run 38859112297 the three of
// meerkat::agent_builder_policy_canary took 446.6, 79.7 and 376.3 s one after
// another: 903 s of the 1760 s row that held them. Their binary leaves the
// packed rows for rows of its own: one selection, run as count partitions,
// one per canary. A count partition deals a binary's tests round-robin in name
// order, and the canaries share a name prefix, so they are adjacent and each
// lands in its own partition, on its own runner. A miscount changes only how
// the canaries spread, never what runs: the partitions together run every
// test, and the coverage check credits the binary only when every partition
// is planned.
export const CANARY_TEST_GROUP = "downstream-cargo-canary";
const CLOSURE_TEST_PROFILE = "ci-integration";
const CLOSURE_LIB_PROFILE = "ci-unit";
// Compile and link of one tests/*.rs binary, in cost units. Uncalibrated:
// the first hosted closure runs measure it.
const TEST_TARGET_COST = 15000;
const LIB_TARGET_KINDS = new Set(["lib", "rlib", "dylib", "cdylib", "staticlib"]);

// Feature-gated unit suites. Every unit lane builds its packages with their
// default features, so a lib or bin test behind a non-default feature ran in
// no lane at all: measured on 58023fbd, 342 lib/bin tests exist only under
// --all-features, and main CI stayed green while #1304 broke one of the
// facade's GPT Live tests. Each suite below is one more lane,
//   nextest run -p <package> --features <features> --lib --bins
// on every push to main, and in the pull request when <package> itself
// changed and is outside the meerkat-mob compile chain. Feature names are
// checked against cargo metadata, so a renamed feature fails the plan
// instead of silently dropping the suite.
//
// 22 of the facade's GPT Live tests overflow the 8 MiB test-thread stack that
// .cargo/config.toml gives every lane (debug build, measured on 82fddfc33)
// and pass at 32 MiB. Their rows, like the --all-features test lane
// (scripts/run-build-backend-lane test-all), carry that stack until the
// frames fit 8 MiB.
//
// A suite with `pullRequest: false` runs on the push only: the facade's two
// GPT Live suites compile its lib test under the realtime features, 1089 s
// and 900 s on run 38859112297 (798 s and 621 s of it building), against
// 293 s for its default-feature row, which the pull request keeps.
const GPT_LIVE_TEST_STACK = 32 * 1024 * 1024;
export const FEATURE_UNIT_SUITES = [
  // The shipping public GPT Live path (experimental_gpt_live::tests,
  // session_runtime::live_summary, service_factory) plus the realtime
  // credential and projection tests of the facade.
  { package: "meerkat", id: "openai-live", features: ["openai-live", "openai-realtime", "memory-store", "test-realtime-fixtures"], rustMinStack: GPT_LIVE_TEST_STACK, pullRequest: false },
  // The deprecated private broker adds its admission and broker tests.
  { package: "meerkat", id: "experimental-gpt-live", features: ["experimental-gpt-live", "openai-realtime", "memory-store", "test-realtime-fixtures"], rustMinStack: GPT_LIVE_TEST_STACK, pullRequest: false },
  { package: "meerkat-openai", id: "live", features: ["live", "experimental-gpt-live-gate0-harness", "test-realtime-fixtures"] },
  // The realtime adapter (live.rs, text_adapter.rs) is behind `realtime`,
  // which neither the default features nor the `live` suite enable: without
  // this row its tests ran only in nightly's workspace lane.
  { package: "meerkat-openai", id: "realtime", features: ["realtime"] },
  { package: "meerkat-live", id: "webrtc", features: ["webrtc"] },
  { package: "meerkat-session", id: "live", features: ["live", "session-store"] },
  // DefaultCompactor (compactor.rs) is behind `session-compaction`, which
  // neither the default features nor the `live` suite enable.
  { package: "meerkat-session", id: "compaction", features: ["session-compaction"] },
  { package: "meerkat-auth-core", id: "cloud", features: ["aws-sigv4", "gcp-auth", "azure-ad"] },
  { package: "meerkat-anthropic", id: "cloud", features: ["bedrock", "vertex", "foundry"] },
  { package: "meerkat-mob", id: "openai-live", features: ["experimental-gpt-live", "schema"] },
  { package: "meerkat-mob-mcp", id: "openai-live", features: ["experimental-gpt-live-gate0-harness"] },
  { package: "meerkat-rpc", id: "openai-live", features: ["experimental-gpt-live"] },
  { package: "xtask", id: "machine-authority", features: ["machine-authority"] },
  // The generated-kernel tests (runtime.rs) are behind `test-oracle`: their
  // hand-built MeerkatMachine inputs broke silently when RegisterSession
  // gained initial_run_start_holds, because no lane enabled the feature.
  { package: "meerkat-machine-kernels", id: "test-oracle", features: ["test-oracle"] },
];

// Integration-test suites. Every unit lane runs `--lib --bins`, so a crate's
// tests/*.rs binaries ran in no pull-request lane: #1349 merged with three
// failures there (runtime_alphabet_parity in meerkat-machine-codegen,
// gpt_live_generated_authority in meerkat-runtime). Each suite below is one
// lane,
//   nextest run -p <package> --test '*'
// on a pull request or main push whose directly changed packages include any
// of its triggers (a workspace-mode plan changes every package, so it runs
// every suite). The codegen parity tests compare the machine catalog with the
// runtime's and meerkat-mob's generated manifests, so the machine schema and
// DSL crates, the kernels, the runtime and meerkat-mob trigger them. Lane
// cost on 4 cores (measured on e20cc9ad9 after a meerkat-runtime change, deps
// cached): runtime 168 s, machine-codegen 396 s (meerkat-mob is compiled as
// a plain library, not its lib-test). Over the last 60 merges to main the
// runtime suite would have run on 17 and the codegen suite on 25.
// A suite whose package compiles meerkat-mob (the heavy chain) runs on the
// push only: on run 38859112297 machine-codegen took 838 s, gpt-live-replay
// 974 s and rpc-configured-turn-limit 1409 s, against 84-387 s for the
// suites outside the chain, which a pull request keeps.
const MACHINE_AUTHORITY_PACKAGES = [
  "meerkat-machine-schema",
  "meerkat-machine-dsl",
  "meerkat-machine-dsl-core",
  "meerkat-machine-derive",
  "meerkat-machine-kernels",
];
//
// A suite may also name `features` (a target behind a required feature
// makes `--test '*'` fail without it) and `paths`: changed-path prefixes
// that select it even when no Rust package changed. xtask's integration
// tests (ci_gate_requires_rmat and friends) pin the workflow files, so a
// workflow-only edit runs them; twice on 2026-10-01 a change merged that
// broke one with no PR lane running it. 163 tests, 169 s on 4 cores.
export const INTEGRATION_SUITES = [
  { package: "meerkat-runtime", triggers: [...MACHINE_AUTHORITY_PACKAGES, "meerkat-runtime"] },
  {
    package: "meerkat-machine-codegen",
    triggers: [...MACHINE_AUTHORITY_PACKAGES, "meerkat-runtime", "meerkat-mob", "meerkat-machine-codegen"],
  },
  // buildbuddy_static_lanes also reads the SDK manifests and generated types
  // and walks sdks/ (version parity, generated-type and banned-name checks).
  { package: "xtask", features: ["machine-authority"], triggers: ["xtask"], paths: [".github/workflows/", "sdks/"] },
  // kernel_typed_round_trip drives the generated kernels with hand-built
  // inputs, so any machine DSL change can break it; it needs `test-oracle`.
  { package: "meerkat-machine-kernels", features: ["test-oracle"], triggers: MACHINE_AUTHORITY_PACKAGES },
  // The facade's store and cold-restart suites, with the Bazel target's
  // features. No other lane ran them before a release tag: the Bazel graph
  // runs only nightly (on main) and at the tag, so a compaction plus
  // cold-restart store corruption reached a green release/0.8.51 (#1673).
  {
    package: "meerkat",
    name: "meerkat-store-restart",
    features: [
      "anthropic", "atif", "comms", "copilot", "gemini", "integration-real-tests", "jsonl-store", "live",
      "live-webrtc", "mcp", "memory-store", "memory-store-session", "openai", "openai-live", "openai-realtime",
      "schedule", "session-compaction", "session-store", "skills", "test-mcp-oauth-fixtures",
      "test-realtime-fixtures", "workgraph",
    ],
    tests: [
      "cold_restart_resume_after_compaction",
      "cold_restart_resume",
      "persistence_contract",
      "storage_provider_seam",
      "brain_swap_surface_parity",
    ],
    triggers: ["meerkat", "meerkat-core", "meerkat-store", "meerkat-session", "meerkat-runtime", "meerkat-sqlite"],
  },
  // A governed realm's composition: posture, restored grant owner, stock
  // composition and the realm's persisted MCP installations (the connection
  // control spawns the mcp-test-server fixture).
  {
    package: "meerkat",
    name: "meerkat-governed-realm",
    features: ["session-store", "local-authorization", "mcp"],
    tests: ["governed_realm"],
    triggers: ["meerkat", "meerkat-core", "meerkat-store", "meerkat-runtime", "meerkat-mcp"],
  },
  // These no-provider regressions exercise configured limits through the
  // factory and the RPC realm path. Select only their named test binaries.
  {
    package: "meerkat",
    name: "standalone-turn-lifecycle",
    tests: ["standalone_turn_lifecycle"],
    triggers: ["meerkat", "meerkat-core"],
  },
  {
    package: "meerkat-rpc",
    name: "rpc-configured-turn-limit",
    tests: ["configured_turn_limit"],
    triggers: ["meerkat-rpc", "meerkat", "meerkat-core"],
  },
  // Deterministic replays of recorded Turbo S gpt-live-1 provider streams
  // (tests/integration/fixtures/gpt_live_replay): the S104/S106 voice
  // contracts without a provider or a key. `tests` names the one target, so
  // the lane does not build every integration binary. Triggered by the
  // packages on the public Live path, its fixtures and its harness.
  {
    package: "meerkat-integration-tests",
    name: "gpt-live-replay",
    features: ["gpt-live-replay"],
    tests: ["gpt_live_replay"],
    triggers: [
      "meerkat",
      "meerkat-core",
      "meerkat-openai",
      "meerkat-live",
      "meerkat-runtime",
      "meerkat-session",
      "meerkat-mob",
      "meerkat-mob-mcp",
      "meerkat-rpc",
      "meerkat-integration-tests",
    ],
    paths: ["tests/integration/fixtures/gpt_live_replay/"],
  },
  // The ordinary native governed loops and cost correctness controls live in
  // tests/*.rs; unit rows only select lib/bin tests. Keep real/perf opt-ins.
  {
    package: "meerkat-authorization",
    triggers: [
      ...MACHINE_AUTHORITY_PACKAGES,
      "meerkat-authorization", "meerkat-authorization-contracts", "meerkat-core", "meerkat-runtime",
      "meerkat", "meerkat-tools", "meerkat-llm-core", "meerkat-anthropic", "meerkat-auth-core", "meerkat-models",
    ],
  },
];

// Packages whose own lib-test binary dominates their push-to-main lane. On
// the hosted 4-vCPU runners meerkat-mob's lanes took 12.5-20 min: 7.7-14.5
// min of cargo compile (one rustc unit, sccache-dependent) plus 4-5 min of
// nextest (measured on 6b2095989 and fe7fc7295). A single crate's compile
// cannot be split and nextest partitions that each rebuild it all miss the
// cache at once, so these packages build a nextest archive once (per
// default-feature lane and per feature suite) and fan the run out over
// `partitions` jobs that execute the archive with `--partition hash:k/n`.
export const ARCHIVED_UNIT_LANES = [{ package: "meerkat-mob", partitions: 2 }];

// Long rows first (see the header): a row measured at this many seconds or
// more, or never measured, is in the first wave.
export const WAVE_FIRST_SECONDS = 480;
export const ROW_DURATIONS = "scripts/ci-row-durations.json";
// The lanes planned in two waves, by their key in ROW_DURATIONS. A main
// unit row may carry a unit row's measurement: a push runs the unit lane's
// rows there.
export const WAVE_LANES = ["clippy", "main-unit", "closure-tests", "feature-check"];
const MEASURED_LANES = ["unit", ...WAVE_LANES];

// The measured row durations, `<lane>:<rowKey>` to whole seconds.
export function readRowDurations(path) {
  return checkRowDurations(JSON.parse(readFileSync(path, "utf8")), path);
}

// A row-duration table's rows, or an error: schema 2, every key a measured
// lane and a rowKey, whole positive seconds. The generator checks the table
// it writes with this too, so it cannot write one the planner refuses.
export function checkRowDurations(data, label) {
  if (data?.schema_version !== 2 || !data.rows || typeof data.rows !== "object" || Array.isArray(data.rows)) {
    throw new Error(`${label} is not a schema-2 row-duration table`);
  }
  for (const [key, seconds] of Object.entries(data.rows)) {
    if (!MEASURED_LANES.includes(key.slice(0, key.indexOf(":"))) || !Number.isInteger(seconds) || seconds <= 0) {
      throw new Error(`${label}: row ${JSON.stringify(key)} has no measured lane or no whole positive seconds`);
    }
    const row = key.slice(key.indexOf(":") + 1);
    if (rowKey(row) !== row) {
      throw new Error(`${label}: row ${JSON.stringify(key)} carries a member count, so it matches no row; regenerate the table with scripts/ci-row-durations.mjs`);
    }
  }
  return data.rows;
}

// The key a row is measured under: its name without the member count of a
// packed row ("+N" after the anchor that names it). Packing is by Rust line
// count, so a merge that moves line counts changes N but not the anchor:
// "core+12" is "core", "openai+8 bazel set" is "openai bazel set", and
// "tests meerkat 1/5+3" is "tests meerkat 1/5". A lane's rows have disjoint
// members, so their anchors, and their keys, differ.
export function rowKey(name) {
  return name.replace(/\+\d+(?= |$)/, "");
}

// A lane's rows in their two waves, by name.
export function waveOf(lane, rows, durations) {
  const names = rows.map((row) => row.name);
  if (new Set(names).size !== names.length) throw new Error(`internal error: ${lane} rows repeat a name`);
  const named = new Map();
  for (const name of names) {
    const key = rowKey(name);
    if (named.has(key)) {
      throw new Error(`internal error: ${lane} rows ${JSON.stringify(named.get(key))} and ${JSON.stringify(name)} share the measurement key ${JSON.stringify(key)}`);
    }
    named.set(key, name);
  }
  const lookup = lane === "main-unit" ? [lane, "unit"] : [lane];
  const wave = { first: [], rest: [] };
  for (const row of rows) {
    const seconds = lookup.map((name) => durations[`${name}:${rowKey(row.name)}`]).find((value) => value !== undefined);
    wave[seconds === undefined || seconds >= WAVE_FIRST_SECONDS ? "first" : "rest"].push(row.name);
  }
  return wave;
}

// No package of the chain runs its unit tests in the pull request.
// meerkat-rpc's default-feature row (its lib tests carry the MCP restart
// controls, governed_jsonl::tests::stock_governed_*) ran there as the one
// exception: alone it took 1801 s on run 38859112297 (1313 s of it building
// meerkat-mob and the rpc lib test), more than the whole pull-request
// budget. It runs in the push-to-main unit lanes with the rest of the chain.

// Feature-combination checks (#1687). Every Cargo lane builds a package with
// its default features (plus FEATURE_UNIT_SUITES and --all-features), but the
// Bazel graph builds each crate with the feature set the generated BUILD.bazel
// gives it, and the nightly feature matrix checks further combinations. That
// graph runs only nightly on main and at the release tag, so a break under one
// of those sets reached release/0.8.51 three times: meerkat-mob-mcp's lib tests
// under `openai-live` alone (#1595) and `meerkat-mob --no-default-features`
// (#1558) among them. For each changed package the plan adds compile-only
// checks under:
//   - its Bazel test feature set (every rust_test of a package carries the
//     same set), in one `check --lib --bins --tests` per clippy shard over the
//     shard's packages, as Bazel's own unified graph builds them;
//   - every other Bazel library or binary feature set it has (production
//     variants that strip `test-support`, the surface feature variants);
//   - the `$(CARGO) check` rows of the Make feature-matrix targets below.
// The rows follow the clippy shards, whose packing is already sized for a
// pull-request lane; each shard gives a Bazel-set job and a matrix job.
// They run on the push only: on run 38859112297 the 16 jobs of a
// whole-workspace plan took 119-1303 s, nine of them over 850 s, and a
// pull request leaves them to the push (deferred_to_push).
// Feature names come from the generated BUILD.bazel (kept fresh by the
// bazel-locks-freshness gate) and the Makefile, and are validated against
// cargo metadata, so a renamed feature fails the plan.
export const MAKE_FEATURE_MATRIX_TARGETS = ["test-minimal", "test-feature-matrix-lib"];

// Whole-workspace Clippy shards. A Clippy lane checks the workspace chain
// below its packages (from sccache since scripts/ci-clippy-sccache-wrapper)
// and lints its own packages. The long chains end in a few heads: measured
// on runs 38471093009, 38415750191 and 38020614570 and main push
// 38473833907, tools -> meerkat-mob -> mob-mcp -> rpc -> rkat finished
// 1169-1363 s into a lane. Packed by weight, those heads shared shards with
// other crates, and the pole was a shard that linted meerkat-mob's lib and
// its 55 test targets after the chain. In a whole-workspace plan each group
// below gets a shard of its own, so the pole is one head, and a head's
// dependencies are checked under the same feature set on every run (stable
// sccache keys); the other packages pack into CLIPPY_PACKED_SHARDS more. A
// changed-package plan keeps one shard per changed package up to
// --max-shards. Names are checked against cargo metadata, so a renamed head
// fails the plan instead of falling back to packing. In a pull-request plan
// the mob-chain packages lint their lib and bin targets only (their tests
// run on the push), so a group or a packing never mixes them with packages
// that lint every target: a mixed group splits in two, and the chain
// packages pack apart from the others.
export const CLIPPY_HEAD_SHARDS = [
  ["rkat"],
  ["meerkat-rpc"],
  ["meerkat-mob"],
  ["meerkat"],
  ["meerkat-mob-mcp", "meerkat-mob-pack", "meerkat-mob-adaptive"],
  ["meerkat-rest", "meerkat-mcp-server", "meerkat-web-runtime"],
  ["meerkat-integration-tests", "surface-build-fixtures"],
];

// The chain heads whose Clippy a pull request leaves to the push when it
// invalidates meerkat-mob's build: a workspace-mode plan, or a change to
// meerkat-mob or to a crate its library compiles. meerkat-mob then misses the
// cache, and each of these lanes compiles the whole chain below its package
// first: on runs 38859112297 and 38931641220, every target, rkat took
// 719-1114 s, meerkat-rpc 745-1158 s and meerkat-integration-tests (with
// surface-build-fixtures) 945-1057 s, and their lib targets alone sit on the
// same chain. A pull request that leaves meerkat-mob's build alone lints them
// (lib and bin targets), since the chain below them is cached. The push
// lints them in every case. Names are checked against cargo metadata.
export const MOB_CHAIN_CLIPPY_HEADS = ["rkat", "meerkat-integration-tests", "meerkat-rpc"];
export const MOB_CHAIN_INVALIDATED = "mob chain invalidated";
export const CLIPPY_PACKED_SHARDS = 5;

// The inputs of the two exact-profile lanes, which select exact lists of
// tests (the governed JSONL step in ci.yml, the CONFINEMENT_* filters and
// counts of `make test-mcp-confinement-positive`): the files that define the
// selected cases and the manifests whose features compile them in or out.
// A pull request that touches one runs that lane, so an added or removed
// case fails its exact selection there, not on the push after the merge (the
// v0.9.3 release merge added cases both push-only guards then refused). An
// entry ending in "/" is a directory. scripts/ci-guard-lane-inputs-selftest.mjs
// maps every selected case to the file defining it and fails when that file
// is missing here.
export const GUARD_LANE_INPUTS = {
  governed_jsonl: [
    "crates/meerkat-rpc/Cargo.toml",
    "crates/meerkat-rpc/src/governed_jsonl.rs",
    "crates/meerkat-rpc/src/governed_jsonl/",
  ],
  mcp_confinement_positive: [
    "crates/meerkat-mcp/Cargo.toml",
    "crates/meerkat-mcp/src/process_confinement_tests.rs",
    "crates/meerkat-sandbox/Cargo.toml",
    "crates/meerkat-sandbox/tests/compiled_confinement.rs",
    "crates/meerkat-sandbox/tests/process_confinement.rs",
    "crates/meerkat-tools/Cargo.toml",
    "crates/meerkat-tools/src/builtin/shell/confinement_tests.rs",
    "crates/meerkat/Cargo.toml",
    "crates/meerkat/src/process_custody.rs",
    "crates/meerkat/tests/command_hook_confinement.rs",
  ],
};

export function touchesGuardLane(lane, paths) {
  const inputs = GUARD_LANE_INPUTS[lane];
  if (!inputs) throw new Error(`unknown exact-profile lane ${lane}`);
  return paths.some((path) =>
    inputs.some((input) => (input.endsWith("/") ? path.startsWith(input) : path === input)),
  );
}

// The two lanes' definitions, beside their inputs: each lane's job in ci.yml,
// the confinement lane's CONFINEMENT_* assignments and recipe in the
// Makefile and the selection of the default nextest profile it runs, and the
// module declarations (the `mod` item and its attributes) that compile the
// selected cases in. A pull request that changes one
// between its base and head runs that lane. [path, extract]: extract(text) is
// the part of the file that defines the lane.
// scripts/ci-guard-lane-inputs-selftest.mjs reads the selections from these
// extracts and pins the declarations.
export const GUARD_LANE_DEFINITIONS = {
  governed_jsonl: [
    [".github/workflows/ci.yml", (text) => workflowJob(text, "governed-jsonl")],
    ["crates/meerkat-rpc/src/lib.rs", (text) => moduleDeclarations(text, "governed_jsonl")],
  ],
  mcp_confinement_positive: [
    [".github/workflows/ci.yml", (text) => workflowJob(text, "mcp-confinement-positive")],
    ["Makefile", confinementMakeDefinition],
    [".config/nextest.toml", nextestDefaultSelection],
    ["crates/meerkat-mcp/src/connection.rs", (text) => moduleDeclarations(text, "process_confinement_tests")],
    ["crates/meerkat-tools/src/builtin/shell/mod.rs", (text) => moduleDeclarations(text, "confinement_tests")],
    ["crates/meerkat/src/lib.rs", (text) => moduleDeclarations(text, "process_custody")],
  ],
};

// A workflow job: its `  id:` line up to the next job or top-level key,
// without blank lines and comment lines (those of its run scripts included),
// so an edit to a comment alone changes no definition.
export function workflowJob(text, id) {
  const lines = text.split("\n");
  const start = lines.indexOf(`  ${id}:`);
  if (start < 0) return null;
  const end = lines.findIndex((line, index) => index > start && /^ {0,2}\S/.test(line));
  return lines
    .slice(start, end < 0 ? lines.length : end)
    .filter((line) => !/^\s*(#.*)?$/.test(line))
    .join("\n");
}

// The confinement lane in the Makefile: every CONFINEMENT_* assignment and
// the test-mcp-confinement-positive rule with its recipe.
export function confinementMakeDefinition(text) {
  const lines = text.split("\n");
  const start = lines.findIndex((line) => line.startsWith("test-mcp-confinement-positive:"));
  if (start < 0) return null;
  const end = lines.findIndex((line, index) => index > start && !line.startsWith("\t"));
  const assignments = lines.filter((line) => /^CONFINEMENT_\w+\s*[:?+]?=/.test(line));
  return [...assignments, ...lines.slice(start, end < 0 ? lines.length : end)].join("\n");
}

// What decides the default nextest profile's selection: its
// [profile.default] table, every default override that sets a (platform)
// default-filter, and the whole [profile] table or top-level section once
// one of its keys reaches the default profile (so a dotted key's multi-line
// string or array keeps its continuation lines), whatever the spelling
// (whitespace or quotes in headers and keys, indented keys), without comment
// and blank lines.
export function nextestDefaultSelection(text) {
  const bare = (part) => part.replace(/#.*/, "").replace(/["'\s]/g, "");
  const key = (line) => bare(line.split("=")[0]);
  const sections = [{ header: "", lines: [] }];
  for (const line of text.split("\n")) {
    if (/^\s*\[\[?[\w.\s"'-]+\]\]?\s*(#.*)?$/.test(line)) sections.push({ header: bare(line), lines: [line] });
    else sections.at(-1).lines.push(line);
  }
  const reachesDefault = { "[profile]": /^default(\.|$)/, "": /^profile(\.default(\.|$)|$)/ };
  return sections
    .flatMap(({ header, lines }) => {
      if (header === "[profile.default]") return lines;
      if (header === "[[profile.default.overrides]]") return lines.some((line) => key(line) === "default-filter") ? lines : [];
      if (Object.hasOwn(reachesDefault, header)) return lines.some((line) => reachesDefault[header].test(key(line))) ? lines : [];
      return [];
    })
    .filter((line) => !/^\s*(#.*)?$/.test(line))
    .join("\n");
}

// Every declaration of module `name` in a Rust source: the `mod` line and the
// attribute lines directly above it (up to a blank or comment line, or a line
// that ends an item or opens a block).
export function moduleDeclarations(text, name) {
  const lines = text.split("\n");
  const item = new RegExp(`^\\s*(pub(\\([^)]*\\))?\\s+)?mod\\s+${name}\\b`);
  return lines
    .flatMap((line, at) => {
      if (!item.test(line)) return [];
      let start = at;
      while (start > 0 && !/^\s*(\/\/.*)?$|[;{}]\s*$/.test(lines[start - 1])) start -= 1;
      return [lines.slice(start, at + 1).join("\n")];
    })
    .join("\n");
}

// The exact-profile lanes whose definition differs between the diff's base
// and head; a side that cannot be read differs. An explicit path list
// compares --base-tree's copy (without one, every listed definition file
// differs) with the working tree.
function changedGuardDefinitions(args, changedPaths) {
  const read = (side, path) => {
    if (args.explicitPaths) {
      const file = side === "head" ? resolve(root, path) : args.baseTree && resolve(args.baseTree, path);
      return file && existsSync(file) ? readFileSync(file, "utf8") : null;
    }
    const result = spawnSync("git", ["show", `${args[side]}:${path}`], { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    return result.status === 0 ? result.stdout : null;
  };
  return Object.entries(GUARD_LANE_DEFINITIONS)
    .filter(([, parts]) =>
      parts.some(([path, extract]) => {
        if (!changedPaths.includes(path)) return false;
        const [base, head] = [read("base", path), read("head", path)];
        return base === null || head === null || extract(base) !== extract(head);
      }),
    )
    .map(([lane]) => lane);
}

function parseBazelFeatureSets(pkg) {
  const buildPath = resolve(root, packageDir(pkg), "BUILD.bazel");
  if (!existsSync(buildPath)) {
    throw new Error(`no generated BUILD.bazel for ${pkg.name} (${relative(root, buildPath)})`);
  }
  const text = readFileSync(buildPath, "utf8");
  const known = new Set(Object.keys(pkg.features ?? {}));
  const testSets = new Set();
  const variants = new Map();
  for (const [, rule, body] of text.matchAll(/^(rust_test|rust_library|rust_binary)\(\n([\s\S]*?)^\)/gm)) {
    // A production target selects its release feature set (no test-only
    // features) under //tools/bazel:release_build; each select branch is a
    // feature set the Bazel graph builds, so each is checked.
    const value = body.match(/^ {4}crate_features = (\[[\s\S]*?\]|select\(\{[\s\S]*?\}\)),?$/m)?.[1];
    const lists = !value
      ? [[]]
      : value.startsWith("select(")
      ? [...value.matchAll(/"[^"]+":\s*\[([\s\S]*?)\]/g)].map((branch) => branch[1])
      : [value.slice(1, -1)];
    if (value?.startsWith("select(") && lists.length < 2) {
      throw new Error(`${relative(root, buildPath)} has a crate_features select() without two feature sets`);
    }
    for (const list of lists) {
      // Weak dependency features (`dep?/feature`) are rustc cfgs the generator
      // passes through; Cargo enables them from the package's own features.
      const features = [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]).filter((name) => !name.includes("/"));
      for (const feature of features) {
        if (!known.has(feature)) {
          throw new Error(`${relative(root, buildPath)} enables ${feature}, which ${pkg.name} does not declare`);
        }
      }
      const key = [...new Set(features)].sort().join(",");
      if (rule === "rust_test") {
        testSets.add(key);
      } else {
        const variant = variants.get(key) ?? { lib: false, bins: false };
        if (rule === "rust_library") variant.lib = true;
        if (rule === "rust_binary") variant.bins = true;
        variants.set(key, variant);
      }
    }
  }
  if (testSets.size > 1) {
    throw new Error(
      `${relative(root, buildPath)} gives ${pkg.name}'s tests ${testSets.size} feature sets; the feature-combination check expects one`,
    );
  }
  const testSet = testSets.size === 1 ? [...testSets][0] : null;
  if (testSet !== null) variants.delete(testSet);
  const split = (key) => (key === "" ? [] : key.split(","));
  return {
    test: testSet === null ? null : split(testSet),
    variants: [...variants]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, variant]) => ({ features: split(key), ...variant })),
  };
}

function parseMakeFeatureMatrix(byName) {
  const text = readFileSync(resolve(root, "Makefile"), "utf8");
  const rows = [];
  for (const target of MAKE_FEATURE_MATRIX_TARGETS) {
    const start = text.match(new RegExp(`^${target}:.*\\n((?:\\t.*\\n|#.*\\n)*)`, "m"));
    if (!start) throw new Error(`Makefile has no ${target} target`);
    let count = 0;
    for (const line of start[1].split("\n")) {
      const check = line.match(/^\t\$\(CARGO\) check (.+)$/);
      if (!check) continue;
      const args = check[1].trim();
      const pkgName = args.match(/(?:^|\s)-p (\S+)/)?.[1];
      if (!pkgName || !byName.has(pkgName)) {
        throw new Error(`Makefile ${target} row names no known package: ${args}`);
      }
      const features = args.match(/--features (\S+)/)?.[1]?.split(",") ?? [];
      for (const feature of features) {
        const [owner, name] = feature.includes("/") ? feature.split("/", 2) : [pkgName, feature];
        const ownerPkg = byName.get(owner);
        if (!ownerPkg || !Object.hasOwn(ownerPkg.features, name)) {
          throw new Error(`Makefile ${target} row enables unknown feature ${feature}: ${args}`);
        }
      }
      rows.push({ target, package: pkgName, args });
      count += 1;
    }
    if (count === 0) throw new Error(`Makefile ${target} has no $(CARGO) check rows`);
  }
  return rows;
}

function targetFlags(pkgs, { tests }) {
  const kinds = new Set(pkgs.flatMap((pkg) => pkg.targets.flatMap((target) => target.kind)));
  const flags = [];
  if (["lib", "rlib", "cdylib", "dylib", "staticlib", "proc-macro"].some((kind) => kinds.has(kind))) flags.push("--lib");
  if (kinds.has("bin")) flags.push("--bins");
  if (tests) flags.push("--tests");
  return flags;
}

function bazelTestRow(pkgs, sets) {
  const withTests = pkgs.filter((pkg) => sets.get(pkg.name).test !== null);
  if (withTests.length === 0) return null;
  const features = withTests.flatMap((pkg) => sets.get(pkg.name).test.map((feature) => `${pkg.name}/${feature}`));
  return [
    ...withTests.map((pkg) => `-p ${pkg.name}`),
    "--no-default-features",
    ...(features.length > 0 ? [`--features ${features.join(",")}`] : []),
    ...targetFlags(withTests, { tests: true }),
  ].join(" ");
}

function featureChecks(shards, byName) {
  const changedNames = shards.flatMap((shard) => shard.packages);
  const sets = new Map(changedNames.map((name) => [name, parseBazelFeatureSets(byName.get(name))]));
  const makeRows = parseMakeFeatureMatrix(byName);
  // Each shard gives two jobs that run in parallel: its Bazel test-set check
  // (one invocation over the shard, 534-796 s cold on #1763's workspace run)
  // and its other rows (variants and Make matrix rows, up to ~1000 s cold).
  // In one job they reached 1568 s, over the pull-request lane budget.
  return shards.flatMap((shard) => {
    const pkgs = shard.packages.map((name) => byName.get(name));
    const bazel = bazelTestRow(pkgs, sets);
    const commands = [];
    const add = (args) => {
      if (args && !commands.includes(args)) commands.push(args);
    };
    for (const pkg of pkgs) {
      for (const variant of sets.get(pkg.name).variants) {
        add([
          `-p ${pkg.name}`,
          "--no-default-features",
          ...(variant.features.length > 0 ? [`--features ${variant.features.join(",")}`] : []),
          ...(variant.lib ? ["--lib"] : []),
          ...(variant.bins ? ["--bins"] : []),
        ].join(" "));
      }
      for (const row of makeRows) {
        if (row.package === pkg.name) add(row.args);
      }
    }
    return [
      { name: `${shard.name} bazel set`, packages: shard.packages, commands: bazel ? [bazel] : [] },
      { name: `${shard.name} matrix`, packages: shard.packages, commands },
    ].filter((job) => job.commands.length > 0);
  });
}

function estimatedMinutes(cost) {
  return Math.round((LANE_SETUP_MINUTES + cost / COST_UNITS_PER_MINUTE) * 10) / 10;
}

function parseArgs(argv) {
  const args = {
    base: "",
    head: "HEAD",
    maxShards: 6,
    workspaceShards: 8,
    closureTestRows: 12,
    libRowRunner: "",
    format: "json",
    pathsFromStdin: false,
    paths: [],
    explicitPaths: false,
    changedPathsOnly: false,
    releaseEvidence: "",
    workspacePlanRequested: false,
    event: "push",
    baseLock: "",
    baseTree: "",
    rowDurations: "",
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--base") {
      args.base = argv[++i] ?? "";
    } else if (arg === "--head") {
      args.head = argv[++i] ?? "HEAD";
    } else if (arg === "--event") {
      // A pull request plans its own lanes; every other event (a push, a
      // manual dispatch) plans the push lanes, which are the whole set.
      const event = argv[++i] ?? "";
      if (!["pull_request", "push", "workflow_dispatch"].includes(event)) {
        throw new Error("--event must be pull_request, push or workflow_dispatch");
      }
      args.event = event === "pull_request" ? "pull_request" : "push";
    } else if (arg === "--row-durations") {
      args.rowDurations = argv[++i] ?? "";
      if (!args.rowDurations) throw new Error("--row-durations requires a file");
    } else if (arg === "--base-lock") {
      args.baseLock = argv[++i] ?? "";
      if (!args.baseLock) throw new Error("--base-lock requires a file");
    } else if (arg === "--base-tree") {
      args.baseTree = argv[++i] ?? "";
      if (!args.baseTree) throw new Error("--base-tree requires a directory");
    } else if (arg === "--max-shards") {
      args.maxShards = Number.parseInt(argv[++i] ?? "", 10);
      if (!Number.isInteger(args.maxShards) || args.maxShards < 1) {
        throw new Error("--max-shards requires a positive integer");
      }
    } else if (arg === "--workspace-shards") {
      args.workspaceShards = Number.parseInt(argv[++i] ?? "", 10);
      if (!Number.isInteger(args.workspaceShards) || args.workspaceShards < 1) {
        throw new Error("--workspace-shards requires a positive integer");
      }
    } else if (arg === "--closure-test-rows") {
      args.closureTestRows = Number.parseInt(argv[++i] ?? "", 10);
      if (!Number.isInteger(args.closureTestRows) || args.closureTestRows < 1) {
        throw new Error("--closure-test-rows requires a positive integer");
      }
    } else if (arg === "--lib-row-runner") {
      args.libRowRunner = argv[++i] ?? "";
      if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(args.libRowRunner)) {
        throw new Error("--lib-row-runner requires a runner label");
      }
    } else if (arg === "--format") {
      args.format = argv[++i] ?? "";
      if (!["json", "github"].includes(args.format)) {
        throw new Error("--format must be json or github");
      }
    } else if (arg === "--changed-paths-only") {
      args.changedPathsOnly = true;
    } else if (arg === "--reuse-release-evidence") {
      args.releaseEvidence = argv[++i] ?? "";
      parseEvidenceArgument(args.releaseEvidence);
    } else if (arg === "--workspace-plan-requested") {
      args.workspacePlanRequested = true;
    } else if (arg === "--paths-from-stdin") {
      args.pathsFromStdin = true;
      args.explicitPaths = true;
    } else if (arg === "--") {
      args.paths.push(...argv.slice(i + 1));
      args.explicitPaths = true;
      break;
    } else if (arg === "--help" || arg === "-h") {
      process.stdout.write(
        "usage: ci-cargo-lanes.mjs [--base <rev> --head <rev>] [--max-shards N] [--workspace-shards N] [--closure-test-rows N] [--lib-row-runner LABEL] [--event pull_request|push|workflow_dispatch] [--base-lock FILE] [--base-tree DIR] [--reuse-release-evidence <commit>:<run id>:<attempt>] [--workspace-plan-requested] [--format json|github] [--paths-from-stdin] [--changed-paths-only] [--] [changed-path ...]\n",
      );
      process.exit(0);
    } else if (arg.startsWith("-")) {
      throw new Error(`unknown argument: ${arg}`);
    } else {
      args.paths.push(arg);
      args.explicitPaths = true;
    }
  }
  return args;
}

function git(argsList, options = {}) {
  return execFileSync("git", argsList, { cwd: root, encoding: "utf8", ...options });
}

function revisionExists(rev) {
  const result = spawnSync("git", ["cat-file", "-e", `${rev}^{commit}`], { cwd: root });
  return result.status === 0;
}

// Returns { paths, reason }. `paths` is null when the diff cannot be
// established, which the caller treats as "everything changed".
function collectChangedPaths(args) {
  if (args.pathsFromStdin) {
    const text = readFileSync(0, "utf8");
    const paths = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    const all = [...paths, ...args.paths];
    return { paths: all, removed: explicitRemoved(all), reason: "explicit path list" };
  }
  if (args.explicitPaths) {
    return { paths: args.paths, removed: explicitRemoved(args.paths), reason: "explicit path list" };
  }
  if (!args.base || args.base === NULL_SHA) {
    return { paths: null, reason: "no diff base supplied" };
  }
  if (!revisionExists(args.base)) {
    const fetched = spawnSync("git", ["fetch", "--no-tags", "--depth=1", "origin", args.base], {
      cwd: root,
      stdio: "inherit",
    });
    if (fetched.status !== 0 || !revisionExists(args.base)) {
      return { paths: null, reason: `diff base ${args.base} is not available` };
    }
  }
  if (!revisionExists(args.head)) {
    return { paths: null, reason: `diff head ${args.head} is not available` };
  }
  // --no-renames lists both sides of a rename (its old side as a deletion),
  // and T keeps type-only changes, such as a source file turned symlink.
  // Either can be the only path that names a package: a test moved from
  // crate A to crate B must still select A, and a source file renamed into
  // docs/ must still select its crate. -z keeps every name exact.
  const output = git([
    "diff", "--no-renames", "--name-only", "-z", "--diff-filter=ACMRDT", args.base, args.head, "--",
  ]);
  // Deleted paths and the old side of renames: a moved or removed source file
  // changes the generated Bazel data dependencies even when no BUILD file is
  // in the diff.
  const statusOutput = git(["diff", "--name-status", "--find-renames", "--diff-filter=RD", args.base, args.head, "--"]);
  const removed = [];
  for (const line of statusOutput.split(/\r?\n/)) {
    const fields = line.split("\t");
    if (fields.length >= 2 && fields[0].startsWith("D")) removed.push(fields[1].trim());
    if (fields.length >= 3 && fields[0].startsWith("R")) removed.push(fields[1].trim());
  }
  return {
    paths: output.split("\0").filter(Boolean),
    removed,
    reason: `git diff ${args.base}..${args.head}`,
  };
}

// For an explicit path list (self-test fixtures, local dry runs) a listed path
// that no longer exists in the working tree is a deletion or the old side of a
// move.
function explicitRemoved(paths) {
  return paths.filter((path) => !existsSync(resolve(root, path)));
}

// A Cargo.lock as written by Cargo: header lines (`version = N` and
// comments) before the first table, then `[[package]]` tables of `key =
// "string"` and `key = [ "string", ... ]` lines. Entries are keyed by name,
// version and source (a path package has none). Any other table is kept as
// raw text, so a change to it can be refused rather than read. Anything the
// parser does not recognize throws.
export function parseCargoLock(text) {
  const header = [];
  const others = [];
  const entries = new Map();
  let table = null;
  let array = null;
  const finish = () => {
    if (table?.kind !== "package") return;
    const entry = table.fields;
    if (typeof entry.name !== "string" || typeof entry.version !== "string") {
      throw new Error("a [[package]] entry has no name or version");
    }
    const key = lockEntryKey(entry);
    if (entries.has(key)) throw new Error(`duplicate lock entry ${key}`);
    entries.set(key, entry);
  };
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "");
    if (array) {
      if (/^\]$/.test(line.trim())) {
        array = null;
        continue;
      }
      const item = /^\s*"([^"]*)",?$/.exec(line);
      if (!item) throw new Error(`unreadable lock array line: ${JSON.stringify(line)}`);
      array.push(item[1]);
      continue;
    }
    const tableHeader = /^\[\[?([A-Za-z0-9_.-]+)\]\]?$/.exec(line);
    if (tableHeader) {
      finish();
      table = tableHeader[1] === "package" && line.startsWith("[[")
        ? { kind: "package", fields: {} }
        : { kind: "other", text: [line] };
      if (table.kind === "other") others.push(table.text);
      continue;
    }
    if (table === null) {
      if (line.trim() !== "") header.push(line);
      continue;
    }
    if (table.kind === "other") {
      table.text.push(line);
      continue;
    }
    if (line.trim() === "") continue;
    const scalar = /^([a-z_-]+) = "([^"]*)"$/.exec(line);
    if (scalar) {
      table.fields[scalar[1]] = scalar[2];
      continue;
    }
    const inline = /^([a-z_-]+) = \[(.*)\]$/.exec(line);
    if (inline) {
      table.fields[inline[1]] = inline[2].trim() === ""
        ? []
        : inline[2].split(",").map((item) => item.trim()).filter(Boolean).map((item) => {
          const quoted = /^"([^"]*)"$/.exec(item);
          if (!quoted) throw new Error(`unreadable lock array item: ${JSON.stringify(item)}`);
          return quoted[1];
        });
      continue;
    }
    const open = /^([a-z_-]+) = \[$/.exec(line);
    if (open) {
      array = [];
      table.fields[open[1]] = array;
      continue;
    }
    throw new Error(`unreadable lock line: ${JSON.stringify(line)}`);
  }
  if (array) throw new Error("the lock ends inside an array");
  finish();
  return { header: header.join("\n"), others: others.map((lines) => lines.join("\n").trimEnd()), entries };
}

function lockEntryKey(entry) {
  return `${entry.name} ${entry.version}${entry.source ? ` (${entry.source})` : ""}`;
}

// The workspace members a Cargo.lock change can reach (see the header):
// every entry the diff added, removed or changed, walked up the head lock's
// own dependency lists (all features, all targets, the members' dev
// dependencies) to the members that depend on it. Throws when the change
// cannot be scoped; the caller then plans the whole workspace.
export function lockScope(baseText, headText, memberNames) {
  const base = parseCargoLock(baseText);
  const head = parseCargoLock(headText);
  if (base.header !== head.header) throw new Error("the lock header changed");
  if (JSON.stringify(base.others) !== JSON.stringify(head.others)) {
    throw new Error("a lock table other than [[package]] changed");
  }
  const body = (entry) => JSON.stringify(entry, Object.keys(entry).sort());
  const changed = [];
  for (const [key, entry] of head.entries) {
    const before = base.entries.get(key);
    if (!before || body(before) !== body(entry)) changed.push(key);
  }
  const removed = [...base.entries.keys()].filter((key) => !head.entries.has(key));
  if (changed.length === 0 && removed.length === 0) {
    throw new Error("the lock changed bytes but no package entry");
  }
  // A dependency is written as `name`, `name version` or `name version
  // (source)`, whichever is unique in the lock.
  const byName = new Map();
  for (const [key, entry] of head.entries) byName.set(entry.name, [...(byName.get(entry.name) ?? []), key]);
  const resolveDependency = (reference) => {
    const match = /^(\S+)(?: (\S+))?(?: \((.+)\))?$/.exec(reference);
    const candidates = (match ? byName.get(match[1]) ?? [] : []).filter((key) => {
      const entry = head.entries.get(key);
      return (!match[2] || entry.version === match[2]) && (!match[3] || entry.source === match[3]);
    });
    if (candidates.length !== 1) throw new Error(`lock dependency ${JSON.stringify(reference)} names ${candidates.length} entries`);
    return candidates[0];
  };
  const dependents = new Map();
  for (const [key, entry] of head.entries) {
    for (const reference of entry.dependencies ?? []) {
      const dependency = resolveDependency(reference);
      dependents.set(dependency, [...(dependents.get(dependency) ?? []), key]);
    }
  }
  // A removed entry is reached through the entries whose dependency lists
  // dropped it, which changed with it.
  const seen = new Set(changed);
  const queue = [...changed];
  for (let i = 0; i < queue.length; i += 1) {
    for (const dependent of dependents.get(queue[i]) ?? []) {
      if (!seen.has(dependent)) {
        seen.add(dependent);
        queue.push(dependent);
      }
    }
  }
  const members = [...seen]
    .map((key) => head.entries.get(key))
    .filter((entry) => !entry.source && memberNames.has(entry.name))
    .map((entry) => entry.name);
  if (members.length === 0) throw new Error("no workspace member depends on a changed lock entry");
  return { members: [...new Set(members)].sort(), changed: changed.sort(), removed: removed.sort() };
}

// The lock text on each side of the diff: the base revision's (or
// --base-lock's) and the head revision's (or the working tree's, for an
// explicit path list). Null when a side cannot be read.
function lockTexts(args) {
  const read = (rev) => {
    const result = spawnSync("git", ["show", `${rev}:Cargo.lock`], { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    return result.status === 0 ? result.stdout : null;
  };
  const fromFile = (path) => {
    try {
      return readFileSync(resolve(root, path), "utf8");
    } catch {
      return null;
    }
  };
  if (args.baseLock) return { base: fromFile(args.baseLock), head: args.explicitPaths ? fromFile("Cargo.lock") : read(args.head) };
  if (args.explicitPaths || !args.base) return null;
  return { base: read(args.base), head: read(args.head) };
}

// The CI workflow and its classifier define every lane, so a change to them
// runs every lane they define, as the Rust lanes do through isGlobalPath.
function isLaneDefinitionPath(path) {
  return (
    path === ".github/workflows/ci.yml" ||
    path === "scripts/ci-cargo-lanes.mjs" ||
    path === "scripts/ci-cargo-lanes-selftest.mjs" ||
    path === "scripts/ci-hook-coverage.mjs" ||
    path === "scripts/ci-verify-row-binaries.mjs" ||
    path === "scripts/ci-non-rust-readers.mjs" ||
    path === "scripts/ci-release-evidence.mjs" ||
    path === "scripts/ci-release-projection-selftest.mjs" ||
    path === "scripts/ci-push-base.mjs" ||
    path === "scripts/ci-push-plan-base.mjs" ||
    path === "scripts/ci-push-base-selftest.mjs" ||
    path === "scripts/release-projection-only.mjs" ||
    path === "scripts/release-version-pins.mjs"
  );
}

// Paths whose change can alter the Bazel graph without touching Rust code:
// generated or hand-written BUILD files, Starlark, module and lock inputs,
// the BUILD generator and the modules and lists it reads, the Bazel
// toolchain and lane definitions, and every Cargo manifest and the lockfile
// (crate_universe reads them).
function isBazelGraphPath(path) {
  const base = path.split("/").pop();
  return (
    isLaneDefinitionPath(path) ||
    base === "BUILD.bazel" ||
    base === "BUILD" ||
    path.endsWith(".bzl") ||
    path === "MODULE.bazel" ||
    path === "MODULE.bazel.lock" ||
    path === ".bazelrc" ||
    path === ".bazelversion" ||
    path === ".bazelignore" ||
    path === "Cargo.lock" ||
    base === "Cargo.toml" ||
    path === "scripts/generate-bazel-rust-builds.mjs" ||
    path === "scripts/rust-source-text.mjs" ||
    path === "scripts/rust-test-selector.mjs" ||
    path === "scripts/non-shipping-features.json" ||
    path.startsWith("tools/bazel/") ||
    path.startsWith("tools/buildbuddy/")
  );
}

// Node-only example suites: examples/037-live-webrtc-web (Playwright
// Chromium) and the examples SDK suite, which links sdks/typescript and runs
// the Python SDK examples against a fake host. Markdown is not an input.
function isExamplesBrowserPath(path) {
  if (isLaneDefinitionPath(path)) return true;
  if (path.startsWith("sdks/typescript/") || path.startsWith("sdks/python/")) return true;
  return path.startsWith("examples/") && !/\.(md|mdx)$/.test(path);
}

// Example web suites: examples 031, 032 and 033 execute the sdks/web wasm
// runtime in Chromium, so the runtime build and its inputs select them along
// with the example sources. Markdown is not an input.
const EXAMPLE_WEB_DIRS = [
  "examples/031-wasm-mini-diplomacy-sh/",
  "examples/032-wasm-webcm-agent/",
  "examples/033-the-office-demo-sh/",
];
function isExampleWebPath(path) {
  if (isLaneDefinitionPath(path)) return true;
  if (
    path.startsWith("sdks/web/") ||
    path.startsWith("crates/meerkat-web-runtime/") ||
    path.startsWith("crates/meerkat-contracts/")
  ) {
    return true;
  }
  return EXAMPLE_WEB_DIRS.some((dir) => path.startsWith(dir)) && !/\.(md|mdx)$/.test(path);
}

// The typed governance gates (effect-authority, ownership-ledger, strict
// RMAT, strict seam inventory, runtime authority bypass, machine-authority
// docs) run on the agent gate's governance paths (scripts/cargo-agent-gate,
// is_governance_path): the machine-authority paths and these dogma
// documents. The self-test keeps the two lists equal.
export const GOVERNANCE_DOC_PATTERNS = [
  ".claude/skills/meerkat-dogma-inquisition/*",
  "docs/architecture/meerkat-dogma.md",
  "docs/architecture/meerkat-dogma-commentary.md",
  "docs/internal/dogma-audits/*",
];
function isGovernanceDocPath(path) {
  return GOVERNANCE_DOC_PATTERNS.some((pattern) =>
    pattern.endsWith("/*") ? path.startsWith(pattern.slice(0, -1)) : path === pattern,
  );
}

// Paths whose change alters the compile graph of every package.
function isGlobalPath(path) {
  if (
    path === "Cargo.toml" ||
    path === "Cargo.lock" ||
    path === "rust-toolchain.toml" ||
    path === "rust-toolchain" ||
    path === "Makefile" ||
    path === ".config/nextest.toml" ||
    path === "scripts/repo-cargo" ||
    path === "scripts/agent-gate" ||
    path === "scripts/rust-lane-doctor" ||
    path === "scripts/ci-cargo-lanes.mjs" ||
    path === "scripts/ci-cargo-lanes-selftest.mjs" ||
    path === "scripts/ci-hook-coverage.mjs" ||
    path === "scripts/ci-verify-row-binaries.mjs" ||
    path === "scripts/ci-non-rust-readers.mjs" ||
    path === "scripts/ci-release-evidence.mjs" ||
    path === "scripts/ci-release-projection-selftest.mjs" ||
    path === "scripts/ci-push-base.mjs" ||
    path === "scripts/ci-push-plan-base.mjs" ||
    path === "scripts/ci-push-base-selftest.mjs" ||
    path === "scripts/release-projection-only.mjs" ||
    path === "scripts/release-version-pins.mjs" ||
    path === "scripts/rust-test-selector.mjs" ||
    path === "scripts/ci-pin-rust-toolchain-env" ||
    path === "scripts/ci-clippy-sccache-wrapper" ||
    path === ".github/workflows/ci.yml"
  ) {
    return true;
  }
  return (
    path.startsWith(".cargo/") ||
    path.startsWith("scripts/cargo-") ||
    path.startsWith(".github/actions/setup-rust-ci/")
  );
}

function isRustSourcePath(path) {
  return path.endsWith(".rs") || path.endsWith("/Cargo.toml") || path === "Cargo.toml";
}

function runClassifier(script, paths) {
  if (paths.length === 0) return false;
  const result = spawnSync(join(root, script), ["--", ...paths], {
    cwd: root,
    encoding: "utf8",
  });
  if (result.status === 0) return true;
  if (result.status === 1) return false;
  throw new Error(
    `${script} failed (status ${result.status}): ${result.stderr || result.stdout || "no output"}`,
  );
}

function rustLineCount(dir) {
  let lines = 0;
  const stack = [dir];
  while (stack.length) {
    const current = stack.pop();
    let entries;
    try {
      entries = readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (entry.name === "target" || entry.name === "node_modules" || entry.name.startsWith(".")) continue;
      const full = join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(full);
      } else if (entry.isFile() && entry.name.endsWith(".rs")) {
        try {
          const text = readFileSync(full, "utf8");
          lines += text.split("\n").length;
        } catch {
          // Unreadable sources are reported by rustc, not by the planner.
        }
      }
    }
  }
  return lines;
}

function shortName(name) {
  return name.startsWith("meerkat-") ? name.slice("meerkat-".length) : name;
}

// In-crate documentation a crate compiles in (see the header): a reader of
// package scope whose .md or .mdx input lies in the reader's own crate
// directory. Another crate's reader of the same file is an ordinary reader.
export function compiledInDocKeys(entries, ownerOf) {
  return new Set(
    entries
      .filter((entry) => entry.scope === "package" && /\.(md|mdx)$/.test(entry.path) && ownerOf(entry.path) === entry.package)
      .map((entry) => `${entry.path}\0${entry.package}`),
  );
}

// Features whose tests need live-provider credentials: the live gates run
// them on demand, no CI lane does.
export const LIVE_ONLY_FEATURES = ["integration-real-tests", "openai-live-e2e"];

// The reader rows of a plan (see the header): for each reader selection
// (scripts/ci-non-rust-readers.mjs) the targets no other lane of the plan
// runs, as one integration-lane row. A pull request leaves the readers that
// compile meerkat-mob to the push (named in `deferred.reader_rows`). The
// integration lane waits for ci.yml's stagger job (the rest wave), so a
// reader row, such as the whole-checkout one every push plans, never takes
// a runner before the long rows.
export function readerRows({ selections, result, byName, heavyChain, deferToPush, deferred, compiledInDocs, unified, canary = null }) {
  const mainUnit = result.main_unit_shards.length > 0;
  // The unit lane of this plan runs the package's lib and bins (default
  // features): a pull request's unit lane, a push's main unit lane (every
  // package) or a release projection's reader unit rows.
  const unitRuns = (name) =>
    deferToPush
      ? result.unit_packages.includes(name)
      : mainUnit || result.unit_shards.some((shard) => shard.packages.includes(name));
  // A selected integration suite runs the package's test targets (all, or
  // the ones it names) with its features.
  const suiteRuns = (name, test, features) =>
    result.integration_suites.some((suite) => {
      if (suite.packages[0] !== name) return false;
      const words = suite.package_flags.split(" ");
      const suiteFeatures = words.includes("--features") ? words[words.indexOf("--features") + 1].split(",") : [];
      if (!features.every((feature) => suiteFeatures.includes(feature))) return false;
      const named = (suite.test_flags ?? "").split(" ").filter((_, i, all) => all[i - 1] === "--test");
      return named.length === 0 || (test !== null && named.includes(test));
    });
  // The push's closure-tests rows build each test-scope package with the
  // features a whole-workspace build resolves; Cargo drops a target whose
  // required features are off.
  const closureBuilds = (selection) => selection.features.every((feature) => unified.get(selection.package)?.has(feature));
  const notRun = [];
  const rows = [];
  for (const selection of selections) {
    // Documentation the package compiles in selects the package like its
    // code (see the header): its own lanes run it.
    if (selection.reads.every((key) => compiledInDocs.has(`${key}\0${selection.package}`))) continue;
    const pkg = byName.get(selection.package);
    const colon = selection.target.indexOf(":");
    const kind = colon < 0 ? selection.target : selection.target.slice(0, colon);
    const name = colon < 0 ? null : selection.target.slice(colon + 1);
    if (name !== null && !pkg.targets.some((target) => target.name === name && target.kind.includes(kind))) {
      throw new Error(`the non-Rust reader inventory names unknown target ${selection.target} of ${selection.package}`);
    }
    // Cargo refuses --lib for a package without a library.
    if (kind === "lib" && !targetFlags([pkg], { tests: false }).includes("--lib")) {
      throw new Error(`the non-Rust reader inventory names the library of ${selection.package}, which has none`);
    }
    // Library, binary or build-script code is reached by every test.
    const whole = selection.scope === "package" || kind === "build";
    const label = `${shortName(selection.package)}:${whole ? "package" : selection.target}`;
    // A reader keyed only by the whole checkout is a live audit that every
    // path selects: a pull request leaves it to the push, whose
    // closure-tests rows run its package's test targets with the features
    // they build. A test target that needs other features gets a row there.
    if (selection.reads.every((key) => key === WHOLE_CHECKOUT || compiledInDocs.has(`${key}\0${selection.package}`))) {
      if (deferToPush || kind !== "test" || closureBuilds(selection) || suiteRuns(selection.package, name, selection.features)) continue;
    }
    if (selection.features.some((feature) => LIVE_ONLY_FEATURES.includes(feature))) {
      notRun.push(label);
      continue;
    }
    const unitCovered = selection.features.length === 0 && unitRuns(selection.package);
    let targets;
    let testsDeferred = false;
    let tests = selection.tests;
    if (whole) {
      // Its lib and bins run here (the unit lanes' targets: only the kinds
      // the package has, since Cargo refuses --lib for a package without a
      // library). Its test targets run in the push's closure-tests rows,
      // which run them the way the hook does (the canary partitions, the
      // profile, the stack), when they build the reader's features; a pull
      // request leaves them there and names them, unless a selected
      // integration suite runs them. A push whose closure-tests rows do not
      // build the reader's features runs them here (Cargo's --tests skips a
      // target whose required features are off).
      const testsRun = suiteRuns(selection.package, null, selection.features) || (!deferToPush && closureBuilds(selection));
      if (deferToPush) testsDeferred = !testsRun;
      targets = testsRun || deferToPush ? (unitCovered ? [] : targetFlags([pkg], { tests: false })) : ["--tests"];
    } else if (kind === "lib") {
      targets = unitCovered ? [] : ["--lib"];
    } else if (kind === "bin") {
      targets = unitCovered ? [] : ["--bin", name];
    } else if (kind === "test") {
      const testRuns = suiteRuns(selection.package, name, selection.features) || (!deferToPush && closureBuilds(selection));
      targets = testRuns ? [] : ["--test", name];
      // The nested-Cargo canaries (CANARY_TEST_GROUP) outlast a reader row's
      // hung-test kill: they run only in the push's closure-tests rows, one
      // count partition each. A reader row runs the binary's other named
      // tests; a pull request names the canaries as left to the push.
      if (targets.length > 0 && canary && name === canary.binary) {
        const others = (tests ?? []).filter((test) => !test.startsWith(canary.prefix));
        if (tests === null || others.length < tests.length) {
          if (deferToPush) deferred.reader_rows.push(`${label} canaries`);
          tests = others;
          if (others.length === 0) targets = [];
        }
      }
    } else if (kind === "example") {
      // An example program reads its inputs when run, which no CI lane does;
      // only the tests an example target holds run here.
      targets = tests?.length ? ["--example", name] : [];
    } else {
      throw new Error(`internal error: reader target ${selection.target} of ${selection.package}`);
    }
    if (deferToPush && heavyChain.has(selection.package)) {
      if (targets.length > 0 || testsDeferred) deferred.reader_rows.push(label);
      continue;
    }
    if (testsDeferred) deferred.reader_rows.push(`${label} test targets`);
    if (targets.length === 0) continue;
    const filter = tests?.length ? ["-E", `test(/(^|::)(${tests.join("|")})$/)`] : [];
    rows.push({
      name: `reader ${label}`,
      packages: [selection.package],
      package_flags: `-p ${selection.package}${selection.features.length ? ` --features ${selection.features.join(",")}` : ""}`,
      test_flags: [...targets, ...filter].join(" "),
      targets,
      filtered: filter.length > 0,
      reads: selection.paths,
    });
  }
  // A row without a filter that runs a package's lib, bins or tests subsumes
  // the narrower rows of the same package and features.
  const runs = (targets) => {
    const set = new Set();
    targets.forEach((word, index) => {
      if (word === "--tests") ["--lib", "--bins", "--tests"].forEach((target) => set.add(target));
      else if (["--lib", "--bins"].includes(word)) set.add(word);
      else if (["--bin", "--test", "--example"].includes(targets[index - 1])) set.add(`${targets[index - 1]} ${word}`);
    });
    return set;
  };
  const coveredBy = (target, wide) =>
    wide.has(target) ||
    (target.startsWith("--bin ") && wide.has("--bins")) ||
    (target.startsWith("--test ") && wide.has("--tests"));
  result.reader_not_run = [...new Set(notRun)].sort();
  const unique = [...new Map(rows.map((row) => [`${row.package_flags}\0${row.test_flags}`, row])).values()];
  return unique
    .filter((row) => {
      const mine = runs(row.targets);
      return !unique.some((wide) => {
        if (wide === row || wide.filtered || wide.package_flags !== row.package_flags) return false;
        const theirs = runs(wide.targets);
        const widerOrEqual = [...mine].every((target) => coveredBy(target, theirs));
        const narrowerOrEqual = [...theirs].every((target) => coveredBy(target, mine));
        return widerOrEqual && (row.filtered || !narrowerOrEqual);
      });
    })
    .map(({ targets, filtered, ...row }) => row);
}

// Longest-processing-time first bin packing of packages into at most
// `maxShards` lanes by Rust line count. Deterministic: ties break by name.
function packShards(pkgs, weights, maxShards, model = null) {
  const sorted = [...pkgs].sort((a, b) => weights.get(b) - weights.get(a) || a.localeCompare(b));
  const shardCount = Math.min(maxShards, sorted.length);
  const bins = Array.from({ length: shardCount }, () => ({ packages: [], weight: 0 }));
  for (const pkg of sorted) {
    bins.sort((a, b) => a.weight - b.weight || a.packages.length - b.packages.length);
    bins[0].packages.push(pkg);
    bins[0].weight += weights.get(pkg);
  }
  return bins
    .filter((bin) => bin.packages.length > 0)
    .map((bin) => {
      const names = [...bin.packages].sort();
      // Label by the heaviest member so a lane reads as what dominates it.
      const heaviest = [...names].sort((a, b) => weights.get(b) - weights.get(a) || a.localeCompare(b))[0];
      const label = names.length === 1
        ? shortName(names[0])
        : `${shortName(heaviest)}+${names.length - 1}`;
      const shard = {
        name: label,
        packages: names,
        package_flags: names.map((name) => `-p ${name}`).join(" "),
        estimated_cost: bin.weight,
      };
      if (model) {
        // A lane compiles each dependency once, so the graph term uses the
        // union of the shard's dependency closures, not their sum.
        const union = new Set();
        let lines = 0;
        for (const name of names) {
          lines += model.lines.get(name);
          for (const dep of model.closures.get(name)) union.add(dep);
        }
        shard.estimated_minutes = estimatedMinutes(lines + LINK_COST_PER_DEP * union.size);
      }
      return shard;
    })
    .sort((a, b) => b.estimated_cost - a.estimated_cost || a.name.localeCompare(b.name));
}

// The lane script's kind(lib) lane: the one a plan without a lib row leaves
// to the push-to-main and nightly unit lanes.
function hookLibLane(hook) {
  const lanes = hook.lanes.filter((lane) => lane.filter === "kind(lib)");
  if (lanes.length !== 1) {
    throw new Error(`${HOOK_SCRIPT}: expected one kind(lib) lane to leave the lib tests to, found ${lanes.length}`);
  }
  return lanes[0];
}

// The canary test group's binary and test-name prefix, from the one
// `[[profile.default.overrides]]` entry that assigns CANARY_TEST_GROUP (the
// default profile's overrides apply to every profile). Its filter must be
// `binary(NAME) and test(/^PREFIX/)`; any other shape fails the plan rather
// than splitting the wrong tests. No such entry: no canary rows.
function canaryGroup(nextestText) {
  const entries = [];
  let section = null;
  let entry = null;
  for (const raw of nextestText.split("\n")) {
    const line = raw.trim();
    const header = /^\[\[?([^\]]+)\]\]?$/.exec(line);
    if (header) {
      section = header[1];
      entry = section === "profile.default.overrides" ? {} : null;
      if (entry) entries.push(entry);
      continue;
    }
    const pair = /^(filter|test-group)\s*=\s*'([^']*)'\s*(#.*)?$/.exec(line);
    if (entry && pair) entry[pair[1]] = pair[2];
  }
  const assigned = entries.filter((candidate) => candidate["test-group"] === CANARY_TEST_GROUP);
  if (assigned.length === 0) return null;
  if (assigned.length > 1) throw new Error(`${assigned.length} default overrides assign test group ${CANARY_TEST_GROUP}; expected one`);
  const shape = /^binary\(([A-Za-z0-9_]+)\) and test\(\/\^([A-Za-z0-9_]+)\/\)$/.exec(assigned[0].filter ?? "");
  if (!shape) {
    throw new Error(`test group ${CANARY_TEST_GROUP} filter ${JSON.stringify(assigned[0].filter)} is not binary(NAME) and test(/^PREFIX/)`);
  }
  return { binary: shape[1], prefix: shape[2] };
}

// The number of #[test] / #[tokio::test] functions in a test source whose
// names start with `prefix`.
function countCanaryTests(source, prefix) {
  const pattern = new RegExp(
    `#\\[(?:tokio::)?test(?:\\([^)]*\\))?\\]\\s*(?:#\\[[^\\]]*\\]\\s*)*(?:async\\s+)?fn\\s+${prefix}[A-Za-z0-9_]*\\s*\\(`,
    "g",
  );
  return (source.match(pattern) ?? []).length;
}

// The hook-parity rows for the test scope (see the header). Every row but
// HeadCanonical selects the whole workspace (`--workspace`) and names only
// its own targets: Cargo then resolves features exactly as the hook's single
// workspace build does, and builds only those targets' graphs, so a row's
// compilation units are the hook's own. Measured with cargo's unit graph on
// b45c8e60: `--workspace --test ...` rows 472/472 and 380/380 units identical
// to the hook's, `--workspace --lib` 572/572; package-scoped rows differed in
// 31-39 units (dependency features), and so does `--workspace --exclude`.
function hookParityRows({ scope, byName, unified, model, maxTestRows, nextestText, hook, libRowRunner }) {
  const rows = [];
  const testTargets = (name) =>
    byName.get(name).targets.filter((target) => target.kind.length === 1 && target.kind[0] === "test");
  const hookBuilt = (name, target) =>
    target.test !== false && (target["required-features"] ?? []).every((feature) => unified.get(name).has(feature));
  const hasLib = (name) =>
    byName.get(name).targets.some((target) => target.test !== false && target.kind.some((kind) => LIB_TARGET_KINDS.has(kind)));

  // Lib tests: one row, on the runner that can build it in time (none
  // given: the plan leaves the kind(lib) lane to the unit lanes). `--workspace --lib` builds
  // every member's lib test; no stable selection builds fewer under the
  // same resolution, so the scope narrows what runs, not what builds. The
  // filter keeps the hook's own kind(lib) (proc-macro crates' tests are not
  // in it) and the scope's packages.
  const libPackages = libRowRunner ? scope.filter(hasLib) : [];
  if (libPackages.length > 0) {
    const everyLib = [...byName.keys()].filter(hasLib);
    const filter = libPackages.length === everyLib.length
      ? "kind(lib)"
      : `kind(lib) & (${libPackages.map((name) => `package(${name})`).join(" | ")})`;
    rows.push({
      name: "lib",
      lane: "lib",
      runner: libRowRunner,
      packages: libPackages,
      package_flags: "--workspace",
      test_flags: "--lib",
      profile: CLOSURE_LIB_PROFILE,
      filter,
      binaries: [...libPackages],
      no_tests: "warn",
      rust_min_stack: HOOK_TEST_STACK,
    });
  }

  // Integration tests: every hook-built tests/*.rs target name in scope that
  // the fast selection keeps. Under --workspace, `--test NAME` selects that
  // name in every member, so a name is one unit with all of its binaries;
  // a name a member cannot build would make Cargo refuse the row.
  const keep = parseFilter(profileFilter(nextestText, CLOSURE_TEST_PROFILE));
  const allNames = [...byName.keys()].sort();
  const home = new Map();
  for (const name of scope) {
    for (const target of testTargets(name)) {
      if (!hookBuilt(name, target) || !keep({ kind: "test", binary: target.name, package: name })) continue;
      if (!home.has(target.name)) home.set(target.name, name);
    }
  }
  const binariesOf = (test) => allNames.filter((name) => {
    const target = testTargets(name).find((candidate) => candidate.name === test);
    if (!target) return false;
    if (!hookBuilt(name, target)) {
      throw new Error(`test target name ${test} is hook-built in ${home.get(test)} but not in ${name}; --test ${test} would make Cargo refuse the row`);
    }
    return true;
  }).map((name) => `${name}::${test}`);
  const byHome = new Map();
  for (const [test, name] of home) byHome.set(name, [...(byHome.get(name) ?? []), test]);
  // The nested-Cargo canaries' binary (CANARY_TEST_GROUP) leaves the packed
  // units for count-partitioned rows of its own, one per canary.
  const canary = canaryGroup(nextestText);
  let canaryRows = null;
  if (canary && home.has(canary.binary)) {
    const owner = home.get(canary.binary);
    const target = testTargets(owner).find((candidate) => candidate.name === canary.binary);
    const source = target?.src_path && existsSync(target.src_path) ? readFileSync(target.src_path, "utf8") : "";
    const count = countCanaryTests(source, canary.prefix);
    if (count >= 2) {
      byHome.set(owner, byHome.get(owner).filter((test) => test !== canary.binary));
      if (byHome.get(owner).length === 0) byHome.delete(owner);
      canaryRows = { owner, test: canary.binary, count };
    }
  }
  const units = [];
  for (const [name, tests] of [...byHome].sort(([a], [b]) => a.localeCompare(b))) {
    tests.sort();
    const parts = Math.ceil(tests.length / CLOSURE_TESTS_PER_ROW);
    for (let index = 0; index < parts; index += 1) {
      const slice = tests.slice(
        Math.floor((index * tests.length) / parts),
        Math.floor(((index + 1) * tests.length) / parts),
      );
      units.push({
        package: name,
        part: parts > 1 ? ` ${index + 1}/${parts}` : "",
        tests: slice,
        weight: model.lines.get(name) + LINK_COST_PER_DEP * model.closures.get(name).size + TEST_TARGET_COST * slice.length,
      });
    }
  }
  // Two parts of one package never share a row, or the split would not
  // parallelize; a target name lives in exactly one unit.
  units.sort((a, b) => b.weight - a.weight || a.package.localeCompare(b.package) || a.part.localeCompare(b.part));
  const bins = [];
  for (const unit of units) {
    const open = bins.filter((bin) => !bin.units.some((other) => other.package === unit.package));
    let bin;
    if (bins.length < maxTestRows || open.length === 0) {
      bin = { units: [], weight: 0 };
      bins.push(bin);
    } else {
      bin = open.sort((a, b) => a.weight - b.weight || a.units.length - b.units.length)[0];
    }
    bin.units.push(unit);
    bin.weight += unit.weight;
  }
  for (const bin of bins) {
    const tests = bin.units.flatMap((unit) => unit.tests).sort();
    const binaries = tests.flatMap(binariesOf);
    const packages = [...new Set(binaries.map((binary) => binary.split("::")[0]))].sort();
    const union = new Set();
    let lines = 0;
    for (const name of packages) {
      lines += model.lines.get(name);
      for (const dep of model.closures.get(name)) union.add(dep);
    }
    // Label by the heaviest part, which the packing placed first.
    const [heaviest] = bin.units;
    rows.push({
      name: `tests ${shortName(heaviest.package)}${heaviest.part}${bin.units.length > 1 ? `+${bin.units.length - 1}` : ""}`,
      lane: "tests",
      packages,
      package_flags: "--workspace",
      test_flags: tests.map((test) => `--test ${test}`).join(" "),
      profile: CLOSURE_TEST_PROFILE,
      binaries,
      no_tests: "warn",
      rust_min_stack: HOOK_TEST_STACK,
      estimated_minutes: estimatedMinutes(lines + LINK_COST_PER_DEP * union.size + TEST_TARGET_COST * binaries.length),
    });
  }
  if (canaryRows) {
    const binaries = binariesOf(canaryRows.test);
    const packages = [...new Set(binaries.map((binary) => binary.split("::")[0]))].sort();
    const union = new Set();
    let lines = 0;
    for (const name of packages) {
      lines += model.lines.get(name);
      for (const dep of model.closures.get(name)) union.add(dep);
    }
    for (let index = 1; index <= canaryRows.count; index += 1) {
      rows.push({
        name: `tests ${shortName(canaryRows.owner)} canaries ${index}/${canaryRows.count}`,
        lane: "tests",
        packages,
        package_flags: "--workspace",
        test_flags: `--test ${canaryRows.test}`,
        profile: CLOSURE_TEST_PROFILE,
        binaries,
        // Every partition of a binary with more tests than canaries holds
        // tests; an empty one means the binary lost its tests.
        no_tests: "fail",
        rust_min_stack: HOOK_TEST_STACK,
        partition: `count:${index}/${canaryRows.count}`,
        estimated_minutes: estimatedMinutes(lines + LINK_COST_PER_DEP * union.size + TEST_TARGET_COST * binaries.length),
      });
    }
  }

  // The hook's two named lanes: HeadCanonical with the hook's own
  // package-scoped flags, e2e-fast from the workspace build (both read from
  // the hook script by hookModel).
  const headCanonical = hook.headCanonical;
  if (scope.includes(headCanonical.package)) {
    if (!testTargets(headCanonical.package).some((target) => target.name === headCanonical.test)) {
      throw new Error(`HeadCanonical target ${headCanonical.test} is missing from ${headCanonical.package}`);
    }
    rows.push({
      name: "HeadCanonical",
      lane: "headcanonical",
      packages: [headCanonical.package],
      package_flags: `-p ${headCanonical.package} --features ${headCanonical.features.map((feature) => `${headCanonical.package}/${feature}`).join(",")}`,
      test_flags: `--test ${headCanonical.test}`,
      profile: CLOSURE_TEST_PROFILE,
      filter: `binary(${headCanonical.test})`,
      binaries: [`${headCanonical.package}::${headCanonical.test}`],
      no_tests: "fail",
      rust_min_stack: HOOK_TEST_STACK,
    });
  }
  const e2eOwners = allNames.filter((name) => testTargets(name).some((target) => target.name === hook.e2eFast.binary));
  if (e2eOwners.length !== 1) {
    throw new Error(`e2e-fast target ${hook.e2eFast.binary} belongs to ${e2eOwners.length} workspace packages`);
  }
  if (scope.includes(e2eOwners[0])) {
    rows.push({
      name: "e2e-fast",
      lane: "e2e-fast",
      packages: [e2eOwners[0]],
      package_flags: "--workspace",
      test_flags: `--test ${hook.e2eFast.binary}`,
      profile: CLOSURE_LIB_PROFILE,
      filter: `binary(${hook.e2eFast.binary})`,
      binaries: [`${e2eOwners[0]}::${hook.e2eFast.binary}`],
      no_tests: "fail",
      rust_min_stack: HOOK_TEST_STACK,
    });
  }
  const rowNames = rows.map((row) => row.name);
  if (new Set(rowNames).size !== rowNames.length) {
    throw new Error(`internal error: duplicate closure test row names: ${rowNames.join(", ")}`);
  }
  return rows;
}

// Clippy shards: per changed package (packed past the cap) for a
// changed-package plan, the fixed head groups plus packed shards for a
// whole-workspace plan (see CLIPPY_HEAD_SHARDS). Every shard names the
// targets it lints (`targets`): all of them, except in a pull-request plan,
// where `prChain` (the mob chain) lints only its lib and bin targets. Chain
// packages then never share a shard with the others: a mixed head group
// keeps its chain packages as the head, and its other packages pack with the
// rest. A chain package with neither a lib nor a bin target lints all of its
// targets, as on the push.
function clippyShards(pkgs, weights, mode, shardCap, byName, prChain = null) {
  for (const name of CLIPPY_HEAD_SHARDS.flat()) {
    if (!byName.has(name)) throw new Error(`Clippy head shard names unknown package ${name}`);
  }
  const allTargets = (shard) => ({ ...shard, targets: "--all-targets" });
  if (prChain === null) return headAndPackedShards(pkgs, weights, mode, shardCap, CLIPPY_HEAD_SHARDS).map(allTargets);
  const chain = pkgs.filter((name) => prChain.has(name));
  const others = pkgs.filter((name) => !prChain.has(name));
  const chainHeads = CLIPPY_HEAD_SHARDS.map((group) => group.filter((name) => prChain.has(name)));
  const otherHeads = CLIPPY_HEAD_SHARDS.filter((group) => group.every((name) => !prChain.has(name)));
  const libAndBins = (shard) => {
    const flags = targetFlags(shard.packages.map((name) => byName.get(name)), { tests: false });
    return { ...shard, targets: flags.length > 0 ? flags.join(" ") : "--all-targets" };
  };
  return [
    ...headAndPackedShards(chain, weights, mode, shardCap, chainHeads).map(libAndBins),
    ...headAndPackedShards(others, weights, mode, shardCap, otherHeads).map(allTargets),
  ].sort((a, b) => b.estimated_cost - a.estimated_cost || a.name.localeCompare(b.name));
}

function headAndPackedShards(pkgs, weights, mode, shardCap, headGroups) {
  if (pkgs.length === 0) return [];
  if (mode !== "workspace") return packShards(pkgs, weights, shardCap);
  const heads = headGroups.map((group) => group.filter((name) => pkgs.includes(name))).filter((group) => group.length > 0);
  const inHeads = new Set(heads.flat());
  const headShards = heads.map((group) => packShards(group, weights, 1)[0]);
  const rest = packShards(pkgs.filter((name) => !inHeads.has(name)), weights, CLIPPY_PACKED_SHARDS);
  return [...headShards, ...rest].sort((a, b) => b.estimated_cost - a.estimated_cost || a.name.localeCompare(b.name));
}

function plan(args) {
  const metadata = readMetadata();
  const packages = workspacePackages(metadata);
  const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
  const byId = new Map(packages.map((pkg) => [pkg.id, pkg]));
  const dirs = packageDirs(packages);
  const allNames = packages.map((pkg) => pkg.name).sort();

  const reverse = new Map(packages.map((pkg) => [pkg.id, new Set()]));
  for (const pkg of packages) {
    for (const dep of pkg.dependencies) {
      if (dep.source !== null) continue;
      const depPkg = byName.get(dep.name);
      if (depPkg) reverse.get(depPkg.id)?.add(pkg.id);
    }
  }

  const { paths: rawPaths, removed: rawRemoved = [], reason: pathReason } = collectChangedPaths(args);
  const changed = rawPaths === null ? null : [...new Set(rawPaths.map(normalizePath))].sort();
  const releaseProjection = releaseProjectionPlan(args, changed, pathReason);
  const readerInventory = loadNonRustReaders();
  const compiledInDocs = compiledInDocKeys(readerInventory.entries, (path) => packageForFile(path, dirs)?.name);

  const result = {
    schema_version: 1,
    event: args.event,
    diff: pathReason,
    changed_paths: changed ?? [],
    rust_changed: false,
    mode: "none",
    reason: "",
    packages: [],
    closure: [],
    shards: [],
    unmapped_rust_paths: [],
    lock_scope: null,
    generated_contract: false,
    machine_authority: false,
    machine_posters: false,
    governance_gates: false,
    wasm: false,
    sdk_host: false,
    bazel_graph: false,
    examples_browser: false,
    example_web: false,
    example_web_wasm: false,
    docs_only: false,
    release_projection: releaseProjection,
  };

  let workspaceReason = "";
  const seeds = new Set();

  const requestedBy = !args.explicitPaths && args.base && changed !== null ? workspacePlanRequested(args.head) : null;
  if (requestedBy) {
    workspaceReason = `commit ${requestedBy} requests a workspace plan (CI-Plan: workspace)`;
  }
  if (releaseProjection) {
    // Every changed path is a version projection (checked above): nothing
    // selects a package, and nothing is global.
  } else if (changed === null) {
    workspaceReason = pathReason;
  } else {
    // A Cargo.lock change that is the only global path selects the members
    // whose resolved dependencies it changed (see the header); any doubt
    // plans the whole workspace.
    if (changed.includes("Cargo.lock") && changed.filter(isGlobalPath).length === 1) {
      const texts = lockTexts(args);
      try {
        if (!texts) throw new Error("no base lock to compare (an explicit path list without --base-lock)");
        if (texts.base === null || texts.head === null) throw new Error("a side of the lock diff is unreadable");
        result.lock_scope = lockScope(texts.base, texts.head, new Set(allNames));
      } catch (error) {
        result.lock_scope = { error: error.message };
      }
    }
    for (const path of changed) {
      if (path === "Cargo.lock" && result.lock_scope?.members) {
        for (const name of result.lock_scope.members) seeds.add(name);
        continue;
      }
      if (isGlobalPath(path)) {
        workspaceReason ||= `global build configuration changed: ${path}${
          path === "Cargo.lock" && result.lock_scope?.error ? ` (not scoped: ${result.lock_scope.error})` : ""
        }`;
        continue;
      }
      if (!isRustSourcePath(path)) {
        // Non-Rust paths can still select a package: fixtures, SQL, TOML,
        // JSON and snapshot data a crate reads at test time, and the
        // embedded inputs (for example the platform skill sources the facade
        // compiles in through include macros) that the selector maps to a
        // package from outside its directory. Only documentation that lives
        // inside the crate directory is not a build input, unless the crate
        // compiles it in (the reader inventory lists it). A file another crate
        // links into its own directory selects that crate as well.
        for (const pkg of packagesForFile(path, dirs)) {
          const insidePackage = path.startsWith(`${packageDir(pkg)}/`);
          const isDoc = /\.(md|mdx)$/.test(path);
          const compiledIn = compiledInDocs.has(`${path}\0${pkg.name}`);
          if (!insidePackage || !isDoc || compiledIn) seeds.add(pkg.name);
        }
        continue;
      }
      const pathPackages = packagesForFile(path, dirs);
      if (pathPackages.length > 0) {
        for (const pkg of pathPackages) seeds.add(pkg.name);
      } else {
        result.unmapped_rust_paths.push(path);
      }
    }
    if (result.unmapped_rust_paths.length > 0) {
      workspaceReason ||= `Rust path outside any workspace package: ${result.unmapped_rust_paths[0]}`;
    }
  }

  if (releaseProjection) {
    // What a version projection can break: the generated contract surfaces
    // (schemas, SDK types and wrappers carry the version) and the Bazel
    // graph (BUILD files and the module lock). It cannot change machine
    // authority, the web runtime, the SDK suites or the examples.
    result.generated_contract = true;
    result.machine_authority = false;
    result.machine_posters = false;
    result.governance_gates = false;
    result.sdk_host = false;
    result.wasm = false;
    result.removed_paths = [];
    result.bazel_graph = true;
    result.examples_browser = false;
    result.example_web = false;
  } else if (changed !== null) {
    result.generated_contract = runClassifier("scripts/generated-contract-ratchet-changed", changed);
    result.machine_authority = runClassifier("scripts/machine-authority-changed", changed);
    result.machine_posters = runClassifier("scripts/machine-posters-changed", changed);
    result.governance_gates = result.machine_authority || changed.some(isGovernanceDocPath);
    result.sdk_host = changed.some(
      (path) =>
        path.startsWith("sdks/python/") ||
        path.startsWith("sdks/typescript/") ||
        path.startsWith("sdks/test-fixtures/") ||
        path.startsWith("tools/sdk-codegen/") ||
        path.startsWith("tools/sdk-builder/"),
    );
    result.wasm = changed.some(
      (path) =>
        path.startsWith("crates/meerkat-web-runtime/") ||
        path.startsWith("crates/meerkat-contracts/") ||
        path.startsWith("crates/meerkat-cli/src/web_runtime_template/"),
    );
    const removed = rawRemoved.map(normalizePath);
    result.removed_paths = removed;
    result.bazel_graph =
      changed.some(isBazelGraphPath) || removed.some((path) => path.endsWith(".rs"));
    result.examples_browser = changed.some(isExamplesBrowserPath);
    result.example_web = changed.some(isExampleWebPath);
  } else {
    result.generated_contract = true;
    result.machine_authority = true;
    result.machine_posters = true;
    result.governance_gates = true;
    result.sdk_host = true;
    result.wasm = true;
    result.bazel_graph = true;
    result.examples_browser = true;
    result.example_web = true;
  }

  // A unit lane compiles the package's dependencies of every kind (its
  // dev-dependencies build its lib-test), then only normal and build
  // dependencies below that. Cargo never builds a dependency's dev fixtures.
  // Reuse this graph for both the link-cost estimate and the heavy chain.
  // Following nested dev edges put xtask in the mob chain (#1362) and priced
  // runtime as though it built authorization's native facade test fixtures.
  const buildClosure = (pkg) => {
    const seen = new Set();
    const queue = [];
    const visit = (dep) => {
      if (dep.source !== null) return;
      const depPkg = byName.get(dep.name);
      if (depPkg && !seen.has(depPkg.name)) {
        seen.add(depPkg.name);
        queue.push(depPkg);
      }
    };
    for (const dep of pkg.dependencies) visit(dep);
    while (queue.length) {
      for (const dep of queue.pop().dependencies) {
        if (dep.kind !== "dev") visit(dep);
      }
    }
    return seen;
  };
  // Estimated lane cost per package: Rust lines (the lib-test binary
  // compiles every inline test) plus a link term per compiled workspace crate.
  const closures = new Map(packages.map((pkg) => [pkg.name, buildClosure(pkg)]));
  const lineCounts = new Map(packages.map((pkg) => [pkg.name, rustLineCount(resolve(root, packageDir(pkg)))]));
  const weights = new Map(
    packages.map((pkg) => [pkg.name, lineCounts.get(pkg.name) + LINK_COST_PER_DEP * closures.get(pkg.name).size]),
  );
  const model = { lines: lineCounts, closures };
  // Crates whose unit lane compiles meerkat-mob still run their unit tests
  // on push to main, with no pull-request budget. Their clippy always runs.
  const heavyChain = new Set(
    packages
      .filter((pkg) => pkg.name === HEAVY_ANCHOR || closures.get(pkg.name).has(HEAVY_ANCHOR))
      .map((pkg) => pkg.name),
  );
  result.unit_deferred_chain = [...heavyChain].sort();
  result.package_model = Object.fromEntries(
    allNames.map((name) => [name, {
      lines: lineCounts.get(name),
      dependency_closure: closures.get(name).size,
      estimated_minutes: estimatedMinutes(weights.get(name)),
      heavy_chain: heavyChain.has(name),
    }]),
  );
  result.pr_unit_budget_minutes = PR_UNIT_BUDGET_MINUTES;
  const featureSuites = FEATURE_UNIT_SUITES.map((suite) => {
    const pkg = byName.get(suite.package);
    if (!pkg) throw new Error(`feature unit suite names unknown package ${suite.package}`);
    for (const feature of suite.features) {
      if (!Object.hasOwn(pkg.features, feature)) {
        throw new Error(`feature unit suite ${suite.package}[${suite.id}] names unknown feature ${feature}`);
      }
    }
    return {
      name: `${shortName(suite.package)}[${suite.id}]`,
      packages: [suite.package],
      features: [...suite.features],
      package_flags: `-p ${suite.package} --features ${suite.features.join(",")}`,
      estimated_minutes: estimatedMinutes(weights.get(suite.package)),
      pull_request: suite.pullRequest !== false,
      ...(suite.rustMinStack ? { rust_min_stack: suite.rustMinStack } : {}),
    };
  });
  // What a pull-request plan leaves to the push (see the header); the push
  // itself runs everything, and a release projection keeps its own plan.
  const deferToPush = args.event === "pull_request" && !releaseProjection;
  const deferred = {
    closure_test_rows: 0,
    closure_check: false,
    rustdoc: false,
    feature_check_jobs: 0,
    integration_suites: [],
    unit_feature_suites: [],
    clippy_packages: [],
    clippy_packages_reason: "",
    clippy_test_targets: [],
    governed_jsonl: false,
    mcp_confinement_positive: false,
    reader_rows: [],
    example_web_wasm: false,
  };
  result.reader_not_run = [];

  const archivedPackages = new Map(
    ARCHIVED_UNIT_LANES.map((lane) => {
      if (!byName.has(lane.package)) {
        throw new Error(`archived unit lane names unknown package ${lane.package}`);
      }
      if (!Number.isInteger(lane.partitions) || lane.partitions < 2) {
        throw new Error(`archived unit lane ${lane.package} needs at least two partitions`);
      }
      return [lane.package, lane];
    }),
  );
  const partitionedUnits = new Map(
    PARTITIONED_UNIT_PACKAGES.map((lane) => {
      if (!byName.has(lane.package)) {
        throw new Error(`partitioned unit lane names unknown package ${lane.package}`);
      }
      if (!Number.isInteger(lane.partitions) || lane.partitions < 2) {
        throw new Error(`partitioned unit lane ${lane.package} needs at least two partitions`);
      }
      return [lane.package, lane.partitions];
    }),
  );

  if (releaseProjection) {
    result.mode = RELEASE_PROJECTION_MODE;
    result.reason =
      `release projection ${releaseProjection.from_version} -> ${releaseProjection.to_version}: ` +
      `tests reuse exact-main CI run ${releaseProjection.evidence.workflow_run_id} ` +
      `(attempt ${releaseProjection.evidence.workflow_run_attempt}) of ${releaseProjection.base_commit}`;
  } else if (workspaceReason) {
    result.rust_changed = true;
    result.mode = "workspace";
    result.reason = workspaceReason;
    result.packages = allNames;
    result.closure = allNames;
    // A push runs every wasm lane; a pull request runs wasm-check (below),
    // and the example lanes for their own inputs only.
    if (!deferToPush) result.wasm = true;
  } else if (seeds.size > 0) {
    result.rust_changed = true;
    result.mode = "packages";
    result.reason = result.lock_scope?.members
      ? `${seeds.size} workspace package(s) own changed paths or resolve a changed Cargo.lock entry (${result.lock_scope.changed.length} changed, ${result.lock_scope.removed.length} removed)`
      : `${seeds.size} workspace package(s) own changed paths`;
    result.packages = [...seeds].sort();
    const seen = new Set(result.packages.map((name) => byName.get(name).id));
    const queue = [...seen];
    for (let i = 0; i < queue.length; i += 1) {
      for (const dependent of reverse.get(queue[i]) ?? []) {
        if (!seen.has(dependent)) {
          seen.add(dependent);
          queue.push(dependent);
        }
      }
    }
    result.closure = [...seen].map((id) => byId.get(id).name).sort();
  } else {
    result.mode = "none";
    result.reason = "no Rust build-relevant paths changed";
    result.docs_only = changed !== null && changed.every(
      (path) => /^(docs\/|docs\/internal\/|CHANGELOG\.md|README\.md|AGENTS\.md|CLAUDE\.md|.*\.mdx?$)/.test(path),
    );
  }

  // The browser examples and WASM timer tests execute this runtime. The push
  // selects them for any changed package that reaches it through the same
  // reverse dependency closure, including a whole-workspace build change; a
  // pull request selects them only for their own inputs (the paths above):
  // on run 38859112297 the three lanes took 619, 256 and 247 s on every
  // pull request whose closure held the web runtime, nearly every Rust change.
  if (!deferToPush && result.closure.includes("meerkat-web-runtime")) {
    result.wasm = true;
    result.example_web = true;
  }
  // wasm-check compiles the web runtime and every workspace crate it links
  // (its dependency closure, which the wasm32 target does not narrow), so a
  // pull request that changes one of them runs it too (#1108): a provider
  // crate that stops compiling for wasm32 fails before the merge.
  const wasmBuild = closures.get("meerkat-web-runtime");
  if (result.packages.some((name) => name === "meerkat-web-runtime" || wasmBuild?.has(name))) {
    result.wasm = true;
  }
  // The example web suites build that closure for wasm32 at the release
  // profile, and a change to any crate in it misses sccache from that crate
  // up: the build step took 1181 s of pull request 39209102985's 1329 s lane
  // after a meerkat-core change (114 s with the closure cached; the lane
  // budget is 1080 s). The web runtime compiles meerkat-mob, so a pull
  // request that changes a crate it builds on, or plans the whole workspace,
  // leaves the wasm build and the browser suites to the push, like the
  // meerkat-mob chain's lanes. It keeps the Web SDK's type and unit tests,
  // which need no wasm. The push builds the wasm whenever the lane runs, and
  // a release projection builds none: it reuses its parent's run.
  const wasmClosureChanged =
    result.mode === "workspace" || [...seeds].some((name) => name !== "meerkat-web-runtime" && wasmBuild?.has(name));
  result.example_web_wasm = !releaseProjection && (!deferToPush || (result.example_web && !wasmClosureChanged));
  deferred.example_web_wasm = deferToPush && result.example_web && !result.example_web_wasm;

  if (result.rust_changed) {
    const shardCap = result.mode === "workspace" ? args.workspaceShards : args.maxShards;
    // Clippy lanes: every changed package. A pull request lints the
    // mob-chain packages' lib and bin targets only, and leaves the Clippy of
    // MOB_CHAIN_CLIPPY_HEADS to the push when it invalidates meerkat-mob's
    // build: in workspace mode, or when meerkat-mob or a crate its library
    // compiles (normal and build dependencies, transitively) changed.
    for (const name of [...MOB_CHAIN_CLIPPY_HEADS, HEAVY_ANCHOR]) {
      if (!byName.has(name)) throw new Error(`mob-chain Clippy deferral names unknown package ${name}`);
    }
    const mobBuild = new Set([HEAVY_ANCHOR]);
    const mobQueue = [byName.get(HEAVY_ANCHOR)];
    while (mobQueue.length) {
      for (const dep of mobQueue.pop().dependencies) {
        if (dep.source !== null || dep.kind === "dev") continue;
        const depPkg = byName.get(dep.name);
        if (depPkg && !mobBuild.has(depPkg.name)) {
          mobBuild.add(depPkg.name);
          mobQueue.push(depPkg);
        }
      }
    }
    const mobInvalidated = result.mode === "workspace" || result.packages.some((name) => mobBuild.has(name));
    deferred.clippy_packages = deferToPush && mobInvalidated
      ? result.packages.filter((name) => MOB_CHAIN_CLIPPY_HEADS.includes(name))
      : [];
    deferred.clippy_packages_reason = deferred.clippy_packages.length > 0 ? MOB_CHAIN_INVALIDATED : "";
    const clippyPackages = result.packages.filter((name) => !deferred.clippy_packages.includes(name));
    result.shards = clippyShards(
      clippyPackages,
      weights,
      result.mode,
      shardCap,
      byName,
      deferToPush ? heavyChain : null,
    );
    deferred.clippy_test_targets = deferToPush ? clippyPackages.filter((name) => heavyChain.has(name)) : [];
    if (result.shards.length === 0) {
      throw new Error("internal error: Rust-relevant change produced no lanes");
    }
    const covered = new Set(result.shards.flatMap((shard) => shard.packages));
    for (const name of clippyPackages) {
      if (!covered.has(name)) throw new Error(`internal error: package ${name} not covered by any shard`);
    }
    // Pull-request unit lanes: the changed packages outside the heavy chain,
    // packed so that no lane models over the budget; the chain is deferred
    // to the push-to-main run and reported. A partitioned package's lane of
    // its own is one row per partition.
    const runsInPullRequest = (name) => !heavyChain.has(name);
    result.unit_deferred = result.packages.filter((name) => !runsInPullRequest(name));
    result.unit_packages = result.packages.filter(runsInPullRequest).sort();
    const soloUnits = result.unit_packages.filter(
      (name) => UNIT_SOLO_PACKAGES.includes(name) || partitionedUnits.has(name),
    );
    result.unit_shards = [
      ...packWithinBudget(
        result.unit_packages.filter((name) => !soloUnits.includes(name)),
        weights,
        Math.max(1, shardCap - soloUnits.length),
        model,
      ),
      ...soloUnits.flatMap((name) => partitionRows(packShards([name], weights, 1, model)[0], partitionedUnits.get(name))),
    ];
    for (const shard of result.unit_shards) {
      if (shard.estimated_minutes > PR_UNIT_BUDGET_MINUTES) {
        throw new Error(
          `internal error: pull-request unit lane ${shard.name} models ${shard.estimated_minutes} min, over the ${PR_UNIT_BUDGET_MINUTES} min budget`,
        );
      }
    }
    // Push-to-main unit lanes: the whole workspace, no budget. Archived
    // packages leave the packed shards for their build-once lanes. A push
    // runs the unit lane's shards here verbatim and packs the rest of the
    // workspace around them (see the header).
    const unitOnMain = !deferToPush;
    const unitOnMainPackages = new Set(unitOnMain ? result.unit_packages : []);
    result.main_unit_shards = [
      ...(unitOnMain ? result.unit_shards : []),
      ...packShards(
        allNames.filter((name) => !archivedPackages.has(name) && !unitOnMainPackages.has(name)),
        weights,
        args.workspaceShards,
        model,
      ),
    ];
    const archived = archivedLanes(archivedPackages, featureSuites);
    result.main_archive_builds = archived.builds;
    result.main_archive_runs = archived.runs;
    // Feature-gated suites: all of them on main (an archived package's
    // suites run as archived lanes); with the unit lanes, those of a changed
    // package outside the heavy chain, but in a pull request only the suites
    // not marked `pullRequest: false`.
    result.main_feature_unit_shards = featureSuites.filter(
      (suite) => !archivedPackages.has(suite.packages[0]),
    );
    const changedSuites = featureSuites.filter(
      (suite) => result.unit_packages.includes(suite.packages[0]) && !heavyChain.has(suite.packages[0]),
    );
    result.unit_feature_shards = changedSuites.filter(
      (suite) => !deferToPush || suite.pull_request,
    );
    deferred.unit_feature_suites = changedSuites
      .filter((suite) => !result.unit_feature_shards.includes(suite))
      .map((suite) => suite.name);
    for (const suite of result.unit_feature_shards) {
      if (suite.estimated_minutes > PR_UNIT_BUDGET_MINUTES) {
        throw new Error(
          `internal error: pull-request feature suite ${suite.name} models ${suite.estimated_minutes} min, over the ${PR_UNIT_BUDGET_MINUTES} min budget`,
        );
      }
    }
    // The push's unit rows move to the main unit lane, the same row objects:
    // nothing is rebuilt or repacked on the way.
    result.unit_rows_on_main = [];
    if (unitOnMain) {
      const mainRows = [...result.main_unit_shards, ...result.main_feature_unit_shards];
      const names = mainRows.map((row) => row.name);
      if (new Set(names).size !== names.length) {
        throw new Error(`internal error: main unit rows repeat a name (${names.join(", ")})`);
      }
      for (const row of [...result.unit_shards, ...result.unit_feature_shards]) {
        if (!mainRows.includes(row)) throw new Error(`internal error: unit row ${row.name} is not a main unit row`);
        result.unit_rows_on_main.push(row.name);
      }
      result.unit_shards = [];
      result.unit_feature_shards = [];
    }
    // Feature checks keep the weight-packed shards: they compile one Bazel
    // set and a few matrix rows per shard, and the head split would only
    // multiply their jobs. A pull request leaves them to the push.
    result.feature_check_shards = packShards(result.packages, weights, shardCap);
    const featureJobs = featureChecks(result.feature_check_shards, byName);
    result.feature_check_jobs = deferToPush ? [] : featureJobs;
    deferred.feature_check_jobs = featureJobs.length - result.feature_check_jobs.length;
  } else {
    result.unit_deferred = [];
    result.unit_packages = [];
    result.unit_shards = [];
    result.unit_rows_on_main = [];
    result.main_unit_shards = [];
    result.main_feature_unit_shards = [];
    result.unit_feature_shards = [];
    result.main_archive_builds = [];
    result.main_archive_runs = [];
    result.feature_check_shards = [];
    result.feature_check_jobs = [];
  }

  // Integration suites of the directly changed packages' triggers. Package
  // names are checked against cargo metadata, so a rename fails the plan
  // instead of silently dropping a suite.
  for (const suite of INTEGRATION_SUITES) {
    for (const name of [suite.package, ...suite.triggers]) {
      if (!byName.has(name)) throw new Error(`integration suite ${suite.package} names unknown package ${name}`);
    }
    for (const feature of suite.features ?? []) {
      if (!Object.hasOwn(byName.get(suite.package).features, feature)) {
        throw new Error(`integration suite ${suite.package} names unknown feature ${feature}`);
      }
    }
    for (const test of suite.tests ?? []) {
      const known = byName
        .get(suite.package)
        .targets.some((target) => target.kind.includes("test") && target.name === test);
      if (!known) throw new Error(`integration suite ${suite.package} names unknown test target ${test}`);
    }
  }
  // A release projection keeps the suites its changed paths select (their
  // tests read those files); the trigger-selected ones need a Rust change.
  // A pull request leaves the suites whose package compiles meerkat-mob to
  // the push.
  const changedPaths = changed ?? [];
  const selectedSuites = INTEGRATION_SUITES.filter(
    (suite) =>
      (result.rust_changed && suite.triggers.some((name) => result.packages.includes(name))) ||
      (suite.paths ?? []).some((prefix) => changedPaths.some((path) => path.startsWith(prefix))),
  ).map((suite) => ({
    name: suite.name ?? shortName(suite.package),
    packages: [suite.package],
    package_flags: `-p ${suite.package}${suite.features?.length ? ` --features ${suite.features.join(",")}` : ""}`,
    ...(suite.tests?.length ? { test_flags: suite.tests.map((test) => `--test ${test}`).join(" ") } : {}),
  }));
  result.integration_suites = selectedSuites.filter((suite) => !deferToPush || !heavyChain.has(suite.packages[0]));
  deferred.integration_suites = selectedSuites
    .filter((suite) => !result.integration_suites.includes(suite))
    .map((suite) => suite.name);

  result.closure_flags = result.closure.map((name) => `-p ${name}`).join(" ");
  result.closure_beyond_packages = result.closure.filter((name) => !result.packages.includes(name));
  // The compile-only checks of the closure, and the two exact-profile lanes
  // (governed JSONL compiles meerkat-rpc's lib test, the confinement lane
  // the facade's lib test under every feature: 866 and 863 s on run
  // 38859112297), run on the push. A pull request also runs an exact-profile
  // lane when it touches that lane's own inputs (GUARD_LANE_INPUTS) or
  // changes its definition (GUARD_LANE_DEFINITIONS), so a case added or
  // removed there meets the lane's exact selection before the merge rather
  // than on the push after it. Without a diff every definition may have
  // changed.
  result.guard_definitions_changed = !deferToPush
    ? []
    : changed === null
      ? Object.keys(GUARD_LANE_DEFINITIONS)
      : changedGuardDefinitions(args, changedPaths);
  // Why each exact-profile lane runs, typed: the one predicate that plans
  // the lane, so a lane runs exactly when it has a reason. The CI gate reads
  // the reasons for its pull-request budget, which leaves out a lane that
  // runs only because the change touches its inputs (inputs_changed) or
  // changes its definition (definition_changed): its own cold build takes
  // 27-33 minutes (ci.yml's mcp-confinement-positive job), and the gate names
  // it in a warning instead. Every other reason keeps the lane in the
  // budget: a plan that runs it on every Rust change (every_rust_change: a
  // push, a dispatch or a release projection), and a pull request whose diff
  // is unknown (diff_unknown), where any definition only may have changed;
  // that plan carries no other reason.
  const guardReasons = (lane) => {
    if (!result.rust_changed) return [];
    if (!deferToPush) return ["every_rust_change"];
    if (changed === null) return ["diff_unknown"];
    return [
      ...(result.guard_definitions_changed.includes(lane) ? ["definition_changed"] : []),
      ...(touchesGuardLane(lane, changedPaths) ? ["inputs_changed"] : []),
    ];
  };
  result.guard_lane_reasons = Object.fromEntries(
    Object.keys(GUARD_LANE_DEFINITIONS)
      .map((lane) => [lane, guardReasons(lane)])
      .filter(([, reasons]) => reasons.length > 0),
  );
  const guardRuns = (lane) => Object.hasOwn(result.guard_lane_reasons, lane);
  result.closure_check = result.rust_changed && !deferToPush;
  // Every crate's documentation, with rustdoc's lints denied: the push
  // runs it on a Rust change, like the closure check.
  result.rustdoc = result.rust_changed && !deferToPush;
  result.governed_jsonl = guardRuns("governed_jsonl");
  result.mcp_confinement_positive = guardRuns("mcp_confinement_positive");
  deferred.closure_check = result.rust_changed && deferToPush;
  deferred.rustdoc = result.rust_changed && deferToPush;
  deferred.governed_jsonl = result.rust_changed && !result.governed_jsonl;
  deferred.mcp_confinement_positive = result.rust_changed && !result.mcp_confinement_positive;

  // Hook-parity rows for the test scope. scripts/ci-hook-coverage.mjs checks
  // them against the hook's own selection; a gap fails the plan.
  const nextestText = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  const hookText = readFileSync(resolve(root, HOOK_SCRIPT), "utf8");
  // A release projection keeps the readers of its changed files, as any
  // plan does: their tests ran on the parent's bytes, not these.
  result.reader_packages = changed === null ? [] : readerPackagesFor(changed, readerInventory);
  for (const name of result.reader_packages) {
    if (!byName.has(name)) throw new Error(`the non-Rust reader inventory names unknown package ${name}`);
  }
  if (releaseProjection) {
    // The readers' own lib and bin unit tests (an ordinary plan leaves lib
    // tests to the push-to-main unit lanes, which a projection does not run)
    // and meerkat-contracts', the owner of
    // ContractVersion::CURRENT. No budget split: these are release lanes.
    if (!byName.has(RELEASE_PROJECTION_UNIT_PACKAGE)) {
      throw new Error(`release-projection unit package ${RELEASE_PROJECTION_UNIT_PACKAGE} is not a workspace package`);
    }
    result.unit_packages = [...new Set([...result.reader_packages, RELEASE_PROJECTION_UNIT_PACKAGE])].sort();
    result.unit_shards = packWithinBudget(result.unit_packages, weights, args.maxShards, model);
  }
  result.test_scope = [...new Set([...result.closure, ...result.reader_packages])].sort();
  result.reader_rows = readerRows({
    selections: changed === null ? [] : readerSelectionsFor(changed, readerInventory),
    result,
    byName,
    heavyChain,
    deferToPush,
    deferred,
    compiledInDocs,
    unified: workspaceFeatures(metadata, packages),
    canary: canaryGroup(nextestText),
  });
  const hook = hookModel(hookText);
  // The `--workspace --lib` row runs only on a runner that can build it in
  // time; without one the plan leaves the lane script's kind(lib) lane to
  // the main and nightly unit lanes.
  result.lib_row_runner = args.libRowRunner || null;
  result.hook_retained_lanes = result.lib_row_runner ? [] : [hookLibLane(hook).label];
  const rows = result.test_scope.length > 0
    ? hookParityRows({
      scope: result.test_scope,
      byName,
      unified: workspaceFeatures(metadata, packages),
      model,
      maxTestRows: args.closureTestRows,
      nextestText,
      hook,
      libRowRunner: result.lib_row_runner,
    })
    : [];
  // A pull-request plan is checked against the rows its push runs.
  const coverage = checkHookCoverage({ ...result, closure_test_rows: rows }, { metadata, nextestText, hookText });
  result.closure_test_rows = deferToPush ? [] : rows;
  deferred.closure_test_rows = rows.length - result.closure_test_rows.length;
  result.deferred_to_push = deferToPush ? deferred : null;
  result.hook_coverage = {
    scope_packages: coverage.scope_packages,
    hook_binaries: coverage.hook_binaries,
    hook_retained: coverage.hook_retained,
    by_lane: coverage.by_lane,
    missing: coverage.missing,
    errors: coverage.errors,
  };
  if (coverage.missing.length > 0 || coverage.errors.length > 0) {
    throw new Error(
      `the plan does not cover the workspace test lanes for its scope: missing ${coverage.missing.join(", ") || "none"}; errors ${coverage.errors.join("; ") || "none"}`,
    );
  }
  // The targets each unit-lane row runs: its packages' lib and bin targets.
  // Cargo refuses --lib when no package of a run has a library (rkat and
  // surface-build-fixtures are binaries only), so a row names only the
  // kinds its packages have.
  for (const key of ["unit_shards", "unit_feature_shards", "main_unit_shards", "main_feature_unit_shards"]) {
    result[key] = result[key].map((row) => {
      const flags = targetFlags(row.packages.map((name) => byName.get(name)), { tests: false });
      if (flags.length === 0) throw new Error(`unit row ${row.name} has no lib or bin target`);
      return { ...row, targets: flags.join(" ") };
    });
  }
  // Long rows first (see the header).
  const durations = readRowDurations(args.rowDurations || resolve(root, ROW_DURATIONS));
  result.waves = {
    clippy: waveOf("clippy", result.shards, durations),
    main_unit: waveOf("main-unit", [...result.main_unit_shards, ...result.main_feature_unit_shards], durations),
    closure_tests: waveOf("closure-tests", result.closure_test_rows, durations),
    feature_check: waveOf("feature-check", result.feature_check_jobs, durations),
  };
  return result;
}

// The package whose lib tests every release-projection plan runs: it owns
// ContractVersion::CURRENT and the schema version the bump rewrites.
const RELEASE_PROJECTION_UNIT_PACKAGE = "meerkat-contracts";

// `CI-Plan: workspace` in the head commit's message or in the message of a
// merged branch head (a parent after the first), as git trailers are
// written: alone on its line.
export const WORKSPACE_PLAN_TRAILER = /^CI-Plan:[ \t]*workspace[ \t]*$/im;
function workspacePlanRequested(head) {
  const parents = spawnSync("git", ["rev-list", "--parents", "-n", "1", head], { cwd: root, encoding: "utf8" });
  if (parents.status !== 0) return null;
  const [commit, , ...merged] = parents.stdout.trim().split(/\s+/);
  for (const revision of [commit, ...merged]) {
    const message = spawnSync("git", ["log", "-1", "--format=%B", revision], { cwd: root, encoding: "utf8" });
    if (message.status === 0 && WORKSPACE_PLAN_TRAILER.test(message.stdout)) return revision;
  }
  return null;
}

// The release-projection plan's evidence (see the header): only with
// --reuse-release-evidence, only for a --base/--head diff whose head's first
// parent is the base and whose diff is a release projection, and only for
// evidence that names that base. Anything else is an error, never a plan
// with fewer lanes.
function releaseProjectionPlan(args, changed, pathReason) {
  if (!args.releaseEvidence) return null;
  if (args.explicitPaths || !args.base) {
    throw new Error("--reuse-release-evidence needs a --base/--head diff, not a path list");
  }
  if (changed === null) throw new Error(`--reuse-release-evidence needs the diff, but ${pathReason}`);
  const evidence = parseEvidenceArgument(args.releaseEvidence);
  // An explicit workspace request outranks reuse.
  if (workspacePlanRequested(args.head)) return null;
  const projection = releaseProjectionBase({ base: args.base, head: args.head, cwd: root });
  if (projection.baseSha !== evidence.commit) {
    throw new Error(`release evidence names ${evidence.commit}, but the diff base is ${projection.baseSha}`);
  }
  return {
    classifier: REUSE_CLASSIFIER,
    from_version: projection.fromVersion,
    to_version: projection.toVersion,
    base_commit: projection.baseSha,
    base_tree: projection.baseTree,
    evidence: {
      workflow_run_id: evidence.runId,
      workflow_run_attempt: evidence.runAttempt,
      attestation_artifact: attestationArtifactName(projection.baseSha, evidence.runAttempt),
    },
  };
}

// Build-once lanes for ARCHIVED_UNIT_LANES: one archive build per package
// default-feature lane and per feature suite of that package, and
// `partitions` run rows per archive that execute it with
// `--partition hash:k/n`.
function archivedLanes(archivedPackages, featureSuites) {
  const builds = [];
  const runs = [];
  const add = (archive, name, packageFlags, partitions, rustMinStack) => {
    builds.push({
      name,
      archive,
      package_flags: packageFlags,
      partitions,
      ...(rustMinStack ? { rust_min_stack: rustMinStack } : {}),
    });
    for (let index = 1; index <= partitions; index += 1) {
      runs.push({
        name: `${name} ${index}/${partitions}`,
        archive,
        partition: `hash:${index}/${partitions}`,
        ...(rustMinStack ? { rust_min_stack: rustMinStack } : {}),
      });
    }
  };
  for (const [pkg, lane] of [...archivedPackages].sort(([a], [b]) => a.localeCompare(b))) {
    add(shortName(pkg), shortName(pkg), `-p ${pkg}`, lane.partitions, null);
    for (const suite of featureSuites.filter((candidate) => candidate.packages[0] === pkg)) {
      add(
        suite.name.replaceAll(/[^A-Za-z0-9_-]+/g, "-").replace(/-+$/, ""),
        suite.name,
        suite.package_flags,
        lane.partitions,
        suite.rust_min_stack,
      );
    }
  }
  return { builds, runs };
}

// A unit lane's rows: the lane, or for a PARTITIONED_UNIT_PACKAGES lane one
// copy per `--partition hash:k/n`. The row name holds no slash: the unit
// jobs name the row's sccache log artifact after it, and an artifact name
// cannot hold one.
function partitionRows(lane, partitions) {
  if (partitions === undefined) return [lane];
  return Array.from({ length: partitions }, (_, index) => ({
    ...lane,
    name: `${lane.name} ${index + 1} of ${partitions}`,
    partition: `hash:${index + 1}/${partitions}`,
  }));
}

// Pack, then widen the shard count until every shard models under the
// pull-request unit budget (a single package over budget cannot be split and
// is reported by the caller).
function packWithinBudget(pkgs, weights, maxShards, model) {
  if (pkgs.length === 0) return [];
  let count = Math.min(maxShards, pkgs.length);
  for (;;) {
    const shards = packShards(pkgs, weights, count, model);
    const over = shards.some((shard) => shard.estimated_minutes > PR_UNIT_BUDGET_MINUTES && shard.packages.length > 1);
    if (!over || count >= pkgs.length) return shards;
    count += 1;
  }
}

// A consumer job that runs is guarded by `if:` on the corresponding count,
// so an empty plan gets a placeholder only where GitHub would otherwise
// reject an empty matrix on a job that still runs (never for the
// push-to-main unit lanes, which are skipped outright for a plan with no
// Rust-relevant change; run 35939276400 failed a "none" shard there).
function archiveMatrixOf(rows) {
  return JSON.stringify({
    include: rows.map((row) => ({
      name: row.name,
      archive: row.archive,
      ...(row.package_flags ? { packages: row.package_flags } : {}),
      ...(row.partitions ? { partitions: String(row.partitions) } : {}),
      ...(row.partition ? { partition: row.partition } : {}),
      ...(row.rust_min_stack ? { rust_min_stack: String(row.rust_min_stack) } : {}),
    })),
  });
}

function matrixOf(shards, { placeholder = true } = {}) {
  const include = shards.map((shard) => ({
    name: shard.name,
    packages: shard.package_flags,
    ...(shard.targets ? { targets: shard.targets } : {}),
    ...(shard.test_flags ? { tests: shard.test_flags } : {}),
    ...(shard.rust_min_stack ? { rust_min_stack: String(shard.rust_min_stack) } : {}),
    ...(shard.partition ? { partition: shard.partition } : {}),
  }));
  if (include.length === 0 && placeholder) include.push({ name: "none", packages: "" });
  return JSON.stringify({ include });
}

// The closure-tests job's rows: package and target flags, the nextest
// profile, an optional filter, the binaries the row must list before it runs,
// its no-tests policy and its test-thread stack.
function closureMatrixOf(rows) {
  const include = rows.map((row) => ({
    name: row.name,
    packages: row.package_flags,
    tests: row.test_flags,
    profile: row.profile,
    filter: row.filter ?? "",
    binaries: row.binaries.join(" "),
    no_tests: row.no_tests,
    rust_min_stack: String(row.rust_min_stack),
    partition: row.partition ?? "",
  }));
  if (include.length === 0) include.push({ name: "none", packages: "" });
  return JSON.stringify({ include });
}

// One line naming what a pull-request plan left to the push, for the gate's
// notice; empty when nothing was left.
function deferredSummary(deferred) {
  if (!deferred) return "";
  const parts = [];
  if (deferred.closure_test_rows > 0) parts.push(`${deferred.closure_test_rows} closure-tests rows`);
  if (deferred.closure_check) parts.push("the closure check");
  if (deferred.feature_check_jobs > 0) parts.push(`${deferred.feature_check_jobs} feature-combination jobs`);
  if (deferred.integration_suites.length > 0) parts.push(`integration suites ${deferred.integration_suites.join(", ")}`);
  if (deferred.reader_rows.length > 0) parts.push(`reader rows ${deferred.reader_rows.join(", ")}`);
  if (deferred.unit_feature_suites.length > 0) parts.push(`feature suites ${deferred.unit_feature_suites.join(", ")}`);
  if (deferred.clippy_packages.length > 0) {
    parts.push(`the Clippy of ${deferred.clippy_packages.join(", ")} (${deferred.clippy_packages_reason})`);
  }
  if (deferred.clippy_test_targets.length > 0) parts.push(`the test targets' Clippy of ${deferred.clippy_test_targets.join(", ")}`);
  if (deferred.governed_jsonl) parts.push("governed JSONL");
  if (deferred.mcp_confinement_positive) parts.push("MCP confinement positive");
  if (deferred.example_web_wasm) parts.push("the web wasm build and browser suites");
  if (deferred.rustdoc) parts.push("the workspace rustdoc");
  return parts.join("; ");
}

function githubOutput(result) {
  const lines = [];
  const scalar = (key, value) => lines.push(`${key}=${value}`);
  scalar("event", result.event);
  scalar("rust_changed", String(result.rust_changed));
  scalar("mode", result.mode);
  scalar("reason", result.reason.replaceAll("\n", " "));
  scalar("generated_contract", String(result.generated_contract));
  scalar("machine_authority", String(result.machine_authority));
  scalar("machine_posters", String(result.machine_posters));
  scalar("governance_gates", String(result.governance_gates));
  scalar("wasm", String(result.wasm));
  scalar("sdk_host", String(result.sdk_host));
  scalar("bazel_graph", String(result.bazel_graph));
  scalar("examples_browser", String(result.examples_browser));
  scalar("example_web", String(result.example_web));
  scalar("example_web_wasm", String(result.example_web_wasm));
  scalar("docs_only", String(result.docs_only));
  const projection = result.release_projection;
  scalar("release_projection", String(projection !== null));
  scalar("release_projection_versions", projection ? `${projection.from_version} -> ${projection.to_version}` : "");
  scalar("release_evidence_commit", projection?.base_commit ?? "");
  scalar("release_evidence_run_id", projection?.evidence.workflow_run_id ?? "");
  scalar("release_evidence_run_attempt", projection?.evidence.workflow_run_attempt ?? "");
  scalar("package_count", String(result.packages.length));
  scalar("closure_count", String(result.closure.length));
  scalar("closure_flags", result.closure_flags);
  scalar("closure_check", String(result.closure_check));
  scalar("rustdoc", String(result.rustdoc));
  scalar("governed_jsonl", String(result.governed_jsonl));
  scalar("mcp_confinement_positive", String(result.mcp_confinement_positive));
  scalar("guard_lane_reasons", JSON.stringify(result.guard_lane_reasons));
  scalar("deferred_to_push", deferredSummary(result.deferred_to_push));
  // Each two-wave lane's first wave keeps the lane's matrix name; its rest
  // is the _rest matrix of the twin job that waits for the stagger job. The
  // counts without a wave are the lane's whole.
  const inWave = (rows, names) => rows.filter((row) => names.includes(row.name));
  const clippyFirst = inWave(result.shards, result.waves.clippy.first);
  const clippyRest = inWave(result.shards, result.waves.clippy.rest);
  scalar("shard_count", String(result.shards.length));
  scalar("shard_first_count", String(clippyFirst.length));
  scalar("shard_rest_count", String(clippyRest.length));
  // A matrix must never be empty when the lanes are conditioned on it; keep a
  // placeholder so `fromJSON` stays well-formed and the consumer job's `if:`
  // decides whether it runs.
  scalar("shard_matrix", matrixOf(clippyFirst));
  scalar("shard_rest_matrix", matrixOf(clippyRest));
  // The feature-gated suites run in the same unit jobs as extra matrix rows.
  const unitLanes = [...result.unit_shards, ...result.unit_feature_shards];
  const mainUnitLanes = [...result.main_unit_shards, ...result.main_feature_unit_shards];
  scalar("unit_shard_count", String(unitLanes.length));
  scalar("unit_shard_matrix", matrixOf(unitLanes));
  scalar("unit_deferred", result.unit_deferred.join(" "));
  scalar("unit_rows_on_main", result.unit_rows_on_main.join(" "));
  scalar("unit_rows_on_main_count", String(result.unit_rows_on_main.length));
  scalar("unit_deferred_count", String(result.unit_deferred.length));
  const mainUnitFirst = inWave(mainUnitLanes, result.waves.main_unit.first);
  const mainUnitRest = inWave(mainUnitLanes, result.waves.main_unit.rest);
  scalar("main_unit_shard_count", String(mainUnitLanes.length));
  scalar("main_unit_first_count", String(mainUnitFirst.length));
  scalar("main_unit_rest_count", String(mainUnitRest.length));
  scalar("main_unit_shard_matrix", matrixOf(mainUnitFirst, { placeholder: false }));
  scalar("main_unit_rest_shard_matrix", matrixOf(mainUnitRest, { placeholder: false }));
  scalar("main_archive_build_count", String(result.main_archive_builds.length));
  scalar("main_archive_build_matrix", archiveMatrixOf(result.main_archive_builds));
  scalar("main_archive_run_count", String(result.main_archive_runs.length));
  scalar("main_archive_run_matrix", archiveMatrixOf(result.main_archive_runs));
  // The integration lane runs the selected suites and the reader rows.
  const integrationRows = [...result.integration_suites, ...result.reader_rows];
  scalar("integration_count", String(integrationRows.length));
  scalar("reader_not_run", result.reader_not_run.join(" "));
  scalar("integration_matrix", matrixOf(integrationRows));
  scalar("test_scope_count", String(result.test_scope.length));
  scalar("hook_binary_count", String(result.hook_coverage.hook_binaries));
  scalar("hook_retained_count", String(result.hook_coverage.hook_retained));
  scalar("hook_retained_lanes", result.hook_retained_lanes.join(","));
  const closureFirst = inWave(result.closure_test_rows, result.waves.closure_tests.first);
  const closureRest = inWave(result.closure_test_rows, result.waves.closure_tests.rest);
  scalar("closure_test_count", String(result.closure_test_rows.length));
  scalar("closure_test_first_count", String(closureFirst.length));
  scalar("closure_test_rest_count", String(closureRest.length));
  scalar("closure_test_matrix", closureMatrixOf(closureFirst));
  scalar("closure_test_rest_matrix", closureMatrixOf(closureRest));
  const featureMatrixOf = (jobs) =>
    JSON.stringify({
      include: jobs.length > 0
        ? jobs.map((job) => ({ name: job.name, commands: job.commands.join("\n") }))
        : [{ name: "none", commands: "" }],
    });
  const featureFirst = inWave(result.feature_check_jobs, result.waves.feature_check.first);
  const featureRest = inWave(result.feature_check_jobs, result.waves.feature_check.rest);
  scalar("feature_check_count", String(result.feature_check_jobs.length));
  scalar("feature_check_first_count", String(featureFirst.length));
  scalar("feature_check_rest_count", String(featureRest.length));
  scalar("feature_check_matrix", featureMatrixOf(featureFirst));
  scalar("feature_check_rest_matrix", featureMatrixOf(featureRest));
  return `${lines.join("\n")}\n`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.workspacePlanRequested) {
    // For the change classification: exit 0 when the head commit (or a
    // merged branch head) carries `CI-Plan: workspace`, so it skips the
    // evidence finder and its wait.
    const requestedBy = workspacePlanRequested(args.head);
    process.stdout.write(requestedBy ? `CI-Plan: workspace on ${requestedBy}\n` : "");
    process.exit(requestedBy ? 0 : 1);
  }
  if (args.changedPathsOnly) {
    // The path collection alone, without Cargo metadata, for the actual-commit
    // controls in scripts/test-release-projection-pre-push.sh.
    const { paths, removed = [], reason } = collectChangedPaths(args);
    process.stdout.write(`${JSON.stringify({ diff: reason, changed_paths: paths, removed_paths: removed })}\n`);
    return;
  }
  const result = plan(args);
  if (args.format === "github") {
    process.stdout.write(githubOutput(result));
  } else {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  }
  process.stderr.write(
    `ci-cargo-lanes: event=${result.event} mode=${result.mode} release_projection=${result.release_projection !== null} packages=${result.packages.length} closure=${result.closure.length} clippy_shards=${result.shards.length} unit_shards=${result.unit_shards.length} feature_suites=${result.unit_feature_shards.length}/${result.main_feature_unit_shards.length} unit_deferred=${result.unit_deferred.length} unit_rows_on_main=${result.unit_rows_on_main.length} integration=${result.integration_suites.length} reader_rows=${result.reader_rows.length} reader_not_run=${result.reader_not_run.length} feature_check_jobs=${result.feature_check_jobs.length} test_scope=${result.test_scope.length} closure_tests=${result.closure_test_rows.length} first_wave=${Object.values(result.waves).reduce((sum, wave) => sum + wave.first.length, 0)} rest_wave=${Object.values(result.waves).reduce((sum, wave) => sum + wave.rest.length, 0)} hook_binaries=${result.hook_coverage.hook_binaries} hook_retained=${result.hook_coverage.hook_retained} deferred_to_push=[${deferredSummary(result.deferred_to_push)}] (${result.reason})\n`,
  );
}

// Run as a script; imported (the self-test reads the lock scoping), nothing
// runs.
if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`ci-cargo-lanes: ${error?.message ?? error}\n`);
    process.exit(1);
  }
}
