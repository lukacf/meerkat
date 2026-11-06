#!/usr/bin/env node
// Self-test of scripts/affected-tests.mjs, the local affected-test selection.
// No cargo build: the fixture cases run the selection on fixture plans and a
// fixture repository context, the git cases on a scratch repository, and the
// end-to-end cases run the CI planner on explicit path lists (cargo metadata
// only, no compile). `make path-classifier-selftest` runs it.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  DSL_DIR,
  ROW_PROFILE,
  TEST_PROFILE,
  TLC_LANE,
  TLC_PROCEDURE_PATHS,
  UsageError,
  changedPaths,
  crateGate,
  dslMachineNames,
  hostCfg,
  ignoredTests,
  machineRegistry,
  parseArgs,
  parseTlcLane,
  resolveRange,
  runCommands,
  selectCommands,
  shellQuote,
  tlcSelection,
} from "./affected-tests.mjs";
import { INTEGRATION_SUITES, LIVE_ONLY_FEATURES } from "./ci-cargo-lanes.mjs";
import { parseNonRustReaders } from "./ci-non-rust-readers.mjs";
import { loadProducts, testFeaturesOf } from "./ci-products.mjs";
import { packageDir, readMetadata, workspacePackages } from "./rust-test-selector.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const script = resolve(here, "affected-tests.mjs");
let cases = 0;
function check(name, fn) {
  try {
    fn();
    cases += 1;
  } catch (error) {
    process.stderr.write(`affected-tests-selftest: ${name}: FAILED\n`);
    throw error;
  }
}

// --------------------------------------------------------------------------
// Arguments.
check("arguments", () => {
  assert.deepEqual(parseArgs([]), { range: null, paths: null, run: false, uncommitted: true, help: false });
  assert.deepEqual(parseArgs(["--run", "main..HEAD", "--no-uncommitted"]), { range: "main..HEAD", paths: null, run: true, uncommitted: false, help: false });
  assert.deepEqual(parseArgs(["--", "a", "b"]).paths, ["a", "b"]);
  for (const bad of [["--bogus"], ["a..b", "c..d"], ["a..b", "--", "x"], ["--"], ["--no-uncommitted", "--", "x"]]) {
    assert.throws(() => parseArgs(bad), UsageError, `refuses ${bad.join(" ")}`);
  }
});

check("shell quoting", () => {
  assert.equal(shellQuote("-p"), "-p");
  assert.equal(shellQuote("meerkat-core"), "meerkat-core");
  assert.equal(shellQuote("*"), "'*'");
  assert.equal(shellQuote("test(/(^|::)(a|b)$/)"), "'test(/(^|::)(a|b)$/)'");
  assert.equal(shellQuote("it's"), "'it'\\''s'");
});

// --------------------------------------------------------------------------
// The change: a scratch repository whose branch commits, stages, edits and
// adds files, beside an origin/main that moved on after the branch point.
check("git change collection", () => {
  const scratch = mkdtempSync(join(tmpdir(), "affected-tests-selftest-"));
  assert.ok(scratch.startsWith(tmpdir() + sep) && scratch.includes("affected-tests-selftest-"));
  try {
    const run = (...args) =>
      execFileSync("git", ["-c", "user.name=selftest", "-c", "user.email=selftest@example.invalid", "-c", "commit.gpgsign=false", ...args], {
        cwd: scratch,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      }).trim();
    const write = (path, text) => {
      mkdirSync(dirname(join(scratch, path)), { recursive: true });
      writeFileSync(join(scratch, path), text);
    };
    run("init", "-q", "-b", "main");
    for (const path of ["a.txt", "keep.txt", "gone.txt", "old.txt", "crates/x/src/lib.rs"]) write(path, `${path}\n`);
    write(".gitignore", "*.log\n");
    run("add", "-A");
    run("commit", "-q", "-m", "base");
    const base = run("rev-parse", "HEAD");
    run("checkout", "-q", "-b", "upstream");
    write("upstream.txt", "moved on\n");
    run("add", "-A");
    run("commit", "-q", "-m", "upstream");
    run("update-ref", "refs/remotes/origin/main", "HEAD");
    run("checkout", "-q", "-b", "work", base);
    write("crates/x/src/lib.rs", "changed\n");
    run("rm", "-q", "gone.txt");
    run("mv", "old.txt", "new.txt");
    run("commit", "-q", "-am", "work");
    write("staged.txt", "staged\n");
    run("add", "staged.txt");
    write("a.txt", "edited\n");
    write("untracked.txt", "new\n");
    write("ignored.log", "ignored\n");

    const range = resolveRange(scratch, null);
    assert.equal(range.base, base, "the default base is merge-base(origin/main, HEAD)");
    assert.equal(range.head, run("rev-parse", "HEAD"));
    const committed = ["crates/x/src/lib.rs", "gone.txt", "new.txt", "old.txt"];
    assert.deepEqual(changedPaths(scratch, { ...range, uncommitted: false }), committed);
    assert.deepEqual(
      changedPaths(scratch, { ...range, uncommitted: true }),
      ["a.txt", ...committed, "staged.txt", "untracked.txt"].sort(),
      "staged, unstaged and untracked paths join; ignored ones and origin/main's own changes do not",
    );
    assert.equal(resolveRange(scratch, `${base}..HEAD`).base, base);
    assert.equal(resolveRange(scratch, "HEAD~1..").base, base);
    assert.equal(resolveRange(scratch, "origin/main...HEAD").base, base, "A...B diffs from the merge base");
    assert.equal(resolveRange(scratch, "origin/main..HEAD").base, run("rev-parse", "origin/main"), "A..B diffs from A");
    assert.throws(() => resolveRange(scratch, "HEAD~1..HEAD~1"), UsageError, "a head other than HEAD is refused");
    assert.throws(() => resolveRange(scratch, "nosuchref..HEAD"), UsageError);
    assert.throws(() => resolveRange(scratch, "HEAD"), UsageError, "a single revision is no range");
    run("update-ref", "-d", "refs/remotes/origin/main");
    assert.throws(() => resolveRange(scratch, null), UsageError, "no origin/main, no default base");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

// --------------------------------------------------------------------------
// The TLC lane: a fixture lane, then the real one.
const FIXTURE_LANE = `#!/usr/bin/env bash
audit_shard() {
  case "$1" in
    audits-a|audits-b|audits-c) return 0 ;;
    *) return 1 ;;
  esac
}
alpha_audit="\${workspace_root}/specs/machines/alpha/alpha_audit.sh"
if run_part audits-a; then
  echo "running alpha"
  TLC_WORKERS="\${tlc_workers}" "\${alpha_audit}" --mutants
fi
beta_gate="\${workspace_root}/specs/machines/beta/coverage_gate.sh"
if run_part audits-b; then
  "\${beta_gate}"
fi
seam_audit="\${workspace_root}/specs/compositions/seam/seam_audit.sh"
if run_part audits-b; then
  "\${seam_audit}" --part one
fi
if run_part audits-c; then
  "\${seam_audit}" --part two
fi
model="\${workspace_root}/specs/compositions/seam/model.tla"
run_machine_verify() {
  "\${xtask_bin}" machine-verify --all --skip-cargo-tests \\
    --skip-tlc-composition seam
}
steer_audit="\${workspace_root}/specs/machines/alpha/steer_audit.sh"
steer_shard="audits-c"
`;

check("TLC lane parsing", () => {
  const lane = parseTlcLane(FIXTURE_LANE);
  assert.deepEqual(Object.fromEntries(lane.parts), {
    "audits-a": ["alpha_audit"],
    "audits-b": ["beta_gate", "seam_audit"],
    "audits-c": ["seam_audit", "steer_audit"],
  });
  assert.equal(lane.runners.get("seam_audit"), "specs/compositions/seam");
  assert.ok(!lane.runners.has("model"), "only .sh runners are audits");
  assert.deepEqual(lane.verifyFlags, ["--skip-cargo-tests"]);
  assert.deepEqual(lane.skipCompositions, ["seam"]);
  const broken = [
    [FIXTURE_LANE.replace('steer_shard="audits-c"\n', ""), /no audit part runs steer_audit/],
    [FIXTURE_LANE.replace("audits-a|audits-b|audits-c)", "audits-a|audits-b|audits-c|audits-d)"), /audit parts audits-d run no audit/],
    [FIXTURE_LANE.replace('  "${beta_gate}"\nfi', '  "${beta_gate}"'), /has no fi before the next part/],
    [FIXTURE_LANE.replace('  "${seam_audit}" --part two\nfi', '  "${seam_audit}" --part two'), /has no fi/],
    [FIXTURE_LANE.replace("machine-verify --all", "machine-verify --machine alpha"), /is not "\$\{xtask_bin\}" machine-verify --all/],
    [FIXTURE_LANE.replace('steer_shard="audits-c"', 'steer_shard="audits-z"'), /audits-z runs steer_audit but is not an audit part/],
  ];
  for (const [text, message] of broken) assert.throws(() => parseTlcLane(text), message);
});

check("the real TLC lane maps every audit to the parts CI runs", () => {
  const lane = parseTlcLane(readFileSync(resolve(repo, TLC_LANE), "utf8"));
  const ciYml = readFileSync(resolve(repo, ".github/workflows/ci.yml"), "utf8");
  const matrix = /^\s+part: \[([a-z0-9, -]+)\]$/m.exec(ciYml);
  assert.ok(matrix, "ci.yml's tlc-audits job lists its parts");
  assert.deepEqual([...lane.parts.keys()].sort(), matrix[1].split(",").map((part) => part.trim()).sort(), "the lane's audit parts are ci.yml's tlc-audits parts");
  for (const [runner, dir] of lane.runners) {
    assert.ok(readdirSync(resolve(repo, dir)).length > 0, `${runner} lives in ${dir}`);
  }
  assert.ok(lane.verifyFlags.includes("--skip-cargo-tests"));
  for (const path of TLC_PROCEDURE_PATHS) assert.ok(readFileSync(resolve(repo, path), "utf8").length > 0, `${path} exists`);
});

// --------------------------------------------------------------------------
// Machine attribution on the real specs.
const registry = machineRegistry(resolve(repo, "specs"));
const realText = (path) => {
  try {
    return readFileSync(resolve(repo, path), "utf8");
  } catch {
    return null;
  }
};

check("machine registry", () => {
  assert.ok(registry.machines.size >= 10, "the specs register machines");
  for (const [name, slug] of registry.nameToSlug) assert.ok(registry.machines.has(slug), `${name} -> ${slug}`);
  assert.ok([...registry.compositions.values()].every((members) => members.size > 0), "every composition names its machines");
  for (const members of registry.compositions.values()) {
    for (const name of members) assert.ok(registry.nameToSlug.has(name), `composition member ${name} is a registered machine`);
  }
});

check("TLC selection", () => {
  // Every DSL file that declares registered machines selects exactly them and
  // the compositions that hold them.
  const dslFiles = readdirSync(resolve(repo, DSL_DIR), { recursive: true }).filter((file) => String(file).endsWith(".rs")).map((file) => `${DSL_DIR}${file}`);
  let attributed = 0;
  for (const path of dslFiles) {
    const names = dslMachineNames(path, realText);
    const selection = tlcSelection([path], registry, realText);
    if (names.length > 0 && names.every((name) => registry.nameToSlug.has(name))) {
      attributed += 1;
      assert.deepEqual(selection.all, [], path);
      assert.deepEqual(selection.machines, [...new Set(names.map((name) => registry.nameToSlug.get(name)))].sort(), path);
      const holders = [...registry.compositions].filter(([, members]) => names.some((name) => members.has(name))).map(([slug]) => slug).sort();
      assert.deepEqual(selection.compositions, holders, path);
    } else {
      assert.equal(selection.all.length, 1, `${path} declares no registered machine, so the whole lane runs`);
    }
  }
  assert.ok(attributed >= 10, "most DSL files belong to a registered machine");
  const [someMachine] = [...registry.machines];
  const [someComposition] = [...registry.compositions.keys()];
  const select = (paths) => tlcSelection(paths, registry, realText);
  assert.deepEqual(select([`specs/machines/${someMachine}/ci.cfg`]), { machines: [someMachine], compositions: [], all: [] });
  assert.deepEqual(select([`specs/compositions/${someComposition}/model.tla`]), { machines: [], compositions: [someComposition], all: [] });
  assert.deepEqual(select(["specs/machines/README.md", "crates/meerkat-runtime/src/meerkat_machine/mod.rs", "Makefile"]), { machines: [], compositions: [], all: [] });
  assert.equal(select(["specs/machines/validate.sh"]).all.length, 1, "a spec script no machine owns runs the whole lane");
  assert.equal(select(["specs/machines/no_such_machine/model.tla"]).all.length, 1);
  assert.equal(select([`${DSL_DIR}mod.rs`]).all.length, 1, "the catalog module declares no machine");
  assert.equal(select([`${DSL_DIR}removed_machine.rs`]).all.length, 1, "a removed DSL file runs the whole lane");
  for (const path of TLC_PROCEDURE_PATHS) assert.equal(select([path]).all.length, 1, `${path} runs the whole lane`);
  // A nested DSL module belongs to the machine its parent file declares.
  const nested = (path) => (path === `${DSL_DIR}parent.rs` ? "machine ParentMachine {\n}\n" : path === `${DSL_DIR}parent/fixtures.rs` ? "fn x() {}\n" : null);
  assert.deepEqual(dslMachineNames(`${DSL_DIR}parent/fixtures.rs`, nested), ["ParentMachine"]);
});

// --------------------------------------------------------------------------
// Command selection on fixture plans and a fixture repository.
const PRODUCTS = loadProducts();
const META_STACK = PRODUCTS.find((product) => product.name === "meta").testStack;
function fixturePackage(name, { product = "core", deps = [], fixture = false, kinds = [["lib"]], tests = [] } = {}) {
  return {
    name,
    product,
    targets: [...kinds.map((kind) => ({ name: name.replaceAll("-", "_"), kind })), ...tests.map((test) => ({ name: test, kind: ["test"] }))],
    devDependsOnFixture: fixture,
    workspaceDeps: deps,
  };
}
// core <- runtime <- integration-tests, meta, auth-core; sqlite alone.
const FIXTURE_PACKAGES = [
  fixturePackage("meerkat-core"),
  fixturePackage("meerkat-sqlite", {
    tests: ["lock_sources", "feature_only", "e2e_fast_lane", "live_smoke", "gated_oracle", "gated_live", "lane_only", "wasm_only", "empty_target", "odd_gate"],
  }),
  fixturePackage("meerkat-runtime", { deps: ["meerkat-core"], tests: ["runtime_contracts"] }),
  fixturePackage("meerkat-auth-core", { deps: ["meerkat-core"] }),
  fixturePackage("meerkat-integration-tests", { deps: ["meerkat-runtime"], fixture: true, tests: ["e2e_fast_lane"] }),
  fixturePackage("meerkat-meta", { product: "meta", deps: ["meerkat-runtime"], kinds: [["lib"], ["bin"]] }),
  fixturePackage("xtask", { kinds: [["lib"], ["bin"]] }),
  // The packages of the feature-bearing integration suites, without reverse
  // dependencies here.
  fixturePackage("meerkat", { tests: ["cold_restart_resume", "persistence_contract", "governed_realm"] }),
  fixturePackage("meerkat-machine-kernels", { tests: ["kernel_round_trip"] }),
];
const FIXTURE_INVENTORY = parseNonRustReaders({
  schema_version: 1,
  paths: {
    "./": [
      { package: "meerkat-sqlite", target: "test:lock_sources", scope: "target", tests: ["every_host_is_listed"], sources: ["crates/meerkat-sqlite/tests/lock_sources.rs"] },
      { package: "xtask", target: "lib", scope: "package", sources: ["crates/xtask/src/lib.rs"] },
    ],
    "docs/": [
      { package: "xtask", target: "test:static_lanes", scope: "target", tests: ["docs_link"], features: ["machine-authority"], sources: ["crates/xtask/tests/static_lanes.rs"] },
      { package: "meerkat-runtime", target: "test:live_docs", scope: "target", features: ["openai-live-e2e"], sources: ["crates/meerkat-runtime/tests/live_docs.rs"] },
    ],
  },
});
// What the fixture's test targets need to run: cargo's required-features, a
// crate-level gate, their tests' #[ignore] attributes.
const ALWAYS = { state: "always", features: [], expr: null };
const FIXTURE_TARGETS = {
  feature_only: { requiredFeatures: ["slow-tests"] },
  live_smoke: { requiredFeatures: ["integration-real-tests"] },
  gated_oracle: { gate: { state: "needs", features: ["test-oracle"], expr: 'feature = "test-oracle"' } },
  gated_live: { gate: { state: "needs", features: ["integration-real-tests"], expr: 'feature = "integration-real-tests"' } },
  lane_only: { ignored: { tests: 2, ignored: 2, reasons: ["lane:e2e-system"] } },
  wasm_only: { gate: { state: "never", features: [], expr: 'target_arch = "wasm32"' } },
  empty_target: { ignored: { tests: 0, ignored: 0, reasons: [] } },
  odd_gate: { gate: { state: "unreadable", features: [], expr: 'any(feature = "a", feature = "b")' } },
  governed_realm: { gate: { state: "needs", features: ["local-authorization", "mcp", "session-store"], expr: 'all(feature = "session-store", feature = "local-authorization", feature = "mcp")' } },
};
function fixtureContext({ lane = parseTlcLane(FIXTURE_LANE) } = {}) {
  const packages = new Map(FIXTURE_PACKAGES.map((pkg) => [pkg.name, pkg]));
  const dirOf = (name) => (packages.get(name).product === "meta" ? `meta/crates/${name}` : `crates/${name}`);
  return {
    products: PRODUCTS,
    inventory: FIXTURE_INVENTORY,
    packages,
    readText: (path) => (path === `${DSL_DIR}alpha.rs` ? "machine AlphaMachine {\n}\n" : path === "Makefile" ? "e2e-system:\n\t@true\n" : null),
    tlc: {
      lane,
      registry: { machines: new Set(["alpha", "beta"]), nameToSlug: new Map([["AlphaMachine", "alpha"], ["BetaMachine", "beta"]]), compositions: new Map([["seam", new Set(["AlphaMachine"])]]) },
    },
    packagesForPath: (path) => [...packages.keys()].filter((name) => path.startsWith(`${dirOf(name)}/`)),
    testTargetsFor: (name, path) => {
      const match = new RegExp(`^${dirOf(name)}/tests/([a-z0-9_]+)\\.rs$`).exec(path);
      if (!match || !packages.get(name).targets.some((target) => target.name === match[1] && target.kind.includes("test"))) return null;
      return [{ name: match[1], requiredFeatures: [], gate: ALWAYS, ignored: { tests: 1, ignored: 0, reasons: [] }, ...FIXTURE_TARGETS[match[1]] }];
    },
    profileSelects: (profile, binary) => profile === TEST_PROFILE && binary.binary !== "e2e_fast_lane",
  };
}
// A push plan as scripts/ci-cargo-lanes.mjs writes it, with only the fields
// the selection reads.
function pushPlan(overrides = {}) {
  return {
    mode: "none",
    reason: "fixture",
    packages: [],
    closure: [],
    shards: [],
    integration_suites: [],
    closure_test_rows: [],
    machine_authority: false,
    machine_posters: false,
    generated_contract: false,
    sdk_host: false,
    governance_gates: false,
    rust_changed: false,
    lock_scope: null,
    products: { changed: ["core"], escalations: {}, lanes: [], floor_checks: [] },
    ...overrides,
  };
}
const NO_PR = { wasm: false, example_web: false, examples_browser: false };
const E2E_FAST_ROW = {
  name: "e2e-fast",
  lane: "e2e-fast",
  product: "core",
  packages: ["meerkat-integration-tests"],
  package_flags: "--workspace --exclude meerkat-meta",
  test_flags: "--test e2e_fast_lane",
  profile: "ci-unit",
  filter: "binary(e2e_fast_lane)",
  binaries: ["meerkat-integration-tests::e2e_fast_lane"],
  no_tests: "fail",
  rust_min_stack: 8388608,
};
const TESTS_ROW = { ...E2E_FAST_ROW, name: "tests runtime 1/1", lane: "tests", test_flags: "--test runtime_contracts", binaries: ["meerkat-runtime::runtime_contracts"] };
const lines = (selection, section) => selection.commands.filter((command) => !section || command.section === section).map((command) => command.line);
const nextest = (rest) => `./scripts/repo-cargo nextest run ${rest} --no-fail-fast --status-level fail --final-status-level fail`;

check("a library change runs its package and reverse dependencies, per product", () => {
  const push = pushPlan({
    mode: "packages",
    rust_changed: true,
    packages: ["meerkat-core"],
    closure: ["meerkat-auth-core", "meerkat-core", "meerkat-integration-tests", "meerkat-meta", "meerkat-runtime"],
    shards: [{ name: "core", packages: ["meerkat-core"], package_flags: "-p meerkat-core", targets: "--all-targets" }],
    closure_test_rows: [E2E_FAST_ROW, TESTS_ROW],
  });
  const selection = selectCommands({ paths: ["crates/meerkat-core/src/lib.rs"], push, pr: { ...NO_PR, wasm: true }, ctx: fixtureContext() });
  assert.deepEqual(lines(selection, "regenerate"), []);
  assert.deepEqual(lines(selection, "clippy"), [
    "./scripts/repo-cargo clippy -p meerkat-core --no-deps --all-targets --all-features -- -D warnings",
    "./scripts/repo-cargo check -p meerkat-auth-core -p meerkat-core -p meerkat-integration-tests -p meerkat-runtime --all-features",
    "./scripts/repo-cargo check -p meerkat-meta --all-features",
    "make wasm-check",
  ]);
  const tests = lines(selection, "tests");
  assert.ok(
    tests.includes(`fixture="$(scripts/mcp-test-server-fixture)" && MEERKAT_MCP_TEST_SERVER="$fixture" ${nextest(`-p meerkat-auth-core -p meerkat-core -p meerkat-integration-tests -p meerkat-runtime --profile ${TEST_PROFILE} --no-tests=pass`)}`),
    "Core's packages run in one invocation, with the fixture one of them needs",
  );
  const metaRow = tests.find((line) => line.includes("-p meerkat-meta --profile"));
  assert.ok(metaRow?.startsWith(`RUST_MIN_STACK=${META_STACK} `), "Meta's row runs on Meta's stack, apart from Core's");
  assert.match(metaRow, /-E 'not \(binary_id\(meerkat-meta::idle_cpu_gate\)\)'/, "CI's hosted-runner exclusion is applied");
  assert.ok(
    tests.some((line) => line.includes("-p meerkat-auth-core --features aws-sigv4,gcp-auth,azure-ad,workspace-tests --lib --profile ci-pr")),
    "the feature suites of the tested packages run, with the package's test features",
  );
  assert.ok(tests.some((line) => line.includes("--workspace --exclude meerkat-meta --test e2e_fast_lane --profile ci-unit -E 'binary(e2e_fast_lane)' --no-tests=fail")), "the plan's e2e-fast row runs");
  assert.ok(!tests.some((line) => line.includes("--test runtime_contracts")), "a plain closure-tests row is covered by the package run");
  assert.ok(tests.some((line) => line.includes("-p xtask --profile ci-integration")), "the whole-checkout reader of a package outside the closure runs");
  assert.ok(tests.some((line) => line.includes("-p meerkat-sqlite --test lock_sources --profile ci-pr")), "so does a target reader outside it");
  assert.deepEqual(selection.summary.closure, ["core 4", "meta 1"]);
});

check("a test-target change runs that target only", () => {
  const push = pushPlan({
    mode: "packages",
    rust_changed: true,
    packages: ["meerkat-runtime"],
    closure: ["meerkat-integration-tests", "meerkat-meta", "meerkat-runtime"],
    shards: [{ name: "runtime", packages: ["meerkat-runtime"], package_flags: "-p meerkat-runtime", targets: "--all-targets" }],
    integration_suites: [{ name: "runtime", packages: ["meerkat-runtime"], package_flags: "-p meerkat-runtime" }],
    closure_test_rows: [E2E_FAST_ROW, TESTS_ROW],
  });
  const selection = selectCommands({ paths: ["crates/meerkat-runtime/tests/runtime_contracts.rs"], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(selection, "clippy"), ["./scripts/repo-cargo clippy -p meerkat-runtime --no-deps --all-targets --all-features -- -D warnings"], "the changed package is still linted, with no closure check");
  const tests = lines(selection, "tests");
  assert.ok(tests.includes(nextest(`-p meerkat-runtime --test runtime_contracts --profile ${TEST_PROFILE} --no-tests=fail`)));
  assert.ok(!tests.some((line) => line.includes("-p meerkat-meta") || line.includes("meerkat-integration-tests")), "no reverse dependency runs");
  assert.ok(!tests.some((line) => line.includes("e2e_fast_lane")), "the e2e-fast row of an untouched package does not run");
  assert.ok(!tests.some((line) => line.includes("--test '*'")), "the integration suite its trigger selected is left out");
  assert.ok(selection.notes.some((note) => note.includes("integration suite runtime: only test targets")));
  assert.deepEqual(selection.summary.narrowed, ["meerkat-runtime"]);
  assert.deepEqual(selection.summary.closure, []);
});

check("narrowed targets take their required features, and leave profile-excluded lanes to their rows", () => {
  const push = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat-sqlite"], closure: ["meerkat-sqlite"], shards: [] });
  const paths = [
    "crates/meerkat-sqlite/tests/feature_only.rs",
    "crates/meerkat-sqlite/tests/e2e_fast_lane.rs",
    "crates/meerkat-sqlite/tests/lock_sources.rs",
    "crates/meerkat-sqlite/tests/live_smoke.rs",
  ];
  const selection = selectCommands({ paths, push, pr: NO_PR, ctx: fixtureContext() });
  const tests = lines(selection, "tests");
  assert.ok(tests.includes(nextest(`-p meerkat-sqlite --features slow-tests --test feature_only --test lock_sources --profile ${TEST_PROFILE} --no-tests=fail`)));
  assert.ok(selection.notes.some((note) => note.includes("meerkat-sqlite --test e2e_fast_lane: the ci-integration profile leaves it to its own lane")));
  assert.ok(selection.notes.some((note) => note.includes("meerkat-sqlite --test live_smoke needs integration-real-tests (live provider credentials): not run")));
  assert.ok(!tests.some((line) => line.includes("live_smoke") || line.includes("integration-real-tests")), "a live-provider target is named, not run");
  assert.ok(!tests.some((line) => line.includes("--test lock_sources --profile ci-pr")), "the narrowed run covers the whole-checkout reader of the same target");
});

check("crate-level gates are read for the host", () => {
  const linux = hostCfg("linux", "x64");
  const windows = hostCfg("win32", "x64");
  assert.deepEqual(crateGate("use std::fs;\n", linux), { state: "always", features: [], expr: null });
  assert.deepEqual(crateGate('//! Docs first.\n/* and a block */\n#![cfg(feature = "test-oracle")]\n#![allow(clippy::unwrap_used)]\nuse x;\n', linux), {
    state: "needs",
    features: ["test-oracle"],
    expr: 'feature = "test-oracle"',
  });
  assert.deepEqual(crateGate('#![cfg(all(\n    not(target_arch = "wasm32"),\n    feature = "oauth",\n))]\n', linux).features, ["oauth"]);
  assert.deepEqual(crateGate('#![cfg(all(unix, feature = "b", feature = "a"))]\n#![cfg(feature = "c")]\n', linux).features, ["a", "b", "c"]);
  assert.equal(crateGate("#![cfg(unix)]\n", linux).state, "always");
  assert.equal(crateGate("#![cfg(unix)]\n", windows).state, "never");
  assert.equal(crateGate('#![cfg(any(target_os = "linux", target_os = "macos"))]\n', linux).state, "always");
  assert.equal(crateGate('#![cfg(target_arch = "wasm32")]\n', linux).state, "never");
  assert.equal(crateGate('#![cfg(all(target_arch = "wasm32", feature = "x"))]\n', linux).state, "never", "a host the gate excludes wins over its features");
  for (const gate of ['#![cfg(any(feature = "a", feature = "b"))]', '#![cfg(not(feature = "skills"))]', "#![cfg(tokio_unstable)]", '#![cfg(feature = "a"']) {
    assert.equal(crateGate(`${gate}\nuse x;\n`, linux).state, "unreadable", gate);
  }
  // Only the attributes the file opens with gate the crate.
  assert.equal(crateGate('#![cfg_attr(test, allow(dead_code))]\nuse x;\n#[cfg(feature = "late")]\nmod late;\n', linux).state, "always");
});

check("tests and their #[ignore] attributes are counted", () => {
  const laneOnly = '#[test]\n#[ignore = "lane:e2e-system"]\nfn a() {}\n\n#[tokio::test(flavor = "multi_thread")]\n#[ignore = "lane:e2e-system"]\nasync fn b() {}\n';
  assert.deepEqual(ignoredTests([laneOnly]), { tests: 2, ignored: 2, reasons: ["lane:e2e-system"] });
  assert.deepEqual(ignoredTests(["#[test]\nfn a() {}\n", "#[test]\n#[ignore]\nfn b() {}\n// #[test]\n#[cfg(test)]\nmod t {}\n"]), { tests: 2, ignored: 1, reasons: [""] });
});

check("crate-level gates and ignored tests decide what a narrowed target runs", () => {
  const push = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat-sqlite"], closure: ["meerkat-sqlite"], shards: [] });
  const paths = ["gated_oracle", "gated_live", "lane_only", "wasm_only", "empty_target", "feature_only"].map((target) => `crates/meerkat-sqlite/tests/${target}.rs`);
  const selection = selectCommands({ paths, push, pr: NO_PR, ctx: fixtureContext() });
  const tests = lines(selection, "tests");
  assert.ok(tests.includes(nextest(`-p meerkat-sqlite --features slow-tests,test-oracle --test feature_only --test gated_oracle --profile ${TEST_PROFILE} --no-tests=fail`)), tests.join("\n"));
  for (const named of ["gated_live", "lane_only", "wasm_only", "empty_target"]) {
    assert.ok(!tests.some((line) => line.includes(`--test ${named}`)), `${named} is named, not run`);
  }
  for (const note of [
    "meerkat-sqlite --test gated_live needs integration-real-tests (live provider credentials): not run",
    "meerkat-sqlite --test lane_only: all 2 tests are ignored (lane e2e-system), run by make e2e-system: not run",
    'meerkat-sqlite --test wasm_only: cfg(target_arch = "wasm32") compiles it out on this host: not run',
    "meerkat-sqlite --test empty_target declares no test: nothing to run",
  ]) {
    assert.ok(selection.notes.includes(note), `${note}\n${selection.notes.join("\n")}`);
  }
  // A gate this script cannot read leaves its package whole.
  const odd = selectCommands({ paths: ["crates/meerkat-sqlite/tests/odd_gate.rs"], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(odd.summary.narrowed, []);
  assert.ok(lines(odd, "tests").includes(nextest(`-p meerkat-sqlite --profile ${TEST_PROFILE} --no-tests=pass`)));
  assert.ok(odd.notes.some((note) => note.startsWith("meerkat-sqlite: --test odd_gate opens with cfg(any(") && note.endsWith("the package runs whole")), odd.notes.join("\n"));
});

check("a feature-bearing suite keeps its features for a narrowed target it holds", () => {
  const suites = [
    {
      name: "meerkat-store-restart",
      packages: ["meerkat"],
      package_flags: "-p meerkat --features live,session-store",
      test_flags: "--test cold_restart_resume --test persistence_contract",
    },
    { name: "meerkat-governed-realm", packages: ["meerkat"], package_flags: "-p meerkat --features session-store,local-authorization,mcp", test_flags: "--test governed_realm" },
  ];
  const push = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat"], closure: ["meerkat"], integration_suites: suites });
  const ofMeerkat = (selection) => lines(selection, "tests").filter((line) => line.includes(" -p meerkat "));
  const restart = selectCommands({ paths: ["crates/meerkat/tests/cold_restart_resume.rs"], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(ofMeerkat(restart), [
    nextest(`-p meerkat --test cold_restart_resume --profile ${TEST_PROFILE} --no-tests=fail`),
    nextest(`-p meerkat --features live,session-store --test cold_restart_resume --profile ${ROW_PROFILE} --no-tests=fail`),
  ]);
  assert.ok(restart.notes.includes("integration suite meerkat-store-restart: runs the changed cold_restart_resume with its features"), restart.notes.join("\n"));
  assert.ok(restart.notes.includes("integration suite meerkat-governed-realm: only test targets of its trigger packages changed"));
  // A crate-level gate that already names the suite's features runs the
  // target once.
  const realm = selectCommands({ paths: ["crates/meerkat/tests/governed_realm.rs"], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(ofMeerkat(realm), [nextest(`-p meerkat --features local-authorization,mcp,session-store --test governed_realm --profile ${TEST_PROFILE} --no-tests=fail`)]);
  // A suite over every test target of its package holds each narrowed one.
  const kernels = pushPlan({
    mode: "packages",
    rust_changed: true,
    packages: ["meerkat-machine-kernels"],
    closure: ["meerkat-machine-kernels"],
    integration_suites: [{ name: "machine-kernels", packages: ["meerkat-machine-kernels"], package_flags: "-p meerkat-machine-kernels --features test-oracle", test_flags: "" }],
  });
  const oracle = selectCommands({ paths: ["crates/meerkat-machine-kernels/tests/kernel_round_trip.rs"], push: kernels, pr: NO_PR, ctx: fixtureContext() });
  assert.ok(
    lines(oracle, "tests").includes(nextest(`-p meerkat-machine-kernels --features test-oracle --test kernel_round_trip --profile ${ROW_PROFILE} --no-tests=fail`)),
    lines(oracle, "tests").join("\n"),
  );
});

// The fixture context with test features on some packages: name ->
// { declared: [standalone-tests features], workspaceTests: boolean }, read
// by scripts/ci-products.mjs testFeaturesOf as in a cargo metadata package.
function withDeclaredTestFeatures(ctx, declarations) {
  const packages = new Map(
    [...ctx.packages].map(([name, pkg]) => {
      const declaration = declarations[name];
      if (!declaration) return [name, pkg];
      const declared = declaration.declared ?? [];
      const features = Object.fromEntries([...declared, ...(declaration.workspaceTests ? ["workspace-tests"] : [])].map((feature) => [feature, []]));
      return [name, { ...pkg, features, metadata: declared.length > 0 ? { meerkat: { "standalone-tests": { features: declared } } } : null }];
    }),
  );
  return { ...ctx, packages };
}

check("every test run passes its packages' test features", () => {
  const ctx = withDeclaredTestFeatures(fixtureContext(), {
    "meerkat-runtime": { declared: ["test-support", "sqlite-store"] },
    "meerkat-integration-tests": {},
    "meerkat-sqlite": { workspaceTests: true },
  });
  const runtimeChange = (suiteFeatures) =>
    pushPlan({
      mode: "packages",
      rust_changed: true,
      packages: ["meerkat-runtime"],
      closure: ["meerkat-integration-tests", "meerkat-meta", "meerkat-runtime"],
      integration_suites: [{ name: "runtime", packages: ["meerkat-runtime"], package_flags: `-p meerkat-runtime --features ${suiteFeatures}` }],
    });
  // The closure run names each package's test features by package.
  const plain = lines(selectCommands({ paths: ["crates/meerkat-runtime/src/lib.rs"], push: runtimeChange("test-support,sqlite-store"), pr: NO_PR, ctx }), "tests");
  assert.ok(
    plain.some((line) =>
      line.includes(`-p meerkat-integration-tests -p meerkat-runtime --features meerkat-runtime/test-support,meerkat-runtime/sqlite-store --profile ${TEST_PROFILE} --no-tests=pass`),
    ),
    plain.join("\n"),
  );
  // A suite of only those features runs nothing the closure run does not;
  // one that adds a feature still runs.
  assert.ok(!plain.some((line) => line.includes("--test '*'")), plain.join("\n"));
  const live = lines(selectCommands({ paths: ["crates/meerkat-runtime/src/lib.rs"], push: runtimeChange("test-support,sqlite-store,live"), pr: NO_PR, ctx }), "tests");
  assert.ok(live.includes(nextest(`-p meerkat-runtime --features test-support,sqlite-store,live --test '*' --profile ${ROW_PROFILE} --no-tests=fail`)), live.join("\n"));
  // A narrowed run adds them to its targets' own features.
  const sqlite = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat-sqlite"], closure: ["meerkat-sqlite"], shards: [] });
  const narrowed = lines(selectCommands({ paths: ["crates/meerkat-sqlite/tests/feature_only.rs"], push: sqlite, pr: NO_PR, ctx }), "tests");
  assert.ok(narrowed.includes(nextest(`-p meerkat-sqlite --features slow-tests,workspace-tests --test feature_only --profile ${TEST_PROFILE} --no-tests=fail`)), narrowed.join("\n"));
  // So does a reader run.
  const readers = lines(selectCommands({ paths: ["docs/guide.md"], push: pushPlan(), pr: NO_PR, ctx }), "tests");
  assert.ok(
    readers.includes(nextest(`-p meerkat-sqlite --features workspace-tests --test lock_sources --profile ${ROW_PROFILE} -E 'test(/(^|::)(every_host_is_listed)$/)' --no-tests=fail`)),
    readers.join("\n"),
  );
});

check("a change outside the tests directory is not narrowed", () => {
  const push = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat-sqlite"], closure: ["meerkat-sqlite"], shards: [] });
  for (const paths of [["crates/meerkat-sqlite/tests/lock_sources.rs", "crates/meerkat-sqlite/src/lib.rs"], ["crates/meerkat-sqlite/tests/fixture.json"]]) {
    const selection = selectCommands({ paths, push, pr: NO_PR, ctx: fixtureContext() });
    assert.deepEqual(selection.summary.narrowed, [], paths.join(" "));
    assert.ok(lines(selection, "tests").includes(nextest(`-p meerkat-sqlite --profile ${TEST_PROFILE} --no-tests=pass`)), paths.join(" "));
  }
  // A lock-scoped or whole-workspace plan names packages no path narrows.
  const locked = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat-sqlite"], closure: ["meerkat-sqlite"], lock_scope: { members: ["meerkat-sqlite"] } });
  assert.deepEqual(selectCommands({ paths: ["Cargo.lock", "crates/meerkat-sqlite/tests/lock_sources.rs"], push: locked, pr: NO_PR, ctx: fixtureContext() }).summary.narrowed, []);
});

check("the closure must match the planner's", () => {
  const push = pushPlan({ mode: "packages", rust_changed: true, packages: ["meerkat-core"], closure: ["meerkat-core"] });
  assert.throws(() => selectCommands({ paths: ["crates/meerkat-core/src/lib.rs"], push, pr: NO_PR, ctx: fixtureContext() }), /differs from the CI planner's/);
});

check("readers: features, live-only readers and whole packages", () => {
  const selection = selectCommands({ paths: ["docs/guide.md"], push: pushPlan(), pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(selection, "clippy"), []);
  assert.deepEqual(lines(selection, "tests"), [
    nextest(`-p meerkat-sqlite --test lock_sources --profile ${ROW_PROFILE} -E 'test(/(^|::)(every_host_is_listed)$/)' --no-tests=fail`),
    nextest(`-p xtask --profile ${TEST_PROFILE} --no-tests=fail`),
    nextest(`-p xtask --features machine-authority --test static_lanes --profile ${ROW_PROFILE} -E 'test(/(^|::)(docs_link)$/)' --no-tests=fail`),
  ]);
  assert.ok(selection.notes.some((note) => note.includes("meerkat-runtime test:live_docs needs openai-live-e2e")), "a live-provider reader is named, not run");
  // The whole-checkout readers leave product roots to their products.
  const meta = selectCommands({ paths: ["meta/console/app.ts"], push: pushPlan(), pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(meta), []);
  assert.ok(meta.notes.some((note) => note.includes("under meta/ outside its crates")));
});

check("machine authority regenerates, then runs TLC for the touched machine", () => {
  const push = pushPlan({ machine_authority: true });
  const selection = selectCommands({ paths: [`${DSL_DIR}alpha.rs`], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(selection, "regenerate"), ["make machine-codegen", "make protocol-codegen", "node scripts/machine-posters/generate-machine-posters.mjs"]);
  assert.deepEqual(lines(selection, "tlc"), [
    "./scripts/repo-cargo xtask machine-verify --skip-cargo-tests --machine alpha --composition seam --skip-tlc-composition seam",
    `bash ${TLC_LANE} --part audits-a`,
    `bash ${TLC_LANE} --part audits-b`,
    `bash ${TLC_LANE} --part audits-c`,
  ]);
  const beta = selectCommands({ paths: ["specs/machines/beta/coverage_gate.sh"], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(beta, "tlc"), ["./scripts/repo-cargo xtask machine-verify --skip-cargo-tests --machine beta", `bash ${TLC_LANE} --part audits-b`]);
  const owner = selectCommands({ paths: ["crates/meerkat-runtime/src/meerkat_machine/mod.rs"], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(owner, "tlc"), []);
  assert.ok(owner.notes.some((note) => note.startsWith("TLC: no spec input changed")));
  const whole = selectCommands({ paths: [TLC_LANE], push, pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(whole, "tlc"), ["make machine-verify"]);
  const unread = selectCommands({ paths: [`${DSL_DIR}alpha.rs`], push, pr: NO_PR, ctx: fixtureContext({ lane: { error: "fixture" } }) });
  assert.deepEqual(lines(unread, "tlc"), ["make machine-verify"], "a lane this script cannot read runs whole");
  const posters = selectCommands({ paths: ["scripts/machine-posters/generate-machine-posters.mjs"], push: pushPlan({ machine_posters: true }), pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(posters, "regenerate"), ["node scripts/machine-posters/generate-machine-posters.mjs"]);
});

check("wire schema and SDK changes run the SDK suites", () => {
  const schema = selectCommands({ paths: ["artifacts/schemas/x.json"], push: pushPlan({ generated_contract: true }), pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(schema, "regenerate"), ["make regen-schemas"]);
  assert.deepEqual(lines(schema, "sdk"), ["./scripts/repo-cargo build -p meerkat-rpc", "make test-sdk-python", "make test-sdk-typescript", "make test-sdk-web"]);
  const host = selectCommands({ paths: ["sdks/python/x.py"], push: pushPlan({ sdk_host: true }), pr: NO_PR, ctx: fixtureContext() });
  assert.deepEqual(lines(host, "sdk"), ["./scripts/repo-cargo build -p meerkat-rpc", "make test-sdk-python", "make test-sdk-typescript"]);
  const web = selectCommands({ paths: ["sdks/web/src/x.ts"], push: pushPlan(), pr: { ...NO_PR, example_web: true }, ctx: fixtureContext() });
  assert.deepEqual(lines(web, "sdk"), ["make test-sdk-web"]);
});

// --------------------------------------------------------------------------
// --run: in order, stop at the first failure, stop when regeneration changed
// the tree.
check("--run", () => {
  const commands = [
    { section: "regenerate", line: "regen" },
    { section: "clippy", line: "lint" },
    { section: "tests", line: "test" },
  ];
  const quiet = () => {};
  const ran = [];
  const spawn = (statuses) => (shell, args) => {
    assert.equal(shell, "bash");
    ran.push(args[1]);
    return { status: statuses[args[1]] ?? 0 };
  };
  assert.equal(runCommands(commands, { repo, spawn: spawn({}), digest: () => "same", log: quiet }), 0);
  assert.deepEqual(ran.splice(0), ["regen", "lint", "test"]);
  assert.equal(runCommands(commands, { repo, spawn: spawn({ lint: 7 }), digest: () => "same", log: quiet }), 7);
  assert.deepEqual(ran.splice(0), ["regen", "lint"], "the first failure stops the run");
  let digests = 0;
  assert.equal(runCommands(commands, { repo, spawn: spawn({}), digest: () => `tree ${digests++}`, log: quiet }), 3);
  assert.deepEqual(ran.splice(0), ["regen"], "a regeneration that changed the tree stops the run");
  assert.equal(runCommands([{ section: "tests", line: "exit 5" }], { repo, digest: () => "unused", log: quiet }), 5, "bash runs each line");
  const said = [];
  assert.equal(runCommands([], { repo, spawn: spawn({}), digest: () => "unused", log: (text) => said.push(text) }), 0);
  assert.deepEqual(ran.splice(0), []);
  assert.deepEqual(said, ["affected-tests: nothing to run\n"], "nothing to run is not reported as passed commands");
});

// --------------------------------------------------------------------------
// End to end on this checkout: the CI planner on explicit path lists.
function cli(...args) {
  const result = spawnSync(process.execPath, [script, ...args], { cwd: repo, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  return { status: result.status, stdout: result.stdout.split("\n").filter(Boolean), stderr: result.stderr };
}
const KNOWN_PREFIXES = ["./scripts/repo-cargo ", 'fixture="$(scripts/mcp-test-server-fixture)" && ', "RUST_MIN_STACK=", "TOOLKIT_SHARED_TEST_PYTHON=", "make ", "node ", "bash "];

check("end to end: a documentation change runs readers only", () => {
  const out = cli("--", "docs/internal/onboarding.md");
  assert.equal(out.status, 0, out.stderr);
  assert.ok(out.stdout.length > 0, "the whole-checkout readers run for every change");
  for (const line of out.stdout) assert.ok(KNOWN_PREFIXES.some((prefix) => line.startsWith(prefix)), line);
  assert.ok(!out.stdout.some((line) => / clippy | check -p /.test(line)), "no Rust package changed");
});

check("end to end: a test target change is narrowed", () => {
  const packages = workspacePackages(readMetadata()).sort((a, b) => a.name.localeCompare(b.name));
  let pick = null;
  for (const pkg of packages) {
    const dir = packageDir(pkg);
    const target = pkg.targets.find((candidate) => {
      if (!candidate.kind.includes("test") || !candidate.src_path.endsWith(`${sep}tests${sep}${candidate.name}.rs`)) return false;
      if (candidate["required-features"]?.length || candidate.name.startsWith("e2e_")) return false;
      const text = readFileSync(candidate.src_path, "utf8");
      const tests = ignoredTests([text]);
      return crateGate(text).state === "always" && tests.tests > tests.ignored;
    });
    if (target && !dir.startsWith("meta/") && !dir.startsWith("connect/")) {
      pick = { pkg: pkg.name, target: target.name, path: `${dir}/tests/${target.name}.rs` };
      break;
    }
  }
  assert.ok(pick, "some Core package has a tests/*.rs target");
  const out = cli("--", pick.path);
  assert.equal(out.status, 0, out.stderr);
  assert.ok(out.stdout.some((line) => line.includes(`nextest run -p ${pick.pkg} --test ${pick.target} --profile ${TEST_PROFILE}`)), `${pick.path} runs its target`);
  assert.ok(!out.stdout.some((line) => line.includes(" check -p ")), "no reverse-dependency closure");
  assert.ok(out.stderr.includes("only test targets changed"), out.stderr);
});

check("end to end: a gated, a live-gated and a lane-only test target", () => {
  const out = cli(
    "--",
    "crates/meerkat-machine-kernels/tests/kernel_typed_round_trip.rs",
    "crates/meerkat-cli/tests/system_cli_realm_mcp.rs",
    "crates/meerkat-rpc/tests/live_smoke_rpc.rs",
  );
  assert.equal(out.status, 0, out.stderr);
  assert.ok(
    out.stdout.some((line) => line.includes(`nextest run -p meerkat-machine-kernels --features test-oracle --test kernel_typed_round_trip --profile ${TEST_PROFILE}`)),
    `the crate-level test-oracle gate is passed as a feature:\n${out.stdout.join("\n")}`,
  );
  assert.ok(!out.stdout.some((line) => /system_cli_realm_mcp|live_smoke_rpc/.test(line)), "the live-gated and lane-only targets are named, not run");
  assert.ok(out.stderr.includes("rkat --test system_cli_realm_mcp needs integration-real-tests (live provider credentials): not run"), out.stderr);
  assert.match(out.stderr, /meerkat-rpc --test live_smoke_rpc: all \d+ tests are ignored \(lanes? [^)]*e2e-live[^)]*\)/);
});

// Does a command run `target` of `pkg` with every one of `features`?
function runsWithFeatures(commands, pkg, target, features) {
  return commands.some((line) => {
    const words = line.split(" ");
    if (!words.includes("nextest") || words[words.indexOf("-p") + 1] !== pkg) return false;
    if (!words.some((word, index) => word === "--test" && words[index + 1] === target)) return false;
    const at = words.indexOf("--features");
    const enabled = at < 0 ? [] : words[at + 1].split(",");
    return features.every((feature) => enabled.includes(feature));
  });
}
const suiteNamed = (name) => INTEGRATION_SUITES.find((suite) => (suite.name ?? suite.package) === name);

check("end to end: a test-only edit of governed_realm keeps the governed-realm suite's features", () => {
  const out = cli("--", "crates/meerkat/tests/governed_realm.rs");
  assert.equal(out.status, 0, out.stderr);
  assert.ok(runsWithFeatures(out.stdout, "meerkat", "governed_realm", suiteNamed("meerkat-governed-realm").features), out.stdout.join("\n"));
});

check("end to end: a test-only edit of cold_restart_resume keeps the store-restart suite's features", () => {
  const out = cli("--", "crates/meerkat/tests/cold_restart_resume.rs");
  assert.equal(out.status, 0, out.stderr);
  assert.ok(runsWithFeatures(out.stdout, "meerkat", "cold_restart_resume", suiteNamed("meerkat-store-restart").features), out.stdout.join("\n"));
});

check("end to end: a narrowed target carries its package's test features", () => {
  // meerkat-runtime's test features are more than the target's crate-level
  // gate (sqlite-store): every one of them is passed.
  const out = cli("--", "crates/meerkat-runtime/tests/pre_ledger_realm_bridge.rs");
  assert.equal(out.status, 0, out.stderr);
  const runtime = workspacePackages(readMetadata()).find((pkg) => pkg.name === "meerkat-runtime");
  const features = testFeaturesOf(runtime);
  assert.ok(features.length > 1, "meerkat-runtime declares test features");
  assert.ok(runsWithFeatures(out.stdout, "meerkat-runtime", "pre_ledger_realm_bridge", features), out.stdout.join("\n"));
});

check("end to end: every feature-bearing integration suite keeps its features for a narrowed target", () => {
  const packages = new Map(workspacePackages(readMetadata()).map((pkg) => [pkg.name, pkg]));
  for (const suite of INTEGRATION_SUITES.filter((entry) => entry.features?.length > 0)) {
    const pkg = packages.get(suite.package);
    const target = pkg.targets.find((candidate) => {
      if (!candidate.kind.includes("test") || !candidate.src_path.endsWith(`${sep}tests${sep}${candidate.name}.rs`) || candidate.name.startsWith("e2e_")) return false;
      if (suite.tests && !suite.tests.includes(candidate.name)) return false;
      const text = readFileSync(candidate.src_path, "utf8");
      const gate = crateGate(text);
      const tests = ignoredTests([text]);
      const needs = [...(candidate["required-features"] ?? []), ...gate.features];
      return (gate.state === "always" || gate.state === "needs") && !needs.some((feature) => LIVE_ONLY_FEATURES.includes(feature)) && tests.tests > tests.ignored;
    });
    const name = suite.name ?? suite.package;
    assert.ok(target, `${name}: a runnable test target to edit`);
    const path = `${packageDir(pkg)}/tests/${target.name}.rs`;
    const out = cli("--", path);
    assert.equal(out.status, 0, out.stderr);
    assert.ok(runsWithFeatures(out.stdout, suite.package, target.name, suite.features), `${name}: ${path}\n${out.stdout.join("\n")}`);
  }
});

check("end to end: a machine DSL change regenerates and runs the machine's TLC", () => {
  const dsl = readdirSync(resolve(repo, DSL_DIR)).filter((file) => file.endsWith(".rs")).map((file) => `${DSL_DIR}${file}`);
  const path = dsl.find((candidate) => {
    const names = dslMachineNames(candidate, realText);
    return names.length === 1 && registry.nameToSlug.has(names[0]);
  });
  assert.ok(path, "a DSL file declares one registered machine");
  const slug = registry.nameToSlug.get(dslMachineNames(path, realText)[0]);
  const out = cli("--", path);
  assert.equal(out.status, 0, out.stderr);
  assert.deepEqual(out.stdout.slice(0, 3), ["make machine-codegen", "make protocol-codegen", "node scripts/machine-posters/generate-machine-posters.mjs"]);
  assert.ok(out.stdout.some((line) => line.startsWith("./scripts/repo-cargo xtask machine-verify --skip-cargo-tests") && line.includes(` --machine ${slug}`)), out.stdout.join("\n"));
});

process.stdout.write(`affected-tests-selftest: ${cases} cases passed\n`);
