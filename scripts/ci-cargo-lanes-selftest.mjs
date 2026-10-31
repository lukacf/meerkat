#!/usr/bin/env node
// Fail-closed contract test for scripts/ci-cargo-lanes.mjs, the changed-path
// classifier behind the GitHub-hosted Cargo PR CI. Each fixture is a changed
// path list; the assertions pin the property the old `changed-paths`
// BuildBuddy mode violated: a build-relevant change always yields lanes.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { basename, dirname, join, relative, resolve } from "node:path";
import {
  CI_ROW_STEPS,
  rowStepBindings,
  HOOK_SCRIPT,
  bindRowSteps,
  checkHookCoverage,
  hookModel,
  featureClosure,
  laneSelections,
  parseFilter,
  profileFilter,
  workspaceFeatures,
} from "./ci-hook-coverage.mjs";
import {
  FEATURE_UNIT_SUITES,
  TOOLKIT_MOBKIT_KNOWN_REDS,
  TOOLKIT_MOBKIT_TEST_DEBT,
  checkKnownRedsDoc,
  featureSuiteFilters,
  featureSuiteFlags,
  GOVERNANCE_DOC_PATTERNS,
  GUARD_LANE_INPUTS,
  HOSTED_RUNNER_EXCLUDED_BINARIES,
  MOB_CHAIN_CLIPPY_HEADS,
  MOB_CHAIN_INVALIDATED,
  PARTITIONED_UNIT_PACKAGES,
  ROW_DURATIONS,
  UNIT_SOLO_PACKAGES,
  WAVE_FIRST_SECONDS,
  WAVE_LANES,
  lockScope,
  nextestDefaultSelection,
  parseCargoLock,
  readRowDurations,
  rowKey,
  waveOf,
  compiledInDocKeys,
  readerRows,
  touchesGuardLane,
} from "./ci-cargo-lanes.mjs";
import { readMetadata, workspacePackages } from "./rust-test-selector.mjs";
import { CORE, loadProducts } from "./ci-products.mjs";

const PRODUCTS = loadProducts();
// The lane selections the plan's rows must use: Core's (`--workspace` and an
// `--exclude` for every other product's package) and each product's (`-p`
// each of its packages).
const SELECTIONS = laneSelections(readMetadata(), { products: PRODUCTS });
const selectionFlags = (product) => SELECTIONS.selections.get(product).flags;
import { durationsOf, rowOf } from "./ci-row-durations.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const classifier = resolve(here, "ci-cargo-lanes.mjs");
const root = resolve(here, "..");

function run(args, { input } = {}) {
  const result = spawnSync("node", [classifier, ...args], {
    cwd: root,
    encoding: "utf8",
    input,
    maxBuffer: 64 * 1024 * 1024,
  });
  return result;
}

function planFor(paths, extra = []) {
  const result = run(["--format", "json", ...extra, "--", ...paths]);
  assert.equal(result.status, 0, `classifier failed: ${result.stderr}`);
  return JSON.parse(result.stdout);
}

// A two-wave lane's rows from its GitHub outputs, both waves together: which
// wave a row starts in is the duration table's call.
function bothWaves(lines, first, rest) {
  return [...JSON.parse(lines[first]).include, ...JSON.parse(lines[rest]).include].filter((row) => row.name !== "none");
}

function assertLanes(plan, label) {
  assert.equal(plan.rust_changed, true, `${label}: rust_changed`);
  assert.ok(plan.shards.length >= 1, `${label}: at least one clippy/unit shard`);
  // Every changed package lints in a shard, or (a workspace-mode pull
  // request's chain heads) is named as linting on the push.
  const covered = new Set(plan.shards.flatMap((shard) => shard.packages));
  const pushClippy = new Set(plan.deferred_to_push?.clippy_packages ?? []);
  for (const name of plan.packages) {
    assert.ok(covered.has(name) !== pushClippy.has(name), `${label}: ${name} lints in exactly one of a shard and the push`);
  }
  for (const shard of plan.shards) {
    assert.match(shard.packages_flags ?? shard.package_flags, /^-p \S+( -p \S+)*$/, `${label}: shard flags`);
  }
  assert.ok(plan.closure.length >= plan.packages.length, `${label}: closure contains the packages`);
  for (const name of plan.packages) {
    assert.ok(plan.closure.includes(name), `${label}: closure includes ${name}`);
  }
  // Unit plan: every changed package is either in a unit shard (on a push,
  // one of the main unit lane's rows named in unit_rows_on_main) or named as
  // deferred to the push-to-main run, never dropped.
  const unitRows = unitRowsOf(plan, label);
  const unitCovered = unitLanesOf(unitRows.shards, label).flatMap((shard) => shard.packages);
  assert.deepEqual(
    [...unitCovered, ...plan.unit_deferred].sort(),
    [...plan.packages].sort(),
    `${label}: unit shards plus deferred packages partition the changed packages`,
  );
  for (const name of plan.unit_deferred) {
    assert.equal(plan.package_model[name].heavy_chain, true, `${label}: ${name} is deferred only because it compiles meerkat-mob`);
  }
  for (const shard of unitRows.shards) {
    assert.ok(
      shard.estimated_minutes <= plan.pr_unit_budget_minutes,
      `${label}: pull-request unit lane ${shard.name} models ${shard.estimated_minutes} min, over the ${plan.pr_unit_budget_minutes} min budget`,
    );
    for (const name of shard.packages) {
      assert.ok(!plan.package_model[name].heavy_chain, `${label}: ${name} must not run its unit tests in the pull request`);
    }
  }
  // No pull-request feature suite compiles meerkat-mob, and a pull request
  // runs only the suites it does not leave to the push.
  for (const suite of unitRows.suites) {
    assert.equal(plan.package_model[suite.packages[0]].heavy_chain, false, `${label}: ${suite.name} must not run in the pull request`);
    if (plan.event === "pull_request") assert.equal(suite.pull_request, true, `${label}: ${suite.name} runs on the push only`);
  }
  assertEventSplit(plan, label);
  // The push-to-main plan covers the whole workspace: the packed shards
  // plus the archived (build once, partitioned run) lanes, never both.
  const archivedPackages = plan.main_archive_builds
    .filter((build) => !build.package_flags.includes("--features"))
    .map((build) => build.package_flags.replace(/^-p /, ""));
  const packed = unitLanesOf(plan.main_unit_shards, label).flatMap((shard) => shard.packages);
  for (const name of archivedPackages) {
    assert.ok(!packed.includes(name), `${label}: archived package ${name} is not also packed`);
  }
  assert.deepEqual(
    [...packed, ...archivedPackages].sort(),
    Object.keys(plan.package_model).sort(),
    `${label}: main unit shards and archived lanes cover every workspace package`,
  );
  assertArchivedLanes(plan, label);
  assertHookParity(plan, label);
}

// A plan's unit lanes from its unit rows: the rows of a partitioned lane
// (PARTITIONED_UNIT_PACKAGES) are one lane, the others a lane each. The rows
// of a partitioned lane are the same build, field for field, named
// "<lane> k of n" and running `--partition hash:k/n` for every k of 1..n,
// once each and in order, so together they run each of its tests once.
function unitLanesOf(rows, label) {
  const lanes = [];
  const partitioned = new Map();
  for (const row of rows) {
    if (row.partition === undefined) {
      lanes.push(row);
      continue;
    }
    const match = row.partition.match(/^hash:([1-9][0-9]*)\/([1-9][0-9]*)$/);
    assert.ok(match, `${label}: ${row.name} runs a hash partition, not ${row.partition}`);
    const [, index, count] = match;
    const suffix = ` ${index} of ${count}`;
    assert.ok(row.name.endsWith(suffix), `${label}: ${row.name} is named after its partition ${row.partition}`);
    const lane = { ...row, name: row.name.slice(0, -suffix.length) };
    delete lane.partition;
    const key = JSON.stringify(lane);
    if (!partitioned.has(key)) {
      partitioned.set(key, { lane, count: Number(count), indices: [] });
      lanes.push(lane);
    }
    const group = partitioned.get(key);
    assert.equal(Number(count), group.count, `${label}: ${row.name} counts the partitions of its lane`);
    group.indices.push(Number(index));
  }
  for (const { lane, count, indices } of partitioned.values()) {
    assert.ok(count >= 2, `${label}: ${lane.name} has at least two partitions`);
    assert.deepEqual(indices, Array.from({ length: count }, (_, i) => i + 1), `${label}: ${lane.name} runs every partition once`);
  }
  return lanes;
}

// A plan's unit rows: the unit lane's in a pull request (or a release
// projection); on a push, which runs no unit lane, the main unit lane's rows
// that unit_rows_on_main names.
function unitRowsOf(plan, label) {
  if (plan.event === "pull_request" || plan.release_projection) {
    assert.deepEqual(plan.unit_rows_on_main, [], `${label}: only a push runs unit rows on main`);
    return { shards: plan.unit_shards, suites: plan.unit_feature_shards };
  }
  assert.deepEqual(plan.unit_shards, [], `${label}: a push runs no unit lane`);
  assert.deepEqual(plan.unit_feature_shards, [], `${label}: a push runs no unit feature rows`);
  const named = new Set(plan.unit_rows_on_main);
  const shards = plan.main_unit_shards.filter((row) => named.has(row.name));
  const suites = plan.main_feature_unit_shards.filter((row) => named.has(row.name));
  assert.equal(shards.length + suites.length, named.size, `${label}: every unit row on main is a main unit row`);
  return { shards, suites };
}

const unitOf = (plan) => unitRowsOf(plan, `${plan.event} plan`);

// A push runs no unit lane, and loses nothing by it: every row the unit lane
// would run for the same change (the pull request's unit shards, which the
// planner packs the same way for either event, and every feature suite of
// those packages, the ones a pull request leaves to the push included) is a
// row of the push's main unit lane, field for field: the same name, the same
// `-p` set (Cargo unifies features across the packages of one run), the same
// features and the same test-thread stack. assertUnitJobsMatch shows that
// the two jobs run a row with the same steps.
function assertUnitRowsOnMain(paths, label) {
  const push = planFor(paths, ["--event", "push"]);
  const pr = planFor(paths, ["--event", "pull_request"]);
  assert.deepEqual(push.unit_shards, [], `${label}: a push runs no unit lane`);
  assert.deepEqual(push.unit_feature_shards, [], `${label}: a push runs no unit feature rows`);
  const suiteNames = new Set([
    ...pr.unit_feature_shards.map((suite) => suite.name),
    ...(pr.deferred_to_push?.unit_feature_suites ?? []),
  ]);
  // A pull request's own suites, and the suites' definitions in its plan:
  // the rows the unit lane would run on a push.
  const expectedSuites = pr.main_feature_unit_shards.filter((suite) => suiteNames.has(suite.name));
  assert.equal(expectedSuites.length, suiteNames.size, `${label}: every unit suite is a defined feature suite`);
  for (const suite of pr.unit_feature_shards) {
    assert.deepEqual(expectedSuites.find((row) => row.name === suite.name), suite, `${label}: ${suite.name} is defined once`);
  }
  const expected = [...pr.unit_shards, ...expectedSuites];
  assert.deepEqual(push.unit_rows_on_main, expected.map((row) => row.name), `${label}: the push names the unit rows it runs on main`);
  const mainRows = [...push.main_unit_shards, ...push.main_feature_unit_shards];
  for (const row of expected) {
    assert.deepEqual(mainRows.filter((candidate) => candidate.name === row.name), [row], `${label}: unit row ${row.name} runs verbatim on main`);
  }
  // The rest of the workspace still runs on main, in rows of its own.
  const unitPackages = new Set(pr.unit_shards.flatMap((shard) => shard.packages));
  for (const shard of push.main_unit_shards.filter((row) => !push.unit_rows_on_main.includes(row.name))) {
    for (const name of shard.packages) assert.ok(!unitPackages.has(name), `${label}: ${name} runs once on main`);
  }
  return push;
}

// The unit and main-unit jobs run a row with the same steps. They differ
// only where a lane names itself: the job's name, condition, matrix and
// timeout, the test step's name and its empty-list error, and the log
// artifact's name. Every other line, the setup, the environment and the
// nextest command with its profile and flags included, is the same text.
const UNIT_JOB_LINES_THAT_DIFFER = [
  /^    name: (Main u|U)nit tests \(\$\{\{ matrix\.name \}\}\)$/,
  /^    if: \$\{\{ .* \}\}$/,
  /^      matrix: \$\{\{ fromJSON\(needs\.changes\.outputs\.(main_)?unit_shard_matrix\) \}\}$/,
  /^    timeout-minutes: \d+$/,
  /^      - name: Unit tests \((changed packages|workspace shard), lib and bin targets\)$/,
  /^          \[\[ -n "\$\{PACKAGES\}" \]\] \|\| \{ echo "::error::empty package list in a (main )?unit lane"; exit 1; \}$/,
  /^          name: sccache-log-(main-)?unit-\$\{\{ matrix\.name \}\}-\$\{\{ github\.run_id \}\}-\$\{\{ github\.run_attempt \}\}$/,
];
function jobLines(workflow, job) {
  const lines = workflow.split("\n");
  const start = lines.indexOf(`  ${job}:`);
  assert.ok(start >= 0, `ci.yml has a ${job} job`);
  let end = start + 1;
  while (end < lines.length && !/^ {0,2}\S/.test(lines[end])) end += 1;
  // Comments are not compared, shellcheck directives are: a copy keeps them.
  return lines
    .slice(start + 1, end)
    .filter((line) => line.trim() !== "" && (!line.trim().startsWith("#") || line.trim().startsWith("# shellcheck ")));
}
function assertUnitJobsMatch(workflow, label) {
  const unit = jobLines(workflow, "unit");
  const main = jobLines(workflow, "main-unit");
  assert.equal(unit.length, main.length, `${label}: the unit and main-unit jobs have the same lines`);
  unit.forEach((line, index) => {
    if (line === main[index]) return;
    const allowed = UNIT_JOB_LINES_THAT_DIFFER.some((pattern) => pattern.test(line) && pattern.test(main[index]));
    assert.ok(allowed, `${label}: the unit and main-unit jobs differ in a line that is not a lane name: ${JSON.stringify(line)} / ${JSON.stringify(main[index])}`);
  });
  // One run of the row, and the known-red ratchet (a row with known reds
  // runs them alone after it, under their bounded profile).
  const nextest = unit.filter((line) => line.includes("nextest run"));
  const row = nextest.filter((line) => line.includes("--profile ci-pr "));
  assert.equal(row.length, 1, `${label}: one nextest run of the row in the unit job`);
  assert.ok(main.includes(row[0]), `${label}: main-unit runs the unit job's nextest command`);
  const ratchet = nextest.filter((line) => !row.includes(line));
  assert.equal(ratchet.length, 1, `${label}: the unit job's only other nextest run is the known-red ratchet`);
  assert.match(ratchet[0], /--profile ci-known-reds -E "\$\{KNOWN_REDS\}"/, `${label}: the ratchet runs the known reds alone`);
  assert.ok(main.includes(ratchet[0]), `${label}: main-unit runs the unit job's ratchet`);
}

// What each event runs. The push runs every lane over every target; a pull
// request (outside a release projection) leaves the closure-tests rows, the
// closure check, the feature checks, the governed JSONL and confinement
// lanes, the mob-chain integration suites, the `pullRequest: false` feature
// suites and the mob-chain test targets' Clippy to the push, and says so.
function assertEventSplit(plan, label) {
  const chain = (name) => plan.package_model[name]?.heavy_chain === true;
  if (plan.event === "push" || plan.release_projection) {
    if (plan.event === "push") assert.equal(plan.deferred_to_push, null, `${label}: a push defers nothing`);
    for (const shard of plan.shards) {
      assert.equal(shard.targets, "--all-targets", `${label}: ${shard.name} lints every target on the push`);
    }
    assert.equal(plan.closure_check, plan.rust_changed, `${label}: the push checks the closure on a Rust change`);
    assert.equal(plan.rustdoc, plan.rust_changed, `${label}: the push runs the workspace rustdoc on a Rust change`);
    assert.equal(plan.governed_jsonl, plan.rust_changed, `${label}: the push runs governed JSONL on a Rust change`);
    assert.equal(plan.mcp_confinement_positive, plan.rust_changed, `${label}: the push runs the confinement lane on a Rust change`);
    // Every Rust change is an ordinary reason: the CI gate keeps such a lane
    // in a pull request's budget (a push enforces none).
    assert.deepEqual(
      plan.guard_lane_reasons,
      plan.rust_changed ? { governed_jsonl: ["every_rust_change"], mcp_confinement_positive: ["every_rust_change"] } : {},
      `${label}: the push's guard lane reasons`,
    );
    return;
  }
  assert.equal(plan.event, "pull_request", `${label}: event`);
  const deferred = plan.deferred_to_push;
  assert.ok(deferred, `${label}: a pull request names what it leaves to the push`);
  assert.deepEqual(plan.closure_test_rows, [], `${label}: no closure-tests row in a pull request`);
  assert.deepEqual(plan.feature_check_jobs, [], `${label}: no feature-combination job in a pull request`);
  assert.equal(plan.closure_check, false, `${label}: no closure check in a pull request`);
  assert.equal(plan.rustdoc, false, `${label}: no workspace rustdoc in a pull request`);
  // An exact-profile lane runs in a pull request exactly when it touches
  // that lane's own inputs or changes its definition; otherwise the push
  // runs it.
  for (const lane of ["governed_jsonl", "mcp_confinement_positive"]) {
    const runs =
      plan.rust_changed && (touchesGuardLane(lane, plan.changed_paths) || plan.guard_definitions_changed.includes(lane));
    assert.equal(plan[lane], runs, `${label}: ${lane} runs in a pull request only on its own inputs or definition`);
    assert.equal(deferred[lane], plan.rust_changed && !runs, `${label}: ${lane} deferral`);
    // The reason the plan exports for the CI gate's budget is the one that
    // planned the lane; an unknown diff is the only other one.
    const reasons = plan.guard_lane_reasons[lane];
    assert.equal(reasons !== undefined, plan[lane], `${label}: ${lane} has a reason exactly when it runs`);
    if (reasons && reasons.join() !== "diff_unknown") {
      assert.deepEqual(
        reasons,
        [
          ...(plan.guard_definitions_changed.includes(lane) ? ["definition_changed"] : []),
          ...(touchesGuardLane(lane, plan.changed_paths) ? ["inputs_changed"] : []),
        ],
        `${label}: ${lane}'s reasons`,
      );
    }
  }
  assert.equal(deferred.closure_check, plan.rust_changed);
  assert.equal(deferred.rustdoc, plan.rust_changed);
  for (const suite of plan.integration_suites) {
    assert.equal(chain(suite.packages[0]), false, `${label}: integration suite ${suite.name} compiles meerkat-mob`);
  }
  for (const shard of plan.shards) {
    const kinds = new Set(shard.packages.map(chain));
    assert.equal(kinds.size, 1, `${label}: ${shard.name} mixes mob-chain packages with others`);
    if (kinds.has(true)) {
      assert.ok(["--lib", "--bins", "--lib --bins"].includes(shard.targets), `${label}: ${shard.name} lints ${shard.targets}`);
    } else {
      assert.equal(shard.targets, "--all-targets", `${label}: ${shard.name} lints every target`);
    }
  }
  // The chain heads' whole Clippy waits for the push only when the change
  // invalidates meerkat-mob's build (always in workspace mode), and says so.
  // A product's mob-chain packages are chain heads of their own.
  const heads = plan.packages.filter(
    (name) => MOB_CHAIN_CLIPPY_HEADS.includes(name) || (plan.package_model[name].product !== CORE && chain(name)),
  );
  if (plan.mode === "workspace") {
    assert.deepEqual(deferred.clippy_packages, heads, `${label}: a workspace pull request leaves the chain heads' Clippy to the push`);
  }
  assert.ok(deferred.clippy_packages.every((name) => heads.includes(name)), `${label}: only chain heads lint on the push alone`);
  assert.equal(
    deferred.clippy_packages_reason,
    deferred.clippy_packages.length > 0 ? MOB_CHAIN_INVALIDATED : "",
    `${label}: the deferral names its reason`,
  );
  assert.deepEqual(
    deferred.clippy_test_targets,
    plan.packages.filter((name) => chain(name) && !deferred.clippy_packages.includes(name)),
    `${label}: the chain's test targets lint on the push`,
  );
}

const HOOK_STACK = 8 * 1024 * 1024;
// The rows' stack is the one the hook exports for its lanes.
{
  const hook = readFileSync(resolve(root, "scripts/pre-push-unit.sh"), "utf8");
  const exported = [...hook.matchAll(/^export RUST_MIN_STACK="\$\{RUST_MIN_STACK:-(\d+)\}"$/gm)];
  assert.equal(exported.length, 1, "scripts/pre-push-unit.sh exports one default RUST_MIN_STACK");
  assert.equal(Number(exported[0][1]), HOOK_STACK, "the hook's lanes run on the closure rows' stack");
}
const OTHER_E2E_LANES = ["e2e_auth_lane", "e2e_build_lane", "e2e_system_lane", "e2e_live_lane", "e2e_smoke_lane", "e2e_models_lane"];

// The closure-tests rows: the deterministic workspace lanes' coverage
// (scripts/pre-push-unit.sh) for the test scope (the reverse-dependency
// closure plus the readers of changed non-Rust inputs), each row naming
// exactly the binaries it runs. The lib row exists only on a named runner;
// without one the plan leaves the kind(lib) lane to the unit lanes.
function assertHookParity(plan, label) {
  assert.deepEqual(
    plan.test_scope,
    [...new Set([...plan.closure, ...plan.reader_packages])].sort(),
    `${label}: the test scope is the closure plus the non-Rust readers`,
  );
  assert.deepEqual(plan.hook_coverage.missing, [], `${label}: every hook binary in scope has a row`);
  assert.deepEqual(plan.hook_coverage.errors, [], `${label}: every row builds`);
  if (plan.deferred_to_push) {
    // A pull request runs no row; its push runs them, and the coverage
    // above was checked against those rows.
    assert.deepEqual(plan.closure_test_rows, [], `${label}: a pull request runs no closure-tests row`);
    assert.equal(plan.deferred_to_push.closure_test_rows > 0, plan.test_scope.length > 0, `${label}: the push runs the rows`);
    return;
  }
  const libRows = plan.closure_test_rows.filter((row) => row.lane === "lib");
  if (plan.lib_row_runner === null) {
    assert.deepEqual(libRows, [], `${label}: no lib row without a runner for it`);
    assert.deepEqual(plan.hook_retained_lanes, ["workspace unit lane"], `${label}: the unit lanes keep the kind(lib) lane`);
  } else {
    assert.deepEqual(plan.hook_retained_lanes, [], `${label}: a plan with a lib row leaves no lane without a row`);
    assert.equal(plan.hook_coverage.hook_retained, 0, `${label}: and no binary`);
    for (const row of libRows) assert.equal(row.runner, plan.lib_row_runner, `${label}: the lib row runs on its runner`);
  }
  for (const row of plan.closure_test_rows.filter((candidate) => candidate.lane !== "lib")) {
    assert.equal(row.runner, undefined, `${label}: ${row.name} runs on its job's runner`);
  }
  const names = plan.closure_test_rows.map((row) => row.name);
  assert.equal(new Set(names).size, names.length, `${label}: closure test row names are unique`);
  const runs = [];
  for (const row of plan.closure_test_rows) {
    assert.ok(row.binaries.length > 0, `${label}: ${row.name} names its binaries`);
    assert.ok(["ci-unit", "ci-integration"].includes(row.profile), `${label}: ${row.name} profile ${row.profile}`);
    assert.ok(["warn", "fail"].includes(row.no_tests), `${label}: ${row.name} no-tests policy`);
    // Core's rows run on the hook's stack; a product's on its own test
    // stack (ci/products.json test_stack) when it sets one.
    const productStack = PRODUCTS.find((product) => product.name === row.product && product.name !== CORE)?.testStack;
    assert.equal(row.rust_min_stack, productStack ?? HOOK_STACK, `${label}: ${row.name} runs on its product's test stack`);
    for (const feature of row.package_flags.match(/--features (\S+)/)?.[1]?.split(",") ?? []) {
      assert.match(feature, /^[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+$/, `${label}: ${row.name} names each feature's package`);
    }
    // Every Core row but HeadCanonical selects Core's whole workspace (the
    // hook's own selection), so Cargo resolves features exactly as the
    // hook's build does; a product's row selects exactly that product's
    // packages. No row selects the bare workspace (it would mix products).
    assert.notEqual(row.package_flags.trim(), "--workspace", `${label}: ${row.name} mixes the products`);
    if (row.lane !== "headcanonical") {
      assert.equal(row.package_flags, selectionFlags(row.product), `${label}: ${row.name} selects its product's lane selection`);
    }
    if (row.lane === "lib") {
      assert.equal(row.test_flags, "--lib", `${label}: ${row.name} runs lib tests`);
      // --workspace --lib also builds proc-macro crates' tests, which the
      // hook's kind(lib) lane does not run and the row does not name.
      assert.match(row.filter, /^kind\(lib\)( & \(.+\))?$/, `${label}: ${row.name} runs only kind(lib) binaries`);
      assert.deepEqual(row.binaries, row.packages, `${label}: ${row.name} lists one lib binary per package`);
    } else {
      // Every named target is one of the row's binaries, and the reverse (a
      // name shared by several members names all of their binaries).
      const named = [...new Set([...row.test_flags.matchAll(/--test (\S+)/g)].map((match) => match[1]))].sort();
      assert.deepEqual(named, [...new Set(row.binaries.map((binary) => binary.split("::")[1]))].sort(), `${label}: ${row.name} names exactly its binaries`);
      for (const binary of row.binaries) {
        assert.ok(row.packages.includes(binary.split("::")[0]), `${label}: ${row.name} selects the package of ${binary}`);
      }
    }
    // The partitions of one selection run its binaries once between them
    // (the coverage check requires every partition): count the group once.
    if (row.partition !== undefined) {
      assert.match(row.partition, /^count:[1-9][0-9]*\/[1-9][0-9]*$/, `${label}: ${row.name} runs a count partition`);
    }
    if (row.lane !== "headcanonical" && (row.partition === undefined || row.partition.startsWith("count:1/"))) {
      runs.push(...row.binaries);
    }
  }
  assert.equal(new Set(runs).size, runs.length, `${label}: each hook binary runs in one row (HeadCanonical's own build aside)`);
  for (const lane of OTHER_E2E_LANES) {
    assert.ok(!runs.includes(`meerkat-integration-tests::${lane}`), `${label}: ${lane} is outside the hook's fast selection`);
  }
  assert.equal(
    plan.closure_test_rows.some((row) => row.lane === "headcanonical"),
    plan.test_scope.includes("meerkat-mob"),
    `${label}: HeadCanonical runs exactly when meerkat-mob is in scope`,
  );
  assert.equal(
    plan.closure_test_rows.some((row) => row.lane === "e2e-fast"),
    plan.test_scope.includes("meerkat-integration-tests"),
    `${label}: e2e-fast runs exactly when meerkat-integration-tests is in scope`,
  );
}

// Every archive build is executed by exactly its partitions, 1..n of n, and
// names unique archives (they name matrix rows and artifacts).
function assertArchivedLanes(plan, label) {
  const archives = plan.main_archive_builds.map((build) => build.archive);
  assert.equal(new Set(archives).size, archives.length, `${label}: archive names are unique`);
  for (const build of plan.main_archive_builds) {
    assert.match(build.archive, /^[A-Za-z0-9_-]+$/, `${label}: archive ${build.archive} is artifact-safe`);
    assert.ok(build.partitions >= 2, `${label}: ${build.name} fans out`);
    const partitions = plan.main_archive_runs
      .filter((run) => run.archive === build.archive)
      .map((run) => run.partition);
    assert.deepEqual(
      partitions,
      Array.from({ length: build.partitions }, (_, index) => `hash:${index + 1}/${build.partitions}`),
      `${label}: ${build.name} runs every partition exactly once`,
    );
  }
  assert.equal(
    plan.main_archive_runs.length,
    plan.main_archive_builds.reduce((sum, build) => sum + build.partitions, 0),
    `${label}: no partition run without its archive build`,
  );
}

// Core touch: the direct lanes are meerkat-core's; the closure is nearly the
// whole workspace (everything depends on core).
{
  const plan = planFor(["crates/meerkat-core/src/lib.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-core"]);
  assertLanes(plan, "core touch");
  assert.ok(plan.closure.length >= 40, `core closure spans the workspace (${plan.closure.length})`);
  assert.ok(plan.closure.includes("meerkat-mob"), "core closure includes meerkat-mob");
  assert.ok(plan.closure.includes("rkat"), "core closure includes rkat");
  assert.equal(plan.shards.length, 1);
  assert.deepEqual(plan.unit_shards, [], "a push runs no unit lane");
  assert.deepEqual(plan.unit_rows_on_main, ["core"], "the push runs core's unit row on main");
  assert.deepEqual(plan.unit_deferred, []);
  assert.equal(plan.wasm, true, "core is in the wasm runtime closure: the push runs the wasm lanes");
  assert.equal(plan.machine_authority, false);
  // Core is a crate the wasm build compiles, so a pull request runs
  // wasm-check too; the example web suites only for their own inputs.
  const pr = planFor(["crates/meerkat-core/src/lib.rs"], ["--event", "pull_request"]);
  assert.equal(pr.wasm, true, "a pull request runs wasm-check for a crate the web runtime compiles");
  assert.equal(pr.example_web, false, "but not the example web suites");
  assert.deepEqual(pr.unit_shards.map((shard) => shard.packages), [["meerkat-core"]], "core runs its unit tests in the pull request");
  assertLanes(pr, "core touch (pull request)");
}

// The meerkat-mob compile chain: a mob touch gets its clippy lane in the pull
// request and no pull-request unit lane; its unit tests are named as
// deferred to the push-to-main run. The chain is computed from metadata.
{
  const plan = planFor(["crates/meerkat-mob/src/lib.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-mob"]);
  assert.equal(plan.shards.length, 1, "clippy lane for mob");
  assert.deepEqual(plan.unit_shards, [], "no pull-request unit lane may compile meerkat-mob");
  assert.deepEqual(plan.unit_deferred, ["meerkat-mob"]);
  assert.ok(plan.package_model["meerkat-mob"].estimated_minutes > plan.pr_unit_budget_minutes, "mob's own lane models over budget");
  assertLanes(plan, "mob touch");
  for (const name of ["meerkat-mob", "meerkat-rpc", "meerkat-rest", "rkat", "meerkat-mob-mcp", "meerkat-integration-tests", "meerkat-web-runtime", "meerkat-machine-codegen"]) {
    assert.ok(plan.unit_deferred_chain.includes(name), `${name} compiles meerkat-mob in its unit lane and is in the deferred chain`);
  }
  // xtask reaches meerkat-mob only through meerkat-machine-codegen's
  // dev-dependency, which Cargo never builds for xtask: not in the chain.
  for (const name of ["meerkat-core", "meerkat-runtime", "meerkat-session", "meerkat", "xtask", "machine-dsl-tests"]) {
    assert.ok(!plan.unit_deferred_chain.includes(name), `${name} does not compile meerkat-mob in its unit lane`);
  }
}
{
  // A session change runs session's unit lane; rkat stays deferred.
  const plan = planFor(["crates/meerkat-cli/src/main.rs", "crates/meerkat-session/src/lib.rs"]);
  assert.deepEqual(plan.unit_deferred, ["rkat"]);
  assert.deepEqual(unitOf(plan).shards.map((shard) => shard.packages).sort(), [["meerkat-session"]]);
  assertLanes(plan, "cli + session");
}

// Runtime's ordinary unit tests remain in the pull request even when its
// authorization dependency has native facade fixtures in its own dev graph.
// The lane runs as the rows of its hash partitions (below).
{
  const plan = planFor(["crates/meerkat-runtime/src/lib.rs"]);
  assert.equal(plan.pr_unit_budget_minutes, 20, "runtime uses the 20-minute surrogate packing cap");
  assert.deepEqual(
    unitLanesOf(unitOf(plan).shards, "runtime touch").map((shard) => shard.packages).sort(),
    [["meerkat-runtime"]],
    "runtime retains its pull-request unit lane",
  );
  assert.deepEqual(plan.unit_deferred, [], "runtime unit tests are not deferred");
  assert.ok(!plan.unit_deferred_chain.includes("meerkat-runtime"), "runtime remains outside the mob build chain");
  const runtimeLane = unitOf(plan).shards.find((shard) => shard.packages.includes("meerkat-runtime"));
  assert.ok(runtimeLane.estimated_minutes <= plan.pr_unit_budget_minutes, "runtime's unit lane fits the budget");
}

// A partitioned unit lane (PARTITIONED_UNIT_PACKAGES): a pull request runs
// the package's lane as one row per hash partition, each the build of the
// package alone; a push runs the same rows verbatim in the main unit lane;
// the matrices carry each of those rows' partition, and no other row's; and
// the three unit jobs pass it to nextest. A row name holds no character an
// artifact name refuses: the jobs name each row's sccache log after it.
{
  const ARTIFACT_NAME_REFUSES = /[":<>|*?\r\n\\/]/;
  const githubLines = (args) =>
    Object.fromEntries(run(["--format", "github", ...args]).stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  const metadataPackages = workspacePackages(readMetadata());
  assert.ok(PARTITIONED_UNIT_PACKAGES.length > 0, "a package runs a partitioned unit lane");
  for (const { package: pkg, partitions } of PARTITIONED_UNIT_PACKAGES) {
    const lib = metadataPackages.find((one) => one.name === pkg)?.targets.find((target) => target.kind.includes("lib"));
    assert.ok(lib, `${pkg} is a workspace package with a library`);
    const path = relative(root, lib.src_path);
    const expected = Array.from({ length: partitions }, (_, index) => `hash:${index + 1}/${partitions}`);
    const pr = planFor([path], ["--event", "pull_request"]);
    assertLanes(pr, `${pkg} touch (pull request)`);
    const rows = pr.unit_shards.filter((row) => row.packages.includes(pkg));
    assert.deepEqual(rows.map((row) => row.partition), expected, `${pkg}: one pull-request unit row per partition`);
    for (const row of rows) {
      assert.deepEqual(row.packages, [pkg], `${row.name}: the partitioned lane builds ${pkg} alone`);
      assert.doesNotMatch(row.name, ARTIFACT_NAME_REFUSES, `${row.name}: the row name is an artifact name`);
    }
    const push = assertUnitRowsOnMain([path], `${pkg} touch`);
    assertLanes(push, `${pkg} touch (push)`);
    const prMatrix = JSON.parse(githubLines(["--event", "pull_request", "--", path]).unit_shard_matrix).include;
    const pushLines = githubLines(["--event", "push", "--", path]);
    const pushMatrix = bothWaves(pushLines, "main_unit_shard_matrix", "main_unit_rest_shard_matrix");
    for (const [label, matrix] of [["pull request", prMatrix], ["push", pushMatrix]]) {
      assert.deepEqual(
        matrix.filter((row) => row.partition !== undefined).map((row) => [row.name, row.packages, row.partition]),
        rows.map((row) => [row.name, row.package_flags, row.partition]),
        `${pkg} (${label}): the matrix carries the partitioned rows' partitions, and only theirs`,
      );
    }
  }
  const workflow = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  for (const job of ["unit", "main-unit", "main-unit-rest"]) {
    const text = jobLines(workflow, job).join("\n");
    assert.match(text, /^ {10}PARTITION: \$\{\{ matrix\.partition \}\}$/m, `${job} reads the row's partition`);
    assert.match(
      text,
      /^ {10}if \[\[ -n "\$\{PARTITION\}" \]\]; then\n {12}partition_args=\(--partition "\$\{PARTITION\}"\)\n {10}fi$/m,
      `${job} passes a partition only for a row that sets one`,
    );
    assert.match(
      text,
      /nextest run \$\{PACKAGES\} \$\{TARGETS\} --profile ci-pr "\$\{partition_args\[@\]\}" "\$\{filter_args\[@\]\}" \\$/m,
      `${job} runs the row's partition and filter`,
    );
  }
}

// A feature suite built with other packages, or with known reds: the
// toolkit-mobkit[native-host] suite builds toolkit-mobkit with meerkat-connect
// (which turns its native host on), runs toolkit-mobkit's tests only and
// none of its known reds, then runs the known reds that fail every time and
// needs each one to fail. Its known-red list and the crate's test-debt page
// name the same tests under the same classes.
{
  const suite = FEATURE_UNIT_SUITES.find((candidate) => candidate.package === "toolkit-mobkit" && candidate.id === "native-host");
  assert.ok(suite, "toolkit-mobkit has a native-host feature suite");
  assert.deepEqual(suite.with, ["meerkat-connect"], "the native-host suite builds with meerkat-connect");
  assert.equal(suite.pullRequest, false, "the native-host suite runs on the push only");
  assert.equal(featureSuiteFlags(suite), "-p toolkit-mobkit -p meerkat-connect --features toolkit-mobkit/native-host");
  const steady = TOOLKIT_MOBKIT_KNOWN_REDS.filter((entry) => !entry.intermittent);
  const filters = featureSuiteFilters(suite);
  assert.ok(filters.filter.startsWith("package(toolkit-mobkit) and not (test(="), "the suite runs its package's tests, without its known reds");
  for (const entry of TOOLKIT_MOBKIT_KNOWN_REDS) {
    assert.ok(filters.filter.includes(`test(=${entry.test})`), `the suite excludes ${entry.test}`);
    assert.equal(filters.known_reds.includes(`test(=${entry.test})`), !entry.intermittent, `the ratchet runs ${entry.test} unless it is intermittent`);
  }
  assert.ok(filters.known_reds.startsWith("package(toolkit-mobkit) and (test(="), "the ratchet runs the package's steady known reds");
  assert.equal(filters.known_red_count, steady.length, "the ratchet counts the steady known reds");
  // A suite without companions or known reds keeps its plain flags and no filter.
  const plain = FEATURE_UNIT_SUITES.find((candidate) => candidate.package === "meerkat-connect" && candidate.id === "console");
  assert.equal(featureSuiteFlags(plain), "-p meerkat-connect --features console");
  assert.deepEqual(featureSuiteFilters(plain), {});

  // The page and the list agree, and every drift between them is refused.
  const page = readFileSync(resolve(root, TOOLKIT_MOBKIT_TEST_DEBT), "utf8");
  checkKnownRedsDoc(page, TOOLKIT_MOBKIT_KNOWN_REDS, TOOLKIT_MOBKIT_TEST_DEBT);
  const doc = "# t\n\n## Class one\n\n- `a::b`\n\n## Class two\n\n| Test | Fix |\n|---|---|\n| `c::d` | x |\n";
  const list = [{ test: "a::b", anchor: "class-one" }, { test: "c::d", anchor: "class-two" }];
  checkKnownRedsDoc(doc, list, "doc");
  assert.throws(() => checkKnownRedsDoc(doc, [list[0]], "doc"), /c::d \(#class-two\) is not a known red/, "a page-only test is refused");
  assert.throws(() => checkKnownRedsDoc(doc, [...list, { test: "e::f", anchor: "class-one" }], "doc"), /e::f is not on doc/, "a list-only test is refused");
  assert.throws(() => checkKnownRedsDoc(doc, [list[0], { test: "c::d", anchor: "class-one" }], "doc"), /names #class-one, but doc lists it under #class-two/, "a moved test is refused");
  assert.throws(() => checkKnownRedsDoc(doc, [...list, list[0]], "doc"), /a::b is listed twice/, "a repeated entry is refused");
  assert.throws(() => checkKnownRedsDoc(`${doc}- \`a::b\`\n`, list, "doc"), /a::b is listed twice/, "a test the page lists twice is refused");

  // The push plan runs the suite as a main unit row; a pull request leaves it.
  const path = "connect/crates/toolkit-mobkit/src/lib.rs";
  const push = planFor([path], ["--event", "push"]);
  const row = push.main_feature_unit_shards.find((candidate) => candidate.name === "toolkit-mobkit[native-host]");
  assert.ok(row, "a toolkit-mobkit change runs the native-host suite on the push");
  assert.equal(row.package_flags, featureSuiteFlags(suite));
  assert.equal(row.filter, filters.filter);
  assert.equal(row.known_reds, filters.known_reds);
  assert.equal(row.known_red_count, filters.known_red_count);
  const pr = planFor([path], ["--event", "pull_request"]);
  assert.ok(!pr.unit_feature_shards.some((candidate) => candidate.name === "toolkit-mobkit[native-host]"), "a pull request leaves the native-host suite to the push");
  const githubLinesFor = (args) =>
    Object.fromEntries(run(["--format", "github", ...args]).stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  const pushLines = githubLinesFor(["--event", "push", "--", path]);
  const matrixRow = bothWaves(pushLines, "main_unit_shard_matrix", "main_unit_rest_shard_matrix").find((candidate) => candidate.name === row.name);
  assert.ok(matrixRow, "the native-host suite is a main unit matrix row");
  assert.deepEqual(
    [matrixRow.packages, matrixRow.filter, matrixRow.known_reds, matrixRow.known_red_count],
    [row.package_flags, row.filter, row.known_reds, String(row.known_red_count)],
    "the matrix row carries the suite's filter and known reds",
  );

  // The unit jobs read the row's filter and known reds; the ratchet runs the
  // known reds alone under the bounded ci-known-reds profile and needs every
  // one to fail.
  const workflow = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  for (const job of ["unit", "main-unit", "main-unit-rest"]) {
    const text = jobLines(workflow, job).join("\n");
    assert.match(text, /^ {10}FILTER: \$\{\{ matrix\.filter \}\}$/m, `${job} reads the row's filter`);
    assert.match(text, /^ {10}KNOWN_REDS: \$\{\{ matrix\.known_reds \}\}$/m, `${job} reads the row's known reds`);
    assert.match(text, /^ {10}KNOWN_RED_COUNT: \$\{\{ matrix\.known_red_count \}\}$/m, `${job} reads the row's known-red count`);
    assert.match(
      text,
      /^ {10}if \[\[ -n "\$\{FILTER\}" \]\]; then\n {12}filter_args=\(-E "\$\{FILTER\}"\)\n {10}fi$/m,
      `${job} passes a filter only for a row that sets one`,
    );
    assert.match(text, /--profile ci-known-reds -E "\$\{KNOWN_REDS\}"/, `${job} runs the known reds alone under the bounded profile`);
    assert.match(
      text,
      /\$\{KNOWN_RED_COUNT\} tests\? run: 0 passed, \$\{KNOWN_RED_COUNT\} failed\(,\|\$\)/,
      `${job} needs every known red to fail`,
    );
  }
  const nextest = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  assert.match(nextest, /^\[profile\.ci-known-reds\]\n(?:#[^\n]*\n)*inherits = "fast"\nslow-timeout = \{ period = "30s", terminate-after = 2 \}$/m, "the ratchet profile bounds each test");
}

// Budget by construction: every package outside the chain models under the
// pull-request unit budget on its own, so no changed-package plan can
// exceed it; every package inside the chain is deferred.
{
  const plan = planFor(["Cargo.lock"]);
  for (const [name, model] of Object.entries(plan.package_model)) {
    if (!model.heavy_chain) {
      assert.ok(model.estimated_minutes <= plan.pr_unit_budget_minutes, `${name} models ${model.estimated_minutes} min alone, over budget`);
    }
  }
  assert.deepEqual(plan.unit_deferred, plan.unit_deferred_chain, "workspace mode defers exactly the chain");
  // Core's rows stay near the shard cap; each product packs apart, into at
  // most one row per package outside the chain.
  const coreUnitRows = unitOf(plan).shards.filter((shard) => (shard.product ?? CORE) === CORE);
  assert.ok(coreUnitRows.length <= 11, `workspace unit rows stay near the shard cap: ${coreUnitRows.length}`);
  for (const product of PRODUCTS.filter((candidate) => candidate.name !== CORE)) {
    const rows = unitOf(plan).shards.filter((shard) => shard.product === product.name);
    const packages = rows.flatMap((shard) => shard.packages);
    assert.ok(rows.length <= Math.max(1, packages.length), `${product.name} unit rows: ${rows.length} for ${packages.length} packages`);
  }
  // A package whose unit tests run long gets a lane of its own.
  for (const name of UNIT_SOLO_PACKAGES) {
    const lanes = unitOf(plan).shards.filter((shard) => shard.packages.includes(name));
    assert.deepEqual(lanes.map((shard) => shard.packages), [[name]], `${name} runs its unit tests in a lane of its own`);
  }
  // The nested-Cargo canaries' binary (the downstream-cargo-canary test
  // group) runs as count partitions of its own, one per canary, and in no
  // packed row; the coverage check credits it only with every partition.
  const canaryTest = "agent_builder_policy_canary";
  const canaryRows = plan.closure_test_rows.filter((row) => row.test_flags === `--test ${canaryTest}`);
  assert.deepEqual(canaryRows.map((row) => row.partition), ["count:1/3", "count:2/3", "count:3/3"], "one count partition per canary");
  for (const row of canaryRows) {
    assert.deepEqual(row.binaries, [`meerkat::${canaryTest}`]);
    assert.equal(row.package_flags, selectionFlags(CORE), "a canary row is a row over Core's whole workspace");
    assert.equal(row.no_tests, "fail", "an empty canary partition fails");
  }
  assert.ok(
    plan.closure_test_rows.every((row) => row.partition || !row.test_flags.split(/\s+/).includes(canaryTest)),
    "no packed row holds the canaries' binary",
  );
  const canaryCoverage = checkHookCoverage(plan);
  assert.equal(
    canaryCoverage.map[`meerkat::test::${canaryTest}`],
    "closure-tests: tests meerkat canaries 1/3, tests meerkat canaries 2/3, tests meerkat canaries 3/3",
  );
  for (const [label, change] of [
    ["a dropped partition", (rows) => rows.filter((row) => row.partition !== "count:2/3")],
    ["a repeated partition", (rows) => rows.map((row) => (row.partition === "count:2/3" ? { ...row, partition: "count:1/3" } : row))],
    ["partitions of another count", (rows) => rows.map((row) => (row.partition === "count:3/3" ? { ...row, partition: "count:3/4" } : row))],
    ["a partition of another selection", (rows) => rows.map((row) => (row.partition === "count:1/3" ? { ...row, profile: "fast" } : row))],
  ]) {
    const copy = JSON.parse(JSON.stringify(plan));
    copy.closure_test_rows = change(copy.closure_test_rows);
    const result = checkHookCoverage(copy);
    assert.ok(result.missing.includes(`meerkat::test::${canaryTest}`), `${label} leaves the canaries' binary uncovered`);
    assert.ok(result.errors.some((error) => error.includes("every partition 1..N of one selection is required")), `${label} is reported`);
  }
  const workspaceGithub = run(["--format", "github", "--", "Cargo.lock"]);
  assert.equal(workspaceGithub.status, 0, workspaceGithub.stderr);
  // The rows of both waves (long rows first): the first wave's matrix and the rest's.
  const closureInclude = ["closure_test_matrix", "closure_test_rest_matrix"].flatMap((key) => {
    const line = workspaceGithub.stdout.split("\n").find((candidate) => candidate.startsWith(`${key}=`));
    return JSON.parse(line.slice(key.length + 1)).include.filter((row) => row.name !== "none");
  });
  assert.deepEqual(
    closureInclude.filter((row) => row.partition !== "").map((row) => [row.name, row.partition]).sort(),
    [["tests meerkat canaries 1/3", "count:1/3"], ["tests meerkat canaries 2/3", "count:2/3"], ["tests meerkat canaries 3/3", "count:3/3"]],
    "the matrix carries each canary row's partition, and only theirs",
  );
  assertLanes(plan, "workspace with exceptions");
}

// Exact-profile lanes on a pull request: a change to one of a lane's inputs
// runs that lane (and the other only on its own inputs); a change elsewhere
// in the same crates leaves both to the push, which runs them on every Rust
// change.
{
  for (const [lane, inputs] of Object.entries(GUARD_LANE_INPUTS)) {
    for (const input of inputs) {
      const path = input.endsWith("/")
        ? join(input, readdirSync(resolve(root, input)).filter((name) => name.endsWith(".rs")).sort()[0])
        : input;
      const plan = planFor([path], ["--event", "pull_request"]);
      assert.equal(plan[lane], true, `a pull request touching ${path} runs ${lane}`);
      assert.equal(plan.deferred_to_push[lane], false, `a pull request touching ${path} leaves ${lane} to no push`);
      for (const other of Object.keys(GUARD_LANE_INPUTS).filter((name) => name !== lane)) {
        assert.equal(plan[other], touchesGuardLane(other, [path]), `a pull request touching ${path} runs ${other} only on its inputs`);
      }
    }
  }
  // Their neighbours (unchanged definitions: the working tree is the base)
  // run neither.
  for (const path of [
    "crates/meerkat-rpc/src/lib.rs",
    "crates/meerkat-mcp/src/connection.rs",
    "crates/meerkat-sandbox/src/lib.rs",
    "crates/meerkat-tools/src/builtin/shell/mod.rs",
  ]) {
    const plan = planFor([path], ["--event", "pull_request", "--base-tree", root]);
    for (const lane of Object.keys(GUARD_LANE_INPUTS)) {
      assert.equal(plan[lane], false, `a pull request touching only ${path} leaves ${lane} to the push`);
      assert.equal(plan.deferred_to_push[lane], true, `a pull request touching only ${path} names ${lane} as deferred`);
    }
    const push = planFor([path]);
    for (const lane of Object.keys(GUARD_LANE_INPUTS)) {
      assert.equal(push[lane], true, `the push of ${path} runs ${lane}`);
    }
  }
}

// A pull request that changes an exact-profile lane's definition
// (GUARD_LANE_DEFINITIONS: its ci.yml job, the confinement rule and
// assignments in the Makefile, a selected module's declaration) runs that
// lane; an edit elsewhere in the same file, or to a comment or blank line of
// a lane's job, does not. Each base is the working tree's file with one edit
// (--base-tree). Without a base copy every listed definition file differs,
// and without a diff every lane runs.
{
  const baseTree = mkdtempSync(join(tmpdir(), "ci-lanes-guard-definitions-"));
  try {
    for (const [path, from, to, lanes] of [
      [".github/workflows/ci.yml", '"governed_jsonl::tests::rpc_runtimes_compose_the_realms_own_schedule_service",\n', "", ["governed_jsonl"]],
      [".github/workflows/ci.yml", "run: make test-mcp-confinement-positive\n", "run: make test-mcp-confinement-positive || true\n", ["mcp_confinement_positive"]],
      [".github/workflows/ci.yml", "  bazel-graph:\n    name: ", "  bazel-graph:\n    name: Old ", []],
      [".github/workflows/ci.yml", "    # Native dependency changes must retain", "    # Native dependency changes keep", []],
      [".github/workflows/ci.yml", "          # No --nocapture: libtest captures", "          # Without --nocapture libtest captures", []],
      [".github/workflows/ci.yml", "    # step before the tests prints the restriction", "\n    # step before the tests prints its restriction", []],
      ["Makefile", "CONFINEMENT_SHELL_POSITIVE_COUNT := 4\n", "CONFINEMENT_SHELL_POSITIVE_COUNT := 3\n", ["mcp_confinement_positive"]],
      ["Makefile", "--no-tests=fail --no-fail-fast -E", "--no-tests=fail -E", ["mcp_confinement_positive"]],
      ["Makefile", "test-mob-dense-topology:\n", "test-mob-dense-topology: build\n", []],
      [".config/nextest.toml", "[profile.default]\n", "[profile.default]\ndefault-filter = 'not test(/confinement/)'\n", ["mcp_confinement_positive"]],
      [
        ".config/nextest.toml",
        "[profile.ci-unit]\n",
        "[[profile.default.overrides]]\nplatform = 'cfg(target_os = \"linux\")'\ndefault-filter = 'not test(/confinement/)'\n\n[profile.ci-unit]\n",
        ["mcp_confinement_positive"],
      ],
      [".config/nextest.toml", "artifacts)'\nthreads-required = \"num-cpus\"", "artifacts)'\nthreads-required = 2", []],
      [".config/nextest.toml", "# Archived lanes (ci.yml main-unit", "# Archived lanes (the main-unit job", []],
      ["crates/meerkat-mcp/src/connection.rs", "#[cfg(test)]\n#[path = \"process_confinement_tests.rs\"]", "#[cfg(all(test, target_os = \"macos\"))]\n#[path = \"process_confinement_tests.rs\"]", ["mcp_confinement_positive"]],
      ["crates/meerkat-rpc/src/lib.rs", "#[cfg(feature = \"local-authorization\")]\npub mod governed_jsonl;", "pub mod governed_jsonl;", ["governed_jsonl"]],
      ["crates/meerkat-rpc/src/lib.rs", "pub mod handlers;\n", "pub mod handlers;\npub mod retired_neighbour;\n", []],
    ]) {
      const text = readFileSync(resolve(root, path), "utf8");
      assert.equal(text.split(from).length, 2, `${path} holds ${JSON.stringify(from)} once`);
      mkdirSync(dirname(join(baseTree, path)), { recursive: true });
      writeFileSync(join(baseTree, path), text.replace(from, to));
      const label = `a pull request that edits ${JSON.stringify(from)} in ${path}`;
      const plan = planFor([path], ["--event", "pull_request", "--base-tree", baseTree]);
      assert.deepEqual(plan.guard_definitions_changed, lanes, `${label}: changed definitions`);
      for (const lane of Object.keys(GUARD_LANE_INPUTS)) assert.equal(plan[lane], lanes.includes(lane), `${label}: ${lane}`);
      assert.deepEqual(
        plan.guard_lane_reasons,
        Object.fromEntries(lanes.map((lane) => [lane, ["definition_changed"]])),
        `${label}: the definition is the reason (none for a comment or an edit elsewhere)`,
      );
      assertLanes(plan, label);
    }
  } finally {
    rmSync(baseTree, { recursive: true, force: true });
  }
  const noBase = planFor(["Makefile"], ["--event", "pull_request"]);
  assert.deepEqual(noBase.guard_definitions_changed, ["mcp_confinement_positive"], "no base copy: the Makefile's lane");
  const noDiff = run(["--format", "json", "--event", "pull_request"]);
  assert.equal(noDiff.status, 0, noDiff.stderr);
  const unknown = JSON.parse(noDiff.stdout);
  assert.equal(unknown.diff, "no diff base supplied");
  assert.deepEqual(unknown.guard_definitions_changed, Object.keys(GUARD_LANE_INPUTS), "no diff: every definition may have changed");
  for (const lane of Object.keys(GUARD_LANE_INPUTS)) assert.equal(unknown[lane], true, `no diff: ${lane} runs`);
  // Its only reason is the unknown diff, never a changed definition: the CI
  // gate keeps it in the budget.
  assert.equal(unknown.mode, "workspace");
  assert.deepEqual(
    unknown.guard_lane_reasons,
    { governed_jsonl: ["diff_unknown"], mcp_confinement_positive: ["diff_unknown"] },
    "no diff: every lane runs for the unknown diff alone",
  );
  // A workspace-mode pull request with a known diff runs a guard lane for
  // its inputs or definition like any other pull request: a Makefile change
  // plans the whole workspace, and so does Cargo.toml.
  const bothTree = mkdtempSync(join(tmpdir(), "ci-lanes-guard-reasons-"));
  try {
    const makefile = readFileSync(resolve(root, "Makefile"), "utf8");
    const count = makefile.match(/^CONFINEMENT_MCP_POSITIVE_COUNT := (\d+)$/m);
    assert.ok(count, "the Makefile counts the confinement lane's MCP cases");
    writeFileSync(join(bothTree, "Makefile"), makefile.replace(count[0], `CONFINEMENT_MCP_POSITIVE_COUNT := ${Number(count[1]) + 1}`));
    const both = planFor(["Makefile", "crates/meerkat-mcp/src/process_confinement_tests.rs"], ["--event", "pull_request", "--base-tree", bothTree]);
    assert.equal(both.mode, "workspace", "a Makefile change plans the workspace");
    assert.deepEqual(
      both.guard_lane_reasons,
      { mcp_confinement_positive: ["definition_changed", "inputs_changed"] },
      "a lane whose definition and inputs both changed carries both reasons",
    );
    assertLanes(both, "a definition and an input of the confinement lane");
  } finally {
    rmSync(bothTree, { recursive: true, force: true });
  }
  const inputs = planFor(["Cargo.toml", "crates/meerkat-rpc/src/governed_jsonl/tests.rs"], ["--event", "pull_request"]);
  assert.equal(inputs.mode, "workspace", "a Cargo.toml change plans the workspace");
  assert.deepEqual(inputs.guard_lane_reasons, { governed_jsonl: ["inputs_changed"] }, "a workspace-mode pull request's inputs reason");
  assert.equal(inputs.governed_jsonl, true);
  assert.equal(inputs.mcp_confinement_positive, false);
}

// The default nextest profile's selection in every TOML spelling: a change to
// it changes the extract the planner compares, and a change elsewhere (another
// profile, an override's threads, a comment) does not.
{
  const filter = "default-filter = 'not test(/confinement/)'";
  for (const [before, after] of [
    ["[ profile.default ]\nretries = 1\n", `[ profile.default ]\nretries = 1\n${filter}\n`],
    ['["profile"."default"]\nretries = 1\n', `["profile"."default"]\nretries = 1\n${filter}\n`],
    ["profile.default.retries = 1\n", `profile.default.retries = 1\nprofile.default.${filter}\n`],
    ["[profile]\ndefault.retries = 1\n", `[profile]\ndefault.retries = 1\ndefault.${filter}\n`],
    ["[profile]\n", `[profile]\ndefault = { ${filter} }\n`],
    ["[[profile.default.overrides]]\nfilter = 'test(a)'\n", `[[profile.default.overrides]]\nfilter = 'test(a)'\n  ${filter}\n`],
    // A value that spans lines: a dotted key's multi-line string or array.
    ["profile.default.default-filter = '''\nnot test(/a/)\n'''\n", "profile.default.default-filter = '''\nnot test(/b/)\n'''\n"],
    ['[profile]\ndefault.default-filter = """\nnot test(/a/)\n"""\n', '[profile]\ndefault.default-filter = """\nnot test(/b/)\n"""\n'],
    ["profile.default.overrides = [\n  { filter = 'test(a)', default-filter = 'test(b)' },\n]\n", "profile.default.overrides = [\n  { filter = 'test(a)', default-filter = 'test(c)' },\n]\n"],
  ]) {
    assert.notEqual(nextestDefaultSelection(before), nextestDefaultSelection(after), `the default selection sees ${JSON.stringify(after)}`);
  }
  for (const [before, after] of [
    ["profile.ci-unit.retries = 1\n", `profile.ci-unit.${filter}\n`],
    ["profile.ci-unit.default-filter = '''\nnot test(/a/)\n'''\n", "profile.ci-unit.default-filter = '''\nnot test(/b/)\n'''\n"],
    ["[profile]\nci-unit.retries = 1\n", `[profile]\nci-unit.${filter}\n`],
    ["[[profile.default.overrides]]\nfilter = 'test(a)'\nthreads-required = 2\n", "[[profile.default.overrides]]\nfilter = 'test(a)'\nthreads-required = 4\n"],
    ["[ profile.default ]\n# one\nretries = 1\n", "[ profile.default ]\n# two\nretries = 1\n"],
  ]) {
    assert.equal(nextestDefaultSelection(before), nextestDefaultSelection(after), `the default selection ignores ${JSON.stringify(after)}`);
  }
}

// No chain exception: meerkat-rpc's default-feature unit row (its MCP restart
// controls) is deferred with the rest of the chain, in a pull request and on
// the push, and the push-to-main unit lanes run it.
{
  for (const extra of [[], ["--event", "pull_request"]]) {
    const rpc = planFor(["crates/meerkat-rpc/src/governed_jsonl/tests.rs"], extra);
    assert.deepEqual(rpc.packages, ["meerkat-rpc"]);
    assert.deepEqual(rpc.unit_deferred, ["meerkat-rpc"], "an rpc change defers rpc's unit tests to main");
    assert.deepEqual(rpc.unit_shards, [], "no pull-request unit row compiles meerkat-mob");
    assert.equal(rpc.package_model["meerkat-rpc"].heavy_chain, true, "rpc still compiles meerkat-mob");
    assert.ok(rpc.main_unit_shards.some((shard) => shard.packages.includes("meerkat-rpc")), "main-unit runs rpc's row");
    assert.equal(Object.hasOwn(rpc, "unit_chain_exceptions"), false, "the exception list is gone");
    assertLanes(rpc, `rpc touch ${extra.join(" ")}`);
  }
  // The native packages its restart controls exercise no longer select it.
  for (const path of [
    "crates/meerkat/src/governed_realm/stock/mod.rs",
    "crates/meerkat-core/src/mcp_attachment.rs",
    "crates/meerkat-store/src/lib.rs",
    "crates/meerkat-runtime/src/lib.rs",
  ]) {
    const plan = planFor([path], ["--event", "pull_request"]);
    assert.ok(!plan.unit_shards.some((shard) => shard.packages.includes("meerkat-rpc")), `${path}: no rpc unit lane`);
    assertLanes(plan, `${path} leaves rpc to main`);
  }
  // GitHub output: an rpc-only change has no unit matrix row and names the
  // deferral.
  const github = run(["--format", "github", "--event", "pull_request", "--", "crates/meerkat-rpc/src/lib.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.unit_shard_count, "0");
  assert.equal(lines.unit_deferred, "meerkat-rpc");
  assert.equal(lines.unit_deferred_count, "1");
}

// Docs-only: no Cargo lanes, and the plan says so explicitly rather than
// erroring or emitting a placeholder shard.
{
  const plan = planFor(["docs/guides/skills.mdx", "CHANGELOG.md", "README.md"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.mode, "none");
  assert.deepEqual(plan.shards, []);
  assert.deepEqual(plan.packages, []);
  assert.equal(plan.docs_only, true);
  assert.equal(plan.generated_contract, false);
}

// Documentation inside a crate directory is not a build input either.
{
  const plan = planFor(["crates/meerkat-core/README.md"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.mode, "none");
}

// Cargo.lock-only: the lock changes what every crate compiles against, so
// the plan is the whole workspace, packed into bounded shards that cover
// every member.
{
  const plan = planFor(["Cargo.lock"], ["--workspace-shards", "8"]);
  assert.equal(plan.mode, "workspace");
  assertLanes(plan, "Cargo.lock");
  assert.ok(plan.packages.length >= 45, `workspace mode lists every member (${plan.packages.length})`);
  assert.deepEqual(plan.closure, plan.packages);
  // The heads of the long check chains each get a fixed Clippy shard of
  // their own (CLIPPY_HEAD_SHARDS), so the pole is one head and their
  // dependencies' feature sets repeat between runs; the rest pack into five
  // more shards.
  const heads = [
    ["rkat"],
    ["meerkat-rpc"],
    ["meerkat-mob"],
    ["meerkat"],
    ["meerkat-mob-adaptive", "meerkat-mob-mcp", "meerkat-mob-pack"],
    ["meerkat-mcp-server", "meerkat-rest", "meerkat-web-runtime"],
    ["meerkat-integration-tests", "surface-build-fixtures"],
  ];
  for (const group of heads) {
    assert.ok(
      plan.shards.some((shard) => JSON.stringify(shard.packages) === JSON.stringify(group)),
      `the workspace plan lints ${group.join(" ")} in a shard of its own`,
    );
  }
  const coreShards = plan.shards.filter((shard) => shard.product === CORE);
  assert.equal(coreShards.length, heads.length + 5, `head shards plus five packed (${coreShards.length})`);
  // Each other product lints all of its packages in one shard of its own.
  for (const product of PRODUCTS.filter((candidate) => candidate.name !== CORE)) {
    const shards = plan.shards.filter((shard) => shard.product === product.name);
    const members = Object.keys(plan.package_model).filter((name) => plan.package_model[name].product === product.name);
    if (members.length === 0) continue;
    assert.equal(shards.length, 1, `${product.name} lints in one shard (${shards.length})`);
    assert.deepEqual(shards[0].packages, members.sort(), `${product.name}'s shard holds every ${product.name} package`);
  }
  // Feature checks keep the weight-packed shards (--workspace-shards).
  assert.ok(new Set(plan.feature_check_jobs.map((job) => job.name.replace(/ (bazel set|matrix)$/, ""))).size <= 8);
  // Cost-balanced, not count-balanced: no shard carries more than a quarter
  // of the estimated workspace cost, so the heaviest crates spread out.
  const total = plan.shards.reduce((sum, shard) => sum + shard.estimated_cost, 0);
  for (const shard of plan.shards) {
    assert.ok(shard.estimated_cost <= total / 4, `${shard.name} is not overloaded`);
  }
  const covered = plan.shards.flatMap((shard) => shard.packages).sort();
  assert.deepEqual(covered, plan.packages, "shards partition the workspace exactly once");
  assert.equal(plan.wasm, true);
}

// Root manifest, Cargo config, nextest config, the toolchain pin, the build
// wrapper, and the workflow itself are all global.
for (const path of [
  "Cargo.toml",
  ".cargo/config.toml",
  ".config/nextest.toml",
  "rust-toolchain.toml",
  "scripts/repo-cargo",
  "scripts/cargo-agent-gate",
  "scripts/ci-cargo-lanes.mjs",
  "scripts/ci-hook-coverage.mjs",
  "scripts/ci-verify-row-binaries.mjs",
  "scripts/ci-non-rust-readers.mjs",
  // They decide when a release projection reuses its parent's evidence.
  "scripts/ci-release-evidence.mjs",
  "scripts/ci-release-projection-selftest.mjs",
  // They decide what a push to main diffs against.
  "scripts/ci-push-base.mjs",
  "scripts/ci-push-plan-base.mjs",
  "scripts/release-projection-only.mjs",
  "scripts/release-version-pins.mjs",
  "scripts/ci-clippy-sccache-wrapper",
  ".github/workflows/ci.yml",
  ".github/actions/setup-rust-ci/action.yml",
]) {
  const plan = planFor([path]);
  assert.equal(plan.mode, "workspace", `${path} escalates to the workspace`);
  assertLanes(plan, path);
}

// Pull request versus push (--event): one change, two plans. The pull
// request runs a subset of the push's lanes, names every lane it leaves to
// the push, and is checked against the push's closure rows.
{
  const names = (rows) => rows.map((row) => row.name).sort();
  // Samples beside the source edits: a Cargo.lock-only change, scoped to the
  // members that resolve a changed entry (a checksum change of cpu-time,
  // which meerkat-mob's own lock entry lists, so the scope holds the mob
  // chain); a non-Rust fixture of meerkat-integration-tests, which selects
  // the mob-chain suite gpt-live-replay by its package and its path; and
  // that fixture directory's README, which no package owns, so the path
  // alone selects the suite.
  const realLock = readFileSync(resolve(root, "Cargo.lock"), "utf8");
  const mobEntry = /\[\[package\]\]\nname = "meerkat-mob"\n[\s\S]*?(?=\n\n)/.exec(realLock);
  assert.ok(mobEntry?.[0].includes('\n "cpu-time",\n'), "meerkat-mob's lock entry lists cpu-time");
  const cpuTime = /\[\[package\]\]\nname = "cpu-time"\nversion = "[^"]+"\nsource = "[^"]+"\nchecksum = "([0-9a-f]+)"/.exec(realLock);
  assert.ok(cpuTime, "the lock holds cpu-time");
  const pairDir = mkdtempSync(join(tmpdir(), "ci-lanes-pair-"));
  const pairBaseLock = join(pairDir, "Cargo.lock.base");
  writeFileSync(pairBaseLock, realLock.replace(`checksum = "${cpuTime[1]}"`, `checksum = "${"0".repeat(64)}"`));
  const replayFixture = "tests/integration/fixtures/gpt_live_replay/s104.provider-stream.jsonl";
  const replayReadme = "tests/integration/fixtures/gpt_live_replay/README.md";
  for (const sample of [replayFixture, replayReadme]) assert.ok(existsSync(resolve(root, sample)), `${sample} exists`);
  try {
  for (const [paths, extra] of [
    [["Cargo.toml"], []],
    [["crates/meerkat-core/src/lib.rs"], []],
    [["crates/meerkat-mob/src/lib.rs"], []],
    [["crates/meerkat/src/experimental_gpt_live.rs"], []],
    [["crates/meerkat-cli/src/main.rs", "crates/meerkat-session/src/lib.rs"], []],
    [["docs/index.mdx"], []],
    [["Cargo.lock"], ["--base-lock", pairBaseLock]],
    [[replayFixture], []],
    [[replayReadme], []],
  ]) {
    const label = paths.join(" ");
    const push = planFor(paths, [...extra, "--event", "push"]);
    const pr = planFor(paths, [...extra, "--event", "pull_request"]);
    assert.equal(push.event, "push", label);
    assert.equal(pr.event, "pull_request", label);
    assert.deepEqual(planFor(paths, [...extra, "--event", "workflow_dispatch"]).event, "push", `${label}: a dispatch plans the push lanes`);
    for (const key of ["mode", "packages", "closure", "test_scope", "unit_deferred", "hook_coverage", "machine_authority", "generated_contract", "bazel_graph"]) {
      assert.deepEqual(pr[key], push[key], `${label}: ${key} does not depend on the event`);
    }
    // The unit shards do not either; a push runs them on main.
    assert.deepEqual(unitOf(pr).shards, unitOf(push).shards, `${label}: unit shards do not depend on the event`);
    const deferred = pr.deferred_to_push;
    assert.deepEqual(
      [...pr.shards.flatMap((shard) => shard.packages), ...deferred.clippy_packages].sort(),
      push.shards.flatMap((shard) => shard.packages).sort(),
      `${label}: a pull request lints every changed package, or leaves its Clippy to the push`,
    );
    assert.equal(deferred.closure_test_rows, push.closure_test_rows.length, `${label}: every closure-tests row is the push's`);
    assert.equal(deferred.feature_check_jobs, push.feature_check_jobs.length, `${label}: every feature check is the push's`);
    assert.deepEqual([...names(pr.integration_suites), ...deferred.integration_suites].sort(), names(push.integration_suites), `${label}: integration suites`);
    assert.deepEqual([...names(pr.unit_feature_shards), ...deferred.unit_feature_suites].sort(), names(unitOf(push).suites).sort(), `${label}: feature suites`);
    if (push.mode !== "workspace") assert.ok(!pr.wasm || push.wasm, `${label}: the pull request selects no wasm lane the push does not`);
    assertEventSplit(pr, `${label} (pull request)`);
    assertEventSplit(push, `${label} (push)`);
    if (pr.rust_changed) {
      assertLanes(pr, `${label} (pull request)`);
      assertLanes(push, `${label} (push)`);
    }
  }
  // What the samples exercise: a scoped lock change that reaches the mob
  // chain leaves its suites to the push, and so do the fixture and the
  // path alone.
  const lockPr = planFor(["Cargo.lock"], ["--base-lock", pairBaseLock, "--event", "pull_request"]);
  assert.equal(lockPr.mode, "packages", "the lock change is scoped, not the workspace");
  assert.deepEqual(lockPr.lock_scope.changed.map((entry) => entry.split(" ")[0]), ["cpu-time"]);
  assert.ok(lockPr.packages.includes("meerkat-mob"), "the scope reaches meerkat-mob");
  assert.ok(lockPr.deferred_to_push.integration_suites.length > 0, "the pull request leaves mob-chain suites to the push");
  const replayPr = planFor([replayFixture], ["--event", "pull_request"]);
  const replayPush = planFor([replayFixture], ["--event", "push"]);
  assert.deepEqual(replayPr.packages, ["meerkat-integration-tests"], "the fixture belongs to a mob-chain package");
  assert.deepEqual(replayPr.integration_suites, [], "the pull request runs no suite for it");
  assert.deepEqual(replayPr.deferred_to_push.integration_suites, ["gpt-live-replay"], "and leaves its suite to the push");
  assert.deepEqual(names(replayPush.integration_suites), ["gpt-live-replay"], "which runs it");
  const readmePr = planFor([replayReadme], ["--event", "pull_request"]);
  assert.equal(readmePr.rust_changed, false, "no package owns the README");
  assert.deepEqual(readmePr.deferred_to_push.integration_suites, ["gpt-live-replay"], "its path alone leaves the suite to the push");
  assert.deepEqual(names(planFor([replayReadme], ["--event", "push"]).integration_suites), ["gpt-live-replay"], "which runs it");
  } finally {
    rmSync(pairDir, { recursive: true, force: true });
  }
  // The whole workspace: the mob-chain packages lint their lib and bin
  // targets, except the chain heads in MOB_CHAIN_CLIPPY_HEADS, which lint
  // on the push only; the facade and the packed shards lint every target;
  // the suites and lanes that compile the chain, and the facade's GPT Live
  // suites, wait for the push.
  const pr = planFor(["Cargo.toml"], ["--event", "pull_request"]);
  const shardOf = (name) => pr.shards.find((shard) => shard.packages.includes(name));
  assert.equal(shardOf("meerkat-mob").targets, "--lib");
  for (const name of MOB_CHAIN_CLIPPY_HEADS) {
    assert.equal(shardOf(name), undefined, `${name} lints on the push in a workspace-mode pull request`);
  }
  assert.deepEqual(
    pr.deferred_to_push.clippy_packages,
    [
      "meerkat-integration-tests",
      "meerkat-rpc",
      "rkat",
      ...Object.keys(pr.package_model).filter((name) => pr.package_model[name].product !== CORE && pr.package_model[name].heavy_chain),
    ].sort(),
  );
  assert.equal(shardOf("meerkat").targets, "--all-targets", "the facade is not in the chain");
  assert.equal(shardOf("surface-build-fixtures").targets, "--all-targets", "a mixed head group splits");
  assert.ok(!shardOf("surface-build-fixtures").packages.includes("meerkat-integration-tests"));
  assert.deepEqual(pr.deferred_to_push.integration_suites.sort(), ["gpt-live-replay", "machine-codegen", "rpc-configured-turn-limit"]);
  assert.deepEqual(pr.deferred_to_push.unit_feature_suites.sort(), ["meerkat[experimental-gpt-live]", "meerkat[openai-live]"]);
  assert.equal(pr.wasm, true, "a workspace pull request runs wasm-check: the web runtime is one of its packages");
  assert.equal(pr.example_web, false, "and the example web suites only for their own inputs");
  const github = run(["--format", "github", "--event", "pull_request", "--", "Cargo.toml"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.event, "pull_request");
  for (const key of ["closure_check", "rustdoc", "governed_jsonl", "mcp_confinement_positive"]) assert.equal(lines[key], "false", key);
  assert.equal(lines.guard_lane_reasons, "{}", "no guard lane ran, so no reason");
  assert.equal(lines.closure_test_count, "0");
  assert.equal(lines.feature_check_count, "0");
  // Core's 17 hook-parity rows plus each product's own rows (a row never
  // mixes products).
  const pushRows = planFor(["Cargo.toml"], ["--event", "push"]).closure_test_rows;
  assert.equal(pushRows.filter((row) => row.product === CORE).length, 17, "Core's closure-tests rows");
  assert.match(lines.deferred_to_push, new RegExp(`^${pushRows.length} closure-tests rows; the closure check; \\d+ feature-combination jobs; `));
  assert.match(lines.deferred_to_push, /; the workspace rustdoc$/);
  assert.match(
    lines.deferred_to_push,
    /; the Clippy of meerkat-connect, meerkat-integration-tests, meerkat-meta, meerkat-rpc, meta-store-conformance, rkat, toolkit-mobkit \(mob chain invalidated\); /,
  );
  // A packages-mode pull request lints a chain head itself (lib and bin
  // targets) while meerkat-mob's build stays cached: a change to the heads,
  // or to crates above meerkat-mob, leaves it alone.
  for (const [paths, lints] of [
    [["crates/meerkat-rpc/src/lib.rs"], { "meerkat-rpc": "--lib --bins" }],
    [["crates/meerkat-cli/src/main.rs"], { rkat: "--bins" }],
    [["tests/integration/src/lib.rs"], { "meerkat-integration-tests": "--lib --bins" }],
    [
      ["tests/integration/src/lib.rs", "crates/meerkat-rpc/src/lib.rs", "crates/meerkat-cli/src/main.rs"],
      { "meerkat-integration-tests": "--lib --bins", "meerkat-rpc": "--lib --bins", rkat: "--bins" },
    ],
    [["crates/meerkat-mob-mcp/src/lib.rs", "crates/meerkat-rpc/src/lib.rs"], { "meerkat-rpc": "--lib --bins" }],
  ]) {
    const label = paths.join(" ");
    const head = planFor(paths, ["--event", "pull_request"]);
    assert.equal(head.mode, "packages", `${label}: packages mode`);
    assert.deepEqual(head.deferred_to_push.clippy_packages, [], `${label}: no Clippy left to the push`);
    for (const [name, targets] of Object.entries(lints)) {
      assert.equal(head.shards.find((shard) => shard.packages.includes(name))?.targets, targets, `${label}: ${name} lints ${targets}`);
    }
  }
  // A change to meerkat-mob or to a crate its library compiles invalidates
  // its build, and the heads' lanes would compile the whole chain uncached:
  // the pull request leaves their whole Clippy to the push and names the
  // reason. The push lints every target of them, in every case.
  for (const [paths, deferredHeads] of [
    [["crates/meerkat-core/src/lib.rs", "crates/meerkat-rpc/src/lib.rs"], ["meerkat-rpc"]],
    [["crates/meerkat-mob/src/lib.rs", "crates/meerkat-cli/src/main.rs"], ["rkat"]],
    [
      ["crates/meerkat-runtime/src/lib.rs", "tests/integration/src/lib.rs", "crates/meerkat-rpc/src/lib.rs", "crates/meerkat-cli/src/main.rs"],
      ["meerkat-integration-tests", "meerkat-rpc", "rkat"],
    ],
  ]) {
    const label = paths.join(" ");
    const head = planFor(paths, ["--event", "pull_request"]);
    assert.equal(head.mode, "packages", `${label}: packages mode`);
    assert.deepEqual(head.deferred_to_push.clippy_packages, deferredHeads, `${label}: the heads' Clippy waits for the push`);
    assert.equal(head.deferred_to_push.clippy_packages_reason, MOB_CHAIN_INVALIDATED, `${label}: and names the reason`);
    for (const name of deferredHeads) {
      assert.equal(head.shards.find((shard) => shard.packages.includes(name)), undefined, `${label}: no ${name} shard in the pull request`);
    }
    const summary = Object.fromEntries(
      run(["--format", "github", "--event", "pull_request", "--", ...paths]).stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
    );
    assert.ok(
      summary.deferred_to_push.includes(`the Clippy of ${deferredHeads.join(", ")} (${MOB_CHAIN_INVALIDATED})`),
      `${label}: the pull request's summary lists them: ${summary.deferred_to_push}`,
    );
    const push = planFor(paths, ["--event", "push"]);
    for (const name of deferredHeads) {
      assert.equal(push.shards.find((shard) => shard.packages.includes(name))?.targets, "--all-targets", `${label}: the push lints every target of ${name}`);
    }
  }
  for (const row of JSON.parse(lines.shard_matrix).include) {
    assert.ok(["--all-targets", "--lib", "--bins", "--lib --bins"].includes(row.targets), `${row.name} carries its targets`);
  }
  const pushLines = Object.fromEntries(
    run(["--format", "github", "--", "Cargo.toml"]).stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
  assert.equal(pushLines.event, "push");
  assert.equal(pushLines.deferred_to_push, "");
  for (const key of ["closure_check", "rustdoc", "governed_jsonl", "mcp_confinement_positive"]) assert.equal(pushLines[key], "true", key);
  assert.deepEqual(
    JSON.parse(pushLines.guard_lane_reasons),
    { governed_jsonl: ["every_rust_change"], mcp_confinement_positive: ["every_rust_change"] },
    "the push's reasons, as the gate reads them",
  );
  // A change to the web runtime's own inputs still runs its lanes in the
  // pull request.
  for (const path of ["crates/meerkat-web-runtime/src/lib.rs", "sdks/web/src/runtime.ts", "crates/meerkat-contracts/src/lib.rs"]) {
    const web = planFor([path], ["--event", "pull_request"]);
    assert.equal(web.example_web, true, `${path} runs the example web suites in the pull request`);
    if (!path.startsWith("sdks/")) assert.equal(web.wasm, true, `${path} runs wasm-check in the pull request`);
  }
  // A pull request builds the web wasm only on a cached closure: a change to
  // a crate the web runtime builds on, or a whole-workspace plan, compiles
  // that closure cold (1181 s on pull request 39209102985), so the plan
  // leaves the wasm build, the browser suites and the WASM timer lane to the
  // push and keeps the Web SDK's type and unit tests. A change to the
  // runtime itself, the SDK or the examples builds the wasm in the pull
  // request; a push builds it whenever the lane runs.
  for (const [paths, wasm] of [
    [["crates/meerkat-contracts/src/lib.rs"], false],
    [["crates/meerkat-core/src/lib.rs", "sdks/web/src/runtime.ts"], false],
    [[".github/workflows/ci.yml"], false],
    [["crates/meerkat-web-runtime/src/lib.rs"], true],
    [["sdks/web/src/runtime.ts"], true],
    [["examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs"], true],
  ]) {
    const plan = planFor(paths, ["--event", "pull_request"]);
    const label = paths.join(" + ");
    assert.equal(plan.example_web, true, `${label} runs the example web lane in the pull request`);
    assert.equal(plan.example_web_wasm, wasm, `${label} ${wasm ? "builds" : "leaves"} the web wasm in the pull request`);
    assert.equal(plan.deferred_to_push.example_web_wasm, !wasm, `${label}: deferred_to_push ${wasm ? "keeps" : "names"} the web wasm`);
    const summary = run(["--format", "github", "--event", "pull_request", "--", ...paths]).stdout;
    assert.equal(
      /^deferred_to_push=.*the web wasm build and browser suites/m.test(summary),
      !wasm,
      `${label}: the gate's deferral notice ${wasm ? "omits" : "names"} the web wasm`,
    );
    assert.match(summary, new RegExp(`^example_web_wasm=${wasm}$`, "m"), `${label}: GitHub output carries example_web_wasm`);
  }
  for (const path of ["crates/meerkat-core/src/lib.rs", "crates/meerkat-rpc/src/lib.rs", "sdks/web/src/runtime.ts"]) {
    assert.equal(planFor([path]).example_web_wasm, true, `a push that changes ${path} builds the web wasm when the lane runs`);
  }
  // wasm-check runs for every crate the wasm build compiles (the web
  // runtime's dependency closure in the cargo graph), the provider crates
  // included (#1108), and for no crate outside it.
  for (const [path, wasm] of [
    ["crates/meerkat-openai/src/lib.rs", true],
    ["crates/meerkat-anthropic/src/lib.rs", true],
    ["crates/meerkat-models/src/lib.rs", true],
    ["crates/meerkat-rest/src/lib.rs", false],
    ["crates/meerkat-rpc/src/lib.rs", false],
    ["crates/meerkat-cli/src/main.rs", false],
  ]) {
    const plan = planFor([path], ["--event", "pull_request"]);
    assert.equal(plan.wasm, wasm, `a pull request that changes ${path} ${wasm ? "runs" : "leaves"} wasm-check`);
    assert.equal(plan.example_web, false, `${path}: the example web suites keep to their own inputs`);
  }
  assert.notEqual(run(["--event", "merge_group", "--", "Cargo.toml"]).status, 0, "an unknown event fails");
}

// Cargo.lock scoping: a lock change that is the only global path plans the
// members whose resolved dependencies it changed, from the head lock's own
// dependency lists; anything it cannot read plans the whole workspace.
{
  const lock = (entries, header = "version = 4") =>
    `# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\n${header}\n\n` +
    entries.map(([name, version, source, dependencies, checksum]) => [
      "[[package]]",
      `name = "${name}"`,
      `version = "${version}"`,
      ...(source ? [`source = "${source}"`] : []),
      ...(checksum ? [`checksum = "${checksum}"`] : []),
      ...(dependencies.length ? ["dependencies = [", ...dependencies.map((dep) => ` "${dep}",`), "]"] : []),
      "",
    ].join("\n")).join("\n");
  const registry = "registry+https://github.com/rust-lang/crates.io-index";
  const members = new Set(["app", "lib-a", "lib-b"]);
  const base = lock([
    ["app", "0.1.0", null, ["lib-a", "lib-b"]],
    ["lib-a", "0.1.0", null, ["leaf 1.0.0", "shared"]],
    ["lib-b", "0.1.0", null, ["shared"]],
    ["leaf", "1.0.0", registry, [], "aa"],
    ["leaf", "2.0.0", registry, [], "bb"],
    ["shared", "1.0.0", registry, ["leaf 2.0.0"], "cc"],
  ]);
  // A version bump of one entry reaches its dependents only.
  const bumped = base.replace('name = "leaf"\nversion = "1.0.0"\nsource = "' + registry + '"\nchecksum = "aa"', 'name = "leaf"\nversion = "1.0.1"\nsource = "' + registry + '"\nchecksum = "ab"')
    .replace('"leaf 1.0.0",', '"leaf 1.0.1",');
  assert.notEqual(bumped, base);
  const bump = lockScope(base, bumped, members);
  assert.deepEqual(bump.members, ["app", "lib-a"], "lib-b does not resolve the bumped leaf");
  assert.ok(bump.removed.includes(`leaf 1.0.0 (${registry})`));
  // A checksum change of a shared entry reaches everything above it.
  const shared = lockScope(base, base.replace('checksum = "cc"', 'checksum = "cd"'), members);
  assert.deepEqual(shared.members, ["app", "lib-a", "lib-b"]);
  // A transitive change walks up through the external graph.
  assert.deepEqual(lockScope(base, base.replace('checksum = "bb"', 'checksum = "bc"'), members).members, ["app", "lib-a", "lib-b"]);
  for (const [label, head, pattern] of [
    ["a header change", lock([], "version = 5"), /header/],
    ["an unchanged entry set", base.replace("\n\n[[package]]", "\n\n\n[[package]]"), /no package entry/],
    ["an unresolvable dependency", base.replace('"shared",\n]', '"missing",\n]'), /names 0 entries/],
    ["an unreadable line", base.replace('checksum = "cc"', "checksum = cc"), /unreadable lock line/],
    ["another table", `${base}\n[[patch.unused]]\nname = "x"\nversion = "1.0.0"\n`, /other than \[\[package\]\]/],
  ]) {
    assert.throws(() => lockScope(base, head, members), pattern, `${label} is refused`);
  }
  assert.equal(parseCargoLock(readFileSync(resolve(root, "Cargo.lock"), "utf8")).entries.size > 100, true, "the real lock parses");
  // End to end: a base lock whose machine-dsl-tests-only entry differs
  // plans that one member, not the workspace.
  const real = readFileSync(resolve(root, "Cargo.lock"), "utf8");
  const trybuild = /\[\[package\]\]\nname = "trybuild"\nversion = "[^"]+"\nsource = "[^"]+"\nchecksum = "([0-9a-f]+)"/.exec(real);
  assert.ok(trybuild, "the lock holds trybuild");
  const dir = mkdtempSync(join(tmpdir(), "ci-lanes-lock-"));
  try {
    const baseLock = join(dir, "Cargo.lock.base");
    writeFileSync(baseLock, real.replace(`checksum = "${trybuild[1]}"`, `checksum = "${"0".repeat(64)}"`));
    for (const event of ["push", "pull_request"]) {
      const plan = planFor(["Cargo.lock"], ["--event", event, "--base-lock", baseLock]);
      assert.equal(plan.mode, "packages", `${event}: a scoped lock change is not the workspace`);
      assert.deepEqual(plan.packages, ["machine-dsl-tests"], `${event}: only trybuild's dependent`);
      assert.deepEqual(plan.lock_scope.members, ["machine-dsl-tests"]);
      assert.match(plan.reason, /resolve a changed Cargo\.lock entry/);
      assert.equal(plan.bazel_graph, true, "the lock still runs the Bazel graph check");
      assertLanes(plan, `scoped lock (${event})`);
    }
    // Another global path beside the lock, or no base lock, plans the
    // workspace.
    assert.equal(planFor(["Cargo.lock", "Cargo.toml"], ["--base-lock", baseLock]).mode, "workspace");
    const unscoped = planFor(["Cargo.lock"]);
    assert.equal(unscoped.mode, "workspace");
    assert.match(unscoped.reason, /not scoped: no base lock/);
    writeFileSync(baseLock, real.replace("version = 4", "version = 3"));
    const header = planFor(["Cargo.lock"], ["--base-lock", baseLock]);
    assert.equal(header.mode, "workspace", "a lock format change plans the workspace");
    assert.match(header.lock_scope.error, /header/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// A path list is never a release projection: only a --base/--head diff with
// verified evidence is (scripts/ci-release-projection-selftest.mjs).
{
  const plan = planFor(["Cargo.toml", "Cargo.lock", "CHANGELOG.md", "README.md"]);
  assert.equal(plan.release_projection, null, "a release-shaped path list keeps the ordinary plan");
  assert.equal(plan.mode, "workspace");
  assertLanes(plan, "release-shaped path list");
}

// Machine-authority-only: TLA+ models and machine specs are not Cargo
// inputs, but the governance flag must fire so machine-check-drift runs.
{
  const plan = planFor(["specs/machines/meerkat_machine/model.tla", "specs/compositions/meerkat_mob_seam/ci.cfg"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.mode, "none");
  assert.equal(plan.machine_authority, true);
  assert.deepEqual(plan.shards, []);
}

// Poster-only: the poster gate runs, but the change is not machine
// authority, so it cannot start the machine-authority lanes (TLC).
{
  const plan = planFor(["docs/internal/machine-posters/mob_machine.html", "scripts/machine-posters/generate-machine-posters.mjs"]);
  assert.equal(plan.rust_changed, false);
  assert.equal(plan.machine_authority, false);
  assert.equal(plan.machine_posters, true);
}

// The typed governance gates run on the agent gate's governance paths: the
// machine-authority paths and the dogma documents (scripts/cargo-agent-gate,
// is_governance_path), which ran them in the pre-push hook's ratchets.
{
  assert.equal(planFor(["specs/machines/meerkat_machine/model.tla"]).governance_gates, true, "machine authority runs the governance gates");
  for (const path of [
    ".claude/skills/meerkat-dogma-inquisition/SKILL.md",
    "docs/architecture/meerkat-dogma.md",
    "docs/architecture/meerkat-dogma-commentary.md",
    "docs/internal/dogma-audits/PR759-final-ledger.md",
  ]) {
    const plan = planFor([path]);
    assert.equal(plan.governance_gates, true, `${path} runs the governance gates`);
    assert.equal(plan.machine_authority, false, `${path} is not machine authority`);
  }
  for (const path of ["docs/architecture/meerkat-dogma-notes.md", ".claude/skills/meerkat-architecture/SKILL.md", "docs/internal/machine-posters/mob_machine.html"]) {
    assert.equal(planFor([path]).governance_gates, false, `${path} does not run the governance gates`);
  }
  const gate = readFileSync(resolve(root, "scripts/cargo-agent-gate"), "utf8");
  const start = gate.indexOf("is_governance_path() {");
  assert.ok(start >= 0, "scripts/cargo-agent-gate defines is_governance_path");
  const body = gate.slice(start, gate.indexOf("\n}\n", start));
  assert.ok(body.includes("scripts/machine-authority-changed"), "the agent gate's governance paths start from machine authority");
  const patterns = body
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => /^[^#(]\S*\)$/.test(line))
    .flatMap((line) => line.slice(0, -1).split("|"));
  assert.deepEqual(patterns, GOVERNANCE_DOC_PATTERNS, "the classifier's governance documents are the agent gate's");
}

// A generated machine kernel is both machine authority and a Rust source
// owned by meerkat-machine-kernels.
{
  const plan = planFor(["crates/meerkat-machine-kernels/src/generated/meerkat.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat-machine-kernels"]);
  assert.equal(plan.machine_authority, true);
  assertLanes(plan, "generated kernel");
}

// A Rust source outside every workspace package cannot be attributed, so it
// fails closed into the workspace instead of into nothing.
{
  const plan = planFor(["examples/001-hello-meerkat-rs/main.rs"]);
  assert.equal(plan.mode, "workspace");
  assert.deepEqual(plan.unmapped_rust_paths, ["examples/001-hello-meerkat-rs/main.rs"]);
  assertLanes(plan, "unmapped .rs");
}

// A crate manifest selects its crate (and the closure through dependents).
{
  const plan = planFor(["crates/meerkat-openai/Cargo.toml", "crates/meerkat/src/help.rs"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat", "meerkat-openai"]);
  assertLanes(plan, "manifest + facade");
  assert.ok(plan.closure.includes("meerkat-mob"));
  assert.equal(plan.shards.length, 2, "one shard per changed package below the shard cap");
}

// Many changed packages are packed into the shard cap, never dropped.
{
  const paths = [
    "crates/meerkat-core/src/lib.rs",
    "crates/meerkat-runtime/src/lib.rs",
    "crates/meerkat-mob/src/lib.rs",
    "crates/meerkat-rpc/src/lib.rs",
    "crates/meerkat-rest/src/lib.rs",
    "crates/meerkat-cli/src/main.rs",
    "crates/meerkat-session/src/lib.rs",
    "crates/meerkat-store/src/lib.rs",
    "crates/xtask/src/main.rs",
  ];
  const plan = planFor(paths, ["--max-shards", "4"]);
  assert.equal(plan.mode, "packages");
  assert.equal(plan.packages.length, 9);
  assert.equal(plan.shards.length, 4);
  assertLanes(plan, "nine packages in four shards");
}

// A pull request that changes crates/xtask runs xtask's unit lane and its
// machine-authority feature suite (main went red through #1362 because both
// were deferred).
{
  const plan = planFor(["crates/xtask/src/machines.rs"]);
  assert.deepEqual(plan.unit_deferred, [], "xtask is not deferred to main");
  assert.deepEqual(unitOf(plan).shards.map((shard) => shard.packages), [["xtask"]]);
  assert.ok(
    unitOf(plan).suites.some((suite) => suite.packages[0] === "xtask" && suite.features.includes("machine-authority")),
    "the xtask machine-authority suite runs in the pull request",
  );
  for (const shard of [...unitOf(plan).shards, ...unitOf(plan).suites]) {
    assert.ok(shard.estimated_minutes <= plan.pr_unit_budget_minutes, `${shard.name} fits the pull-request budget`);
  }
  // Its tests run 5-7 minutes on a hosted runner, so it never shares its
  // lane: another changed package packs apart from it.
  const mixed = planFor(["crates/xtask/src/machines.rs", "crates/meerkat-core/src/lib.rs", "crates/meerkat-tools/src/lib.rs"]);
  assert.deepEqual(
    unitOf(mixed).shards.filter((shard) => shard.packages.includes("xtask")).map((shard) => shard.packages),
    [["xtask"]],
    "xtask's unit lane holds xtask alone",
  );
  assert.deepEqual(
    unitOf(mixed).shards.flatMap((shard) => shard.packages).sort(),
    ["meerkat-core", "meerkat-tools", "xtask"],
    "and every changed package still has a unit lane",
  );
}

// Embedded inputs the facade compiles in through include macros select the
// facade even though they are Markdown outside its directory.
{
  const plan = planFor([".claude/skills/meerkat-platform/SKILL.md"]);
  assert.equal(plan.mode, "packages");
  assert.deepEqual(plan.packages, ["meerkat"]);
  assertLanes(plan, "embedded skill doc");
}

// A crate can link a file of another crate into its own directory, such as a
// test fixture both crates' tests read. Git records the link, not the file
// it names, so the planner resolves the link: a change to that file selects
// the linking crate as well as the file's own crate, and a pull request runs
// the linking crate's unit lane. The row adds such a link (staged, not
// committed) in a scratch worktree of HEAD and plans there with this
// checkout's classifier. Its caches stay in the scratch directory
// (XDG_CACHE_HOME), and repo-cargo takes this checkout's Cargo home and lane
// name, so cargo metadata there downloads nothing and leaves no lane behind.
{
  const scratch = mkdtempSync(join(tmpdir(), "ci-lanes-cross-crate-link-"));
  // A unique name: concurrent runs share the repository's worktree list.
  const tree = join(scratch, basename(scratch));
  const git = (args, cwd) => {
    const result = spawnSync("git", args, { cwd, encoding: "utf8" });
    assert.equal(result.status, 0, `git ${args.join(" ")}: ${result.stderr}`);
  };
  const printed = spawnSync(join(root, "scripts/repo-cargo"), ["--print-env"], { cwd: root, encoding: "utf8" });
  assert.equal(printed.status, 0, `repo-cargo --print-env: ${printed.stderr}`);
  const cargo = Object.fromEntries(
    printed.stdout.split("\n").filter((line) => line.includes("=")).map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
  assert.ok(cargo.CARGO_HOME && cargo.lane_key, "repo-cargo names its Cargo home and lane");
  const env = { ...process.env, XDG_CACHE_HOME: join(scratch, "cache"), CARGO_HOME: cargo.CARGO_HOME, RUST_LANE_ID: cargo.lane_key };
  const target = "crates/meerkat-core/tests/fixtures/v0_8_8_manifest.json";
  const sibling = "crates/meerkat-core/tests/fixtures/v0_8_8_slim_session.json";
  const link = "crates/meerkat-runtime/tests/fixtures/cross_crate_link_selftest.json";
  let added = false;
  try {
    git(["worktree", "add", "--quiet", "--detach", tree, "HEAD"], root);
    added = true;
    for (const path of [target, sibling]) assert.ok(existsSync(join(tree, path)), `${path} exists at HEAD`);
    assert.ok(!existsSync(join(tree, link)), `${link} is free at HEAD`);
    mkdirSync(dirname(join(tree, link)), { recursive: true });
    symlinkSync(relative(dirname(join(tree, link)), join(tree, target)), join(tree, link));
    git(["add", "--", link], tree);
    const planIn = (path) => {
      const result = spawnSync("node", [classifier, "--format", "json", "--event", "pull_request", "--", path], {
        cwd: tree,
        encoding: "utf8",
        env,
        maxBuffer: 64 * 1024 * 1024,
      });
      assert.equal(result.status, 0, `classifier failed in the scratch worktree: ${result.stderr}`);
      return JSON.parse(result.stdout);
    };
    const linked = planIn(target);
    assert.deepEqual(linked.packages, ["meerkat-core", "meerkat-runtime"], "a linked fixture selects its own crate and the linking crate");
    assert.ok(linked.unit_packages.includes("meerkat-runtime"), "the linking crate's unit lane runs in the pull request");
    assertLanes(linked, "a fixture another crate links");
    assert.deepEqual(planIn(sibling).packages, ["meerkat-core"], "a fixture no link names selects its own crate alone");
    assert.deepEqual(planIn(link).packages, ["meerkat-runtime"], "the link itself selects the crate that holds it");
  } finally {
    if (added) spawnSync("git", ["worktree", "remove", "--force", tree], { cwd: root });
    rmSync(scratch, { recursive: true, force: true });
  }
}

// Generated-contract and SDK-host flags.
{
  const plan = planFor(["artifacts/schemas/version.json", "sdks/python/meerkat/client.py"]);
  assert.equal(plan.generated_contract, true);
  assert.equal(plan.sdk_host, true);
}

// No diff base at all: the plan is the whole workspace, never nothing.
{
  const result = run(["--format", "json"]);
  assert.equal(result.status, 0, result.stderr);
  const plan = JSON.parse(result.stdout);
  assert.equal(plan.mode, "workspace");
  assertLanes(plan, "no base");
}
{
  const result = run(["--format", "json", "--base", "0000000000000000000000000000000000000000"]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).mode, "workspace");
}
{
  const result = run(["--format", "json", "--base", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", "--head", "HEAD"]);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).mode, "workspace", "unknown base escalates to the workspace");
}

// GitHub output: every key the workflow reads, and a well-formed matrix.
{
  const result = run(["--format", "github", "--", "crates/meerkat-core/src/lib.rs"]);
  assert.equal(result.status, 0, result.stderr);
  const lines = Object.fromEntries(
    result.stdout.trim().split("\n").map((line) => {
      const index = line.indexOf("=");
      return [line.slice(0, index), line.slice(index + 1)];
    }),
  );
  for (const key of [
    "event",
    "rust_changed",
    "mode",
    "reason",
    "generated_contract",
    "machine_authority",
    "machine_posters",
    "governance_gates",
    "wasm",
    "sdk_host",
    "bazel_graph",
    "examples_browser",
    "example_web",
    "docs_only",
    "package_count",
    "closure_count",
    "closure_flags",
    "closure_check",
    "rustdoc",
    "governed_jsonl",
    "mcp_confinement_positive",
    "guard_lane_reasons",
    "deferred_to_push",
    "shard_count",
    "shard_matrix",
  ]) {
    assert.ok(key in lines, `github output has ${key}`);
  }
  assert.equal(lines.rust_changed, "true");
  assert.deepEqual(bothWaves(lines, "shard_matrix", "shard_rest_matrix"), [{ name: "core", packages: "-p meerkat-core", targets: "--all-targets" }]);
  assert.match(lines.closure_flags, /-p meerkat-core/);
  for (const key of ["unit_shard_count", "unit_shard_matrix", "unit_deferred", "unit_deferred_count", "unit_rows_on_main", "unit_rows_on_main_count", "main_unit_shard_count", "main_unit_shard_matrix"]) {
    assert.ok(key in lines, `github output has ${key}`);
  }
  // Core's unit row runs on main on a push (and in the unit lane of a pull
  // request, below).
  assert.equal(lines.unit_shard_count, "0");
  assert.equal(lines.unit_rows_on_main, "core");
  assert.ok(bothWaves(lines, "main_unit_shard_matrix", "main_unit_rest_shard_matrix").some((row) => row.packages === "-p meerkat-core"));
  const prLines = Object.fromEntries(
    run(["--format", "github", "--event", "pull_request", "--", "crates/meerkat-core/src/lib.rs"]).stdout.trim().split("\n")
      .map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
  assert.equal(prLines.unit_shard_count, "1");
  assert.equal(prLines.unit_rows_on_main_count, "0");
  assert.deepEqual(
    JSON.parse(prLines.unit_shard_matrix).include.map((row) => row.packages).sort(),
    ["-p meerkat-core"],
  );
  assert.equal(lines.unit_deferred, "");
  assert.ok(Number(lines.main_unit_shard_count) >= 2);
  const integrationRows = JSON.parse(lines.integration_matrix).include;
  assert.deepEqual(integrationRows.find((row) => row.name === "standalone-turn-lifecycle"), {
    name: "standalone-turn-lifecycle",
    packages: "-p meerkat",
    tests: "--test standalone_turn_lifecycle",
  });
  assert.deepEqual(integrationRows.find((row) => row.name === "rpc-configured-turn-limit"), {
    name: "rpc-configured-turn-limit",
    packages: "-p meerkat-rpc",
    tests: "--test configured_turn_limit",
  });
}
{
  const result = run(["--format", "github", "--", "crates/meerkat-mob/src/lib.rs"]);
  const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.unit_shard_count, "0");
  assert.equal(lines.unit_deferred, "meerkat-mob");
  assert.deepEqual(JSON.parse(lines.unit_shard_matrix).include, [{ name: "none", packages: "" }]);
  assert.equal(lines.shard_count, "1");
}
{
  const result = run(["--format", "github", "--", "docs/index.mdx"]);
  const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.deepEqual(JSON.parse(lines.shard_matrix).include, [{ name: "none", packages: "" }]);
  assert.equal(lines.rust_changed, "false");
  // A docs-only push to main has no unit lanes: the main-unit matrix is
  // empty (no "none" shard for the job to trip over) and its count is 0.
  assert.deepEqual(JSON.parse(lines.main_unit_shard_matrix), { include: [] });
  assert.equal(lines.main_unit_shard_count, "0");
  assert.equal(lines.unit_shard_count, "0");
  assert.equal(lines.unit_deferred, "");
}
{
  const plan = planFor(["CHANGELOG.md"]);
  assert.equal(plan.rust_changed, false);
  assert.deepEqual(plan.main_unit_shards, [], "a CHANGELOG-only merge yields no main unit lanes");
  assert.deepEqual(plan.main_archive_builds, [], "a CHANGELOG-only merge yields no archived lanes");
  assert.deepEqual(plan.main_archive_runs, []);
}

// A push runs the unit lane's rows on main, verbatim, and the two jobs run
// a row with the same steps.
{
  for (const [label, paths] of [
    ["workspace push", ["Cargo.toml"]],
    ["core push", ["crates/meerkat-core/src/lib.rs"]],
    ["facade push", ["crates/meerkat/src/experimental_gpt_live.rs"]],
    ["session push", ["crates/meerkat-session/src/lib.rs"]],
    ["xtask push", ["crates/xtask/src/main.rs"]],
  ]) {
    const push = assertUnitRowsOnMain(paths, label);
    assert.ok(push.unit_rows_on_main.length > 0, `${label}: the push runs unit rows on main`);
  }
  // A change to the mob chain alone has no unit rows: its packages are
  // deferred to main, as before.
  const chain = assertUnitRowsOnMain(["crates/meerkat-rpc/src/lib.rs"], "rpc push");
  assert.deepEqual(chain.unit_rows_on_main, [], "a mob-chain change has no unit rows");
  assert.deepEqual(chain.unit_deferred, ["meerkat-rpc"]);
  const github = run(["--format", "github", "--event", "push", "--", "crates/meerkat-core/src/lib.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.unit_shard_count, "0", "a push's unit lane runs no row");
  assert.equal(lines.unit_rows_on_main, "core");
  assert.equal(lines.unit_rows_on_main_count, "1");
  assert.ok(bothWaves(lines, "main_unit_shard_matrix", "main_unit_rest_shard_matrix").some((row) => row.name === "core" && row.packages === "-p meerkat-core"));
  assertUnitJobsMatch(readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8"), "ci.yml");
}

// Long rows first: each two-wave lane's rows split by their measured
// duration (ROW_DURATIONS, or --row-durations): rows at WAVE_FIRST_SECONDS
// or more, and rows never measured, start first; a main unit row may carry
// a unit row's measurement; every row is in exactly one wave; and the
// GitHub outputs split each lane's matrix and count the same way.
{
  const scratch = mkdtempSync(join(tmpdir(), "ci-lanes-waves-"));
  try {
    const table = (rows, extra = {}) => {
      const file = join(scratch, `table-${Object.keys(rows).length}-${Math.random().toString(36).slice(2)}.json`);
      writeFileSync(file, JSON.stringify({ schema_version: 2, rows, ...extra }));
      return file;
    };
    const wavesFor = (rows, paths = ["crates/meerkat-core/src/lib.rs"]) => planFor(paths, ["--event", "push", "--row-durations", table(rows)]).waves;
    const below = WAVE_FIRST_SECONDS - 1;
    let waves = wavesFor({ "clippy:core": below, "main-unit:core": WAVE_FIRST_SECONDS });
    assert.deepEqual(waves.clippy, { first: [], rest: ["core"] }, "a short Clippy row waits");
    assert.ok(waves.main_unit.first.includes("core"), "a row measured at the threshold starts first");
    waves = wavesFor({ "unit:core": below });
    assert.ok(waves.main_unit.rest.includes("core"), "a main unit row carries its unit row's measurement");
    assert.ok(!waves.main_unit.first.includes("core"));
    waves = wavesFor({ "main-unit:core": WAVE_FIRST_SECONDS, "unit:core": below });
    assert.ok(waves.main_unit.first.includes("core"), "its own measurement comes before the unit row's");
    // Rows never measured start first: an empty table plans every row first.
    const workspace = planFor(["Cargo.toml"], ["--event", "push", "--row-durations", table({})]);
    for (const [lane, rows] of [
      ["clippy", workspace.shards],
      ["main_unit", [...workspace.main_unit_shards, ...workspace.main_feature_unit_shards]],
      ["closure_tests", workspace.closure_test_rows],
      ["feature_check", workspace.feature_check_jobs],
    ]) {
      assert.deepEqual(workspace.waves[lane], { first: rows.map((row) => row.name), rest: [] }, `${lane}: an unmeasured row starts first`);
    }
    // The real table splits a whole-workspace push into both waves, each row in one.
    const real = planFor(["Cargo.toml"], ["--event", "push"]);
    for (const [lane, rows] of [
      ["clippy", real.shards],
      ["main_unit", [...real.main_unit_shards, ...real.main_feature_unit_shards]],
      ["closure_tests", real.closure_test_rows],
      ["feature_check", real.feature_check_jobs],
    ]) {
      const { first, rest } = real.waves[lane];
      assert.ok(first.length > 0 && rest.length > 0, `${lane}: the measured table splits a workspace push`);
      assert.deepEqual([...first, ...rest].sort(), rows.map((row) => row.name).sort(), `${lane}: every row is in exactly one wave`);
    }
    // A repack keeps a row's measurement. Packing is by Rust line count, so
    // a merge can change a packed row's member count ("core+12" becoming
    // "core+13"); the table measures the row under its anchor, and the plan's
    // packed rows land by that measurement, whatever count the tree gives
    // them.
    assert.deepEqual(
      ["core+12", "openai+8 bazel set", "rkat+5 matrix", "tests meerkat 1/5+3", "integration-tests+1", "mob 1/2", "mob[openai-live] 1/2", "meerkat", "e2e-fast"].map(rowKey),
      ["core", "openai bazel set", "rkat matrix", "tests meerkat 1/5", "integration-tests", "mob 1/2", "mob[openai-live] 1/2", "meerkat", "e2e-fast"],
      "a row's key drops only the member count after its anchor",
    );
    // Two rows of a lane never share a key (their bins have disjoint
    // members); a plan in which they did is an internal error, not two rows
    // sharing one measurement.
    assert.throws(
      () => waveOf("clippy", [{ name: "core+1" }, { name: "core+2" }], {}),
      /internal error: clippy rows "core\+1" and "core\+2" share the measurement key "core"/,
    );
    // A packed main unit row carries its unit row's measurement by key too.
    assert.deepEqual(
      waveOf("main-unit", [{ name: "core+3" }], { "unit:core": below }),
      { first: [], rest: ["core+3"] },
      "a packed main unit row falls back to its unit row's measurement under its key",
    );
    // Every row of the four lanes (and the unit lane) of a whole-workspace
    // push and pull request has a key that is its own key, and the reader
    // accepts a table of those keys: the generator's output is the reader's
    // form.
    const keyed = {};
    for (const event of ["push", "pull_request"]) {
      const plan = planFor(["Cargo.toml"], ["--event", event, "--row-durations", table({})]);
      for (const [lane, rows] of [
        ["clippy", plan.shards],
        ["unit", [...plan.unit_shards, ...plan.unit_feature_shards]],
        ["main-unit", [...plan.main_unit_shards, ...plan.main_feature_unit_shards]],
        ["closure-tests", plan.closure_test_rows],
        ["feature-check", plan.feature_check_jobs],
      ]) {
        for (const { name } of rows) {
          assert.equal(rowKey(rowKey(name)), rowKey(name), `${event}: ${lane} row ${name} has a fixed key`);
          keyed[`${lane}:${rowKey(name)}`] = 1;
        }
      }
    }
    assert.ok(Object.keys(keyed).length > 0);
    assert.deepEqual(readRowDurations(table(keyed)), keyed, "the reader accepts every row's key");
    const unmeasured = planFor(["Cargo.toml"], ["--event", "push", "--row-durations", table({})]);
    const anchors = {};
    const packed = {};
    for (const [lane, key, rows] of [
      ["clippy", "clippy", unmeasured.shards],
      ["main_unit", "main-unit", [...unmeasured.main_unit_shards, ...unmeasured.main_feature_unit_shards]],
      ["closure_tests", "closure-tests", unmeasured.closure_test_rows],
      ["feature_check", "feature-check", unmeasured.feature_check_jobs],
    ]) {
      packed[lane] = rows.map((row) => row.name).filter((name) => rowKey(name) !== name);
      assert.ok(packed[lane].length > 0, `${lane}: a workspace push packs rows`);
      for (const name of packed[lane]) anchors[`${key}:${rowKey(name)}`] = below;
    }
    const measured = planFor(["Cargo.toml"], ["--event", "push", "--row-durations", table(anchors)]).waves;
    for (const [lane, names] of Object.entries(packed)) {
      assert.deepEqual(names.filter((name) => !measured[lane].rest.includes(name)), [], `${lane}: a packed row is measured under its anchor`);
    }
    const github = run(["--format", "github", "--event", "push", "--", "Cargo.toml"]);
    assert.equal(github.status, 0, github.stderr);
    const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
    const names = (key) => JSON.parse(lines[key]).include.map((row) => row.name).filter((name) => name !== "none");
    for (const [lane, first, rest, firstCount, restCount, total] of [
      ["clippy", "shard_matrix", "shard_rest_matrix", "shard_first_count", "shard_rest_count", "shard_count"],
      ["main_unit", "main_unit_shard_matrix", "main_unit_rest_shard_matrix", "main_unit_first_count", "main_unit_rest_count", "main_unit_shard_count"],
      ["closure_tests", "closure_test_matrix", "closure_test_rest_matrix", "closure_test_first_count", "closure_test_rest_count", "closure_test_count"],
      ["feature_check", "feature_check_matrix", "feature_check_rest_matrix", "feature_check_first_count", "feature_check_rest_count", "feature_check_count"],
    ]) {
      assert.deepEqual(names(first), real.waves[lane].first, `${lane}: the first-wave matrix holds the first wave`);
      assert.deepEqual(names(rest), real.waves[lane].rest, `${lane}: the rest matrix holds the rest`);
      assert.equal(Number(lines[firstCount]), real.waves[lane].first.length, `${lane}: first count`);
      assert.equal(Number(lines[restCount]), real.waves[lane].rest.length, `${lane}: rest count`);
      assert.equal(Number(lines[total]), real.waves[lane].first.length + real.waves[lane].rest.length, `${lane}: the count is the lane's whole`);
    }
    // A table the planner cannot read stops the plan: schema 1 keyed rows by
    // their full names, and a key that still carries a member count can
    // never match a row.
    for (const [label, file] of [
      ["the previous schema", table({ "clippy:core": 600 }, { schema_version: 1 })],
      ["a member count", table({ "clippy:core+12": 600 })],
      ["an unknown lane", table({ "integration:core": 600 })],
      ["zero seconds", table({ "clippy:core": 0 })],
      ["fractional seconds", table({ "clippy:core": 600.5 })],
      ["seconds as text", table({ "clippy:core": "600" })],
    ]) {
      const result = run(["--format", "json", "--row-durations", file, "--", "crates/meerkat-core/src/lib.rs"]);
      assert.equal(result.status, 1, `${label}: the plan stops`);
    }
    // The table's generator: a row is its lane and its matrix name's key, and
    // keeps the longest successful job, across packings of one anchor too.
    assert.equal(rowOf("Clippy (mob)"), "clippy:mob");
    assert.equal(rowOf("Main unit tests (meerkat[openai-live])"), "main-unit:meerkat[openai-live]");
    assert.equal(rowOf("Closure tests (tests mob 1/4+3)"), "closure-tests:tests mob 1/4");
    assert.equal(rowOf("Feature combinations (mob+4 bazel set)"), "feature-check:mob bazel set");
    assert.equal(rowOf("Unit tests (core+4)"), "unit:core");
    assert.equal(rowOf("CI gate"), null);
    assert.equal(rowOf("Integration tests (runtime)"), null);
    const job = (name, conclusion, seconds) => ({ name, conclusion, started_at: "2026-01-01T00:00:00Z", completed_at: new Date(Date.parse("2026-01-01T00:00:00Z") + seconds * 1000).toISOString() });
    assert.deepEqual(
      durationsOf([job("Clippy (mob)", "success", 600), job("Clippy (mob)", "success", 700), job("Clippy (mob)", "failure", 900), job("Clippy (rpc)", "cancelled", 800), job("CI gate", "success", 30)]),
      { "clippy:mob": 700 },
      "the longest successful job of each row",
    );
    assert.deepEqual(
      durationsOf([job("Clippy (core+12)", "success", 300), job("Clippy (core+13)", "success", 400)]),
      { "clippy:core": 400 },
      "two packings of one anchor share its row",
    );
    // The checked-in table is readable and measures only the planned lanes.
    const durations = readRowDurations(resolve(root, ROW_DURATIONS));
    assert.ok(Object.keys(durations).length > 0, "the checked-in table measures rows");
    for (const key of Object.keys(durations)) {
      assert.ok(["unit", ...WAVE_LANES].includes(key.slice(0, key.indexOf(":"))), `${key} names a measured lane`);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// Every unit-lane row names the targets its packages have: Cargo refuses
// --lib for a run whose packages have no library (rkat and
// surface-build-fixtures are binaries only; a push's main unit lane runs
// rkat in a row of its own), and a row still covers every lib and bin
// target of its packages. Checked over every row of the workspace plans and
// of each binary-only package's own push.
{
  const LIB_KINDS = ["lib", "rlib", "cdylib", "dylib", "staticlib", "proc-macro"];
  const kinds = new Map(workspacePackages(readMetadata()).map((pkg) => [pkg.name, new Set(pkg.targets.flatMap((target) => target.kind))]));
  const binaryOnly = [...kinds].filter(([, set]) => !LIB_KINDS.some((kind) => set.has(kind))).map(([name]) => name).sort();
  assert.deepEqual(binaryOnly, ["rkat", "surface-build-fixtures"], "the workspace's binary-only packages");
  const UNIT_KEYS = ["unit_shards", "unit_feature_shards", "main_unit_shards", "main_feature_unit_shards"];
  let checked = 0;
  for (const [label, paths, extra] of [
    ["workspace push", ["Cargo.lock"], []],
    ["workspace pull request", ["Cargo.lock"], ["--event", "pull_request"]],
    ["rkat push", ["crates/meerkat-cli/src/main.rs"], []],
    ["fixtures push", ["tests/fixtures/surface-build-fixtures/src/main.rs"], []],
  ]) {
    const plan = planFor(paths, extra);
    for (const key of UNIT_KEYS) {
      for (const row of plan[key]) {
        const where = `${label}: ${key} ${row.name}`;
        // The unit jobs run `${TARGETS}` and refuse any other value.
        assert.ok(["--lib", "--bins", "--lib --bins"].includes(row.targets), `${where}: targets ${row.targets}`);
        const flags = row.targets.split(" ");
        const lib = row.packages.some((name) => LIB_KINDS.some((kind) => kinds.get(name).has(kind)));
        const bins = row.packages.some((name) => kinds.get(name).has("bin"));
        assert.equal(flags.includes("--lib"), lib, `${where}: --lib exactly when a package has a library`);
        assert.equal(flags.includes("--bins"), bins, `${where}: --bins exactly when a package has a binary`);
        checked += 1;
      }
    }
  }
  assert.ok(checked >= 40, `the check covers the workspace's unit rows (${checked})`);
  // rkat runs alone in a main unit row on a workspace push: its binary only.
  const rkat = planFor(["Cargo.lock"]).main_unit_shards.find((row) => row.packages.includes("rkat"));
  assert.deepEqual([rkat.packages, rkat.targets], [["rkat"], "--bins"]);
  const lines = Object.fromEntries(
    run(["--format", "github", "--", "Cargo.lock"]).stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
  const mainRows = ["main_unit_shard_matrix", "main_unit_rest_shard_matrix"].flatMap((key) => JSON.parse(lines[key]).include);
  assert.equal(mainRows.find((row) => row.packages === "-p rkat")?.targets, "--bins", "the matrix carries a row's targets");
  // The three unit jobs run the row's targets.
  const workflow = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  for (const job of ["unit", "main-unit", "main-unit-rest"]) {
    const text = jobLines(workflow, job).join("\n");
    assert.match(text, /^ {10}TARGETS: \$\{\{ matrix\.targets \}\}$/m, `${job} reads the row's targets`);
    assert.match(text, /^ {12}"--lib"\|"--bins"\|"--lib --bins"\) ;;$/m, `${job} refuses other targets`);
    assert.match(text, /nextest run \$\{PACKAGES\} \$\{TARGETS\} --profile ci-pr/, `${job} runs them`);
    assert.doesNotMatch(text, /nextest run \$\{PACKAGES\} --lib --bins/, `${job} names no fixed targets`);
  }
}

// The nested-Cargo canaries outlast a reader row's hung-test kill: a
// reader row of their binary runs its other named tests, and a pull request
// names the canaries as left to the push's closure-tests rows (one count
// partition each).
{
  const canaryRows = (tests) => {
    const deferred = { reader_rows: [] };
    const rows = readerRows({
      selections: [{ package: "facade", target: "test:canarybin", scope: "target", tests, features: [], paths: ["scripts/x"], reads: ["scripts/x"] }],
      result: { main_unit_shards: [], unit_packages: [], unit_shards: [], integration_suites: [] },
      byName: new Map([["facade", { name: "facade", targets: [{ name: "facade", kind: ["lib"] }, { name: "canarybin", kind: ["test"] }] }]]),
      heavyChain: new Set(),
      deferToPush: true,
      deferred,
      compiledInDocs: new Set(),
      unified: new Map([["facade", new Set()]]),
      canary: { binary: "canarybin", prefix: "downstream_" },
    });
    return [rows.map((row) => row.test_flags), deferred.reader_rows];
  };
  assert.deepEqual(canaryRows(["downstream_builds", "static_check"]), [
    ["--test canarybin -E test(/(^|::)(static_check)$/)"],
    ["facade:test:canarybin canaries"],
  ]);
  assert.deepEqual(canaryRows(["downstream_builds"]), [[], ["facade:test:canarybin canaries"]]);
  assert.deepEqual(canaryRows(null), [[], ["facade:test:canarybin canaries"]], "the whole canary binary is the push's");
  assert.deepEqual(canaryRows(["static_check"]), [["--test canarybin -E test(/(^|::)(static_check)$/)"], []]);
}

// Compiled-in documentation is a crate's reader of a doc in its own
// directory: another crate's reader of that doc, a test-only reader and a
// reader of a doc outside every crate are ordinary readers.
{
  const owners = { "crates/a/skills/x/SKILL.md": "a", "crates/a/README.md": "a" };
  const keys = compiledInDocKeys(
    [
      { path: "crates/a/skills/x/SKILL.md", package: "a", scope: "package" },
      { path: "crates/a/skills/x/SKILL.md", package: "b", scope: "package" },
      { path: "crates/a/README.md", package: "a", scope: "target" },
      { path: "docs/x.mdx", package: "a", scope: "package" },
    ],
    (path) => owners[path],
  );
  assert.deepEqual([...keys], ["crates/a/skills/x/SKILL.md\0a"]);
}

// A reader in library, binary or build-script code of a binary-only
// package (the class rkat's main unit row hit): its row names only the
// target kinds the package has.
{
  const byName = new Map([["binonly", { name: "binonly", targets: [{ name: "binonly", kind: ["bin"] }, { name: "it", kind: ["test"] }] }]]);
  const rowsFor = (deferToPush) =>
    readerRows({
      selections: [{ package: "binonly", target: "bin:binonly", scope: "package", tests: null, features: [], paths: ["docs/x.mdx"], reads: ["docs/"] }],
      result: { main_unit_shards: [], unit_packages: [], unit_shards: [], integration_suites: [] },
      byName,
      heavyChain: new Set(),
      deferToPush,
      deferred: { reader_rows: [] },
      compiledInDocs: new Set(),
      unified: new Map([["binonly", new Set()]]),
    }).map((row) => `${row.package_flags} | ${row.test_flags}`);
  assert.deepEqual(rowsFor(false), ["-p binonly | --bins"], "a push with no main unit lane runs its bins, not --lib");
  assert.deepEqual(rowsFor(true), ["-p binonly | --bins"], "a pull request runs its bins and leaves its test targets to the push");
  // A library reader of a package without a library is an inventory error,
  // never a --lib row.
  assert.throws(
    () =>
      readerRows({
        selections: [{ package: "binonly", target: "lib", scope: "target", tests: null, features: [], paths: ["docs/x.mdx"], reads: ["docs/"] }],
        result: { main_unit_shards: [], unit_packages: [], unit_shards: [], integration_suites: [] },
        byName,
        heavyChain: new Set(),
        deferToPush: false,
        deferred: { reader_rows: [] },
        compiledInDocs: new Set(),
        unified: new Map([["binonly", new Set()]]),
      }),
    /names the library of binonly, which has none/,
  );
}

// The two-wave lanes in ci.yml: each lane's rest wave is a copy of its job
// that differs only in its key, its needs (the stagger job), its row count
// and its matrix; the stagger job waits on a push only; the short lanes queue
// behind it, the long single lanes do not; and the gate waits for every wave.
{
  const workflow = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  for (const [job, firstCount, restCount, matrix, restMatrix] of [
    ["clippy", "shard_first_count", "shard_rest_count", "shard_matrix", "shard_rest_matrix"],
    ["main-unit", "main_unit_first_count", "main_unit_rest_count", "main_unit_shard_matrix", "main_unit_rest_shard_matrix"],
    ["closure-tests", "closure_test_first_count", "closure_test_rest_count", "closure_test_matrix", "closure_test_rest_matrix"],
    ["feature-check", "feature_check_first_count", "feature_check_rest_count", "feature_check_matrix", "feature_check_rest_matrix"],
  ]) {
    const first = jobLines(workflow, job);
    const rest = jobLines(workflow, `${job}-rest`);
    assert.equal(first.length, rest.length, `${job}: the rest wave has the job's lines`);
    first.forEach((line, index) => {
      const twin = rest[index];
      if (line === twin) return;
      if (line === "    needs: changes") return assert.equal(twin, "    needs: [changes, stagger]", `${job}: the rest wave waits for the stagger job`);
      if (line.startsWith("    if: ")) {
        assert.ok(line.includes(`needs.changes.outputs.${firstCount} != '0'`), `${job}: the first wave runs when it holds rows`);
        return assert.equal(twin, line.replace(firstCount, restCount), `${job}: the rest wave runs when it holds rows, on the job's condition`);
      }
      if (line.startsWith("      matrix: ")) {
        assert.equal(line, `      matrix: \${{ fromJSON(needs.changes.outputs.${matrix}) }}`, `${job}: first-wave matrix`);
        return assert.equal(twin, `      matrix: \${{ fromJSON(needs.changes.outputs.${restMatrix}) }}`, `${job}: rest matrix`);
      }
      assert.fail(`${job}: the rest wave differs from the job in ${JSON.stringify(line)} / ${JSON.stringify(twin)}`);
    });
  }
  // The feature-check rows are compile-only, but a row whose objects are not
  // in sccache yet runs long: the rkat+6 matrix row took 41m23s cold on
  // dispatch 39211283488 and was cancelled at the old 45-minute cap on main
  // push 39213218053 after its commands had run 44m29s. Both waves allow
  // exactly 60 minutes: room for a cold row, and no more, so a hung row still
  // fails early.
  for (const job of ["feature-check", "feature-check-rest"]) {
    const timeout = jobLines(workflow, job).find((line) => /^    timeout-minutes: \d+$/.test(line));
    assert.equal(
      timeout,
      "    timeout-minutes: 60",
      `${job}: ${timeout?.trim() ?? "no timeout-minutes"}, not 60 (a cold feature row takes up to 45 minutes, and a longer cap only delays a hung row)`,
    );
  }
  // The integration suites, the example web suites and the confinement lane
  // compile cold on the first push or dispatch of a tree that changes
  // meerkat-core: a pull request keeps its caps, a push or dispatch gets
  // caps sized from the cold runs (the comments beside each job).
  for (const [job, pullRequest, push] of [
    ["integration", 30, 50],
    ["example-web", 25, 35],
    ["mcp-confinement-positive", 40, 50],
  ]) {
    const timeout = jobLines(workflow, job).find((line) => /^    timeout-minutes: /.test(line));
    assert.equal(
      timeout,
      `    timeout-minutes: \${{ github.event_name == 'pull_request' && ${pullRequest} || ${push} }}`,
      `${job}: ${timeout?.trim() ?? "no timeout-minutes"}, not ${pullRequest} minutes on a pull request and ${push} on a push or dispatch`,
    );
  }
  // A pull request that leaves the web wasm to the push still runs the Web
  // SDK's type and unit tests: every step after them that needs Rust, the
  // wasm or a browser runs only when the plan builds the wasm.
  {
    const steps = [];
    for (const line of jobLines(workflow, "example-web")) {
      const name = /^      - name: (.*)$/.exec(line);
      if (name) steps.push({ name: name[1], lines: [] });
      else if (steps.length > 0) steps.at(-1).lines.push(line);
    }
    const wasmGate = "        if: ${{ needs.changes.outputs.example_web_wasm == 'true' }}";
    const ungated = steps.filter((step) => !step.lines.includes(wasmGate)).map((step) => step.name);
    assert.deepEqual(
      ungated,
      ["Checkout", "Setup Node", "Install the Web SDK", "Web SDK type and unit tests"],
      "only the checkout, Node and the Web SDK's type and unit tests run without the wasm",
    );
    const unitTests = steps.find((step) => step.name === "Web SDK type and unit tests");
    assert.ok(unitTests.lines.includes("        run: npm test"), "the Web SDK's type and unit tests run npm test");
    assert.ok(
      steps.some((step) => step.name === "Build the Web SDK wasm" && step.lines.includes("        run: npm run build:wasm")),
      "the wasm build is its own gated step",
    );
    assert.ok(
      workflow.includes("      example_web_wasm: ${{ steps.plan.outputs.example_web_wasm }}\n"),
      "the classification job exports example_web_wasm",
    );
    assert.ok(
      jobLines(workflow, "wasm-timers").includes(
        "    if: ${{ (github.event_name == 'push' && needs.changes.outputs.release_projection != 'true' && needs.changes.outputs.core_paths_changed == 'true') || needs.changes.outputs.example_web_wasm == 'true' }}",
      ),
      "the WASM timer lane, which compiles the same closure, runs with the web wasm build (a push that changes only a product's tree runs neither)",
    );
  }
  // Every run checks the generated BUILD files against their inputs: the
  // bazel-graph job's query and --nobuild build accept a stale graph that is
  // still valid, and the commit hook is local and never sees a merge commit.
  // Format + governance runs on every event, and the step has no condition.
  const governance = jobLines(workflow, "fmt-governance");
  assert.ok(!governance.some((line) => /^    if: /.test(line)), "Format + governance runs on every event");
  const freshness = governance.indexOf("      - name: Generated Bazel BUILD files are current");
  assert.ok(freshness >= 0, "Format + governance checks that the generated Bazel BUILD files are current");
  const nextStep = governance.findIndex((line, index) => index > freshness && /^      - /.test(line));
  assert.deepEqual(
    governance.slice(freshness + 1, nextStep === -1 ? governance.length : nextStep),
    ["        run: node scripts/generate-bazel-rust-builds.mjs --check"],
    "the BUILD freshness step runs the generator's check and nothing else, on every run",
  );
  const stagger = jobLines(workflow, "stagger").join("\n");
  assert.match(stagger, /^    needs: changes$/m, "the stagger job follows the classification");
  assert.match(stagger, /STAGGER_SECONDS: \$\{\{ github\.event_name == 'pull_request' && '0' \|\| '30' \}\}/, "it waits on a push only");
  assert.match(stagger, /run: sleep "\$\{STAGGER_SECONDS\}"$/m);
  for (const job of ["ratchets", "integration", "governed-jsonl", "closure-check", "sdk-host", "example-web", "wasm-timers", "bazel-graph"]) {
    assert.ok(jobLines(workflow, job).includes("    needs: [changes, stagger]"), `${job} is a short lane: it waits for the stagger job`);
  }
  for (const job of ["wasm-check", "governance-gates", "mcp-confinement-positive", "tlc-audits", "main-unit-archive-build", "clippy", "main-unit", "closure-tests", "feature-check", "rustdoc"]) {
    assert.ok(jobLines(workflow, job).includes("    needs: changes"), `${job} starts with the first wave`);
  }
  const gate = jobLines(workflow, "gate");
  for (const job of ["stagger", "clippy-rest", "main-unit-rest", "closure-tests-rest", "feature-check-rest"]) {
    assert.ok(gate.includes(`      - ${job}`), `the gate waits for ${job}`);
  }
}

// Format + governance ends with the Mintlify parse and link check of the docs
// site (docs/) and of MobKit's source pages (meta/docs): Node.js 22, then the
// mint version the MobKit mirror's publication pins, so an MDX error fails the
// pull request rather than the publication after the merge. Each directory's
// .mintignore keeps its internal records, which MDX cannot parse, out of it.
{
  const workflow = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  const publish = readFileSync(resolve(root, ".github/workflows/publish-mobkit-docs.yml"), "utf8");
  const pin = publish.match(/npx --yes (mint@[0-9.]+) broken-links/);
  assert.ok(pin, "the MobKit docs publication pins its mint version");
  const mint = pin[1].replaceAll(".", "\\.");
  const expected = [
    /^      - name: Setup Node\.js for the Mintlify docs parse$/,
    /^        uses: actions\/setup-node@[0-9a-f]{40} # v\d+$/,
    /^        with:$/,
    /^          node-version: "22"$/,
    /^      - name: Mintlify parse and links \(docs\)$/,
    /^        working-directory: docs$/,
    new RegExp(`^        run: npx --yes ${mint} broken-links$`),
    /^      - name: Mintlify parse and links \(meta\/docs\)$/,
    /^        working-directory: meta\/docs$/,
    new RegExp(`^        run: npx --yes ${mint} broken-links$`),
  ];
  const tail = jobLines(workflow, "fmt-governance").slice(-expected.length);
  tail.forEach((line, index) =>
    assert.match(line, expected[index], `Format + governance ends with the Mintlify docs parse (line ${index + 1} of ${expected.length})`),
  );
  for (const dir of ["docs", "meta/docs"]) {
    const ignored = readFileSync(resolve(root, dir, ".mintignore"), "utf8").split("\n");
    assert.ok(ignored.includes("internal/"), `${dir}/.mintignore keeps the internal records out of the parse`);
  }
}

// Feature-gated unit suites: the default-feature unit lanes cannot run a
// test behind a non-default feature, so every Rust-relevant push to main
// carries every suite as an extra main unit row, and a pull request carries
// the suites of a changed package that runs its unit tests there.
{
  // A suite built `with` other packages names them after its own, and its
  // features by package.
  const featureFlags = /^-p (\S+)((?: -p \S+)*) --features ([a-z0-9-]+\/)?[a-z0-9-]+(,([a-z0-9-]+\/)?[a-z0-9-]+)*$/;
  const facade = planFor(["crates/meerkat/src/experimental_gpt_live.rs"]);
  assert.ok(facade.main_feature_unit_shards.length >= 2, "main carries the feature suites");
  for (const suite of facade.main_feature_unit_shards) {
    const match = featureFlags.exec(suite.package_flags);
    assert.ok(match, `feature suite ${suite.name} flags: ${suite.package_flags}`);
    const features = suite.package_flags.split(" --features ")[1].split(",");
    assert.ok(
      features.every((feature) => (match[2] ? feature.startsWith(`${match[1]}/`) : !feature.includes("/"))),
      `${suite.name} names its features by package exactly when it builds with other packages`,
    );
    assert.deepEqual(suite.packages, [match[1]], `${suite.name} runs exactly its package`);
    assert.ok(suite.packages[0] in facade.package_model, `${suite.name} names a workspace package`);
  }
  const names = facade.main_feature_unit_shards.map((suite) => suite.name);
  assert.equal(new Set(names).size, names.length, "feature suite names are unique (they name matrix rows and artifacts)");
  assert.ok(
    facade.main_feature_unit_shards.some((suite) => suite.packages[0] === "meerkat" && suite.features.includes("openai-live")),
    "the facade's public GPT Live suite is on main",
  );
  // The facade runs its unit tests in the pull request, so its suites do too.
  assert.deepEqual(
    unitOf(facade).suites.map((suite) => suite.packages[0]).filter((name) => name !== "meerkat"),
    [],
    "only the changed package's suites join the pull request",
  );
  assert.ok(unitOf(facade).suites.some((suite) => suite.features.includes("openai-live")), "the facade touch runs its GPT Live suite in the pull request");
  // meerkat-openai's realtime adapter (live.rs, text_adapter.rs) is behind
  // `realtime`, which neither its default features nor its `live` suite
  // enable: its own suite runs those tests in the pull request and on main.
  const openai = planFor(["crates/meerkat-openai/src/live.rs"], ["--event", "pull_request"]);
  const realtime = (suites) => suites.filter((suite) => suite.packages[0] === "meerkat-openai" && suite.features.includes("realtime"));
  assert.equal(realtime(unitOf(openai).suites).length, 1, "a pull request that changes meerkat-openai runs its realtime suite");
  assert.equal(realtime(facade.main_feature_unit_shards).length, 1, "main runs meerkat-openai's realtime suite");
  for (const suite of unitOf(facade).suites) {
    assert.ok(suite.estimated_minutes <= facade.pr_unit_budget_minutes, `${suite.name} fits the pull-request budget`);
  }
  const github = run(["--format", "github", "--", "crates/meerkat/src/experimental_gpt_live.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  const unitRows = JSON.parse(lines.unit_shard_matrix).include;
  // The main unit rows of both waves (long rows first).
  const mainRows = [
    ...JSON.parse(lines.main_unit_shard_matrix).include,
    ...JSON.parse(lines.main_unit_rest_shard_matrix).include,
  ];
  assert.equal(Number(lines.main_unit_shard_count), mainRows.length);
  assert.equal(Number(lines.main_unit_first_count) + Number(lines.main_unit_rest_count), mainRows.length);
  // A push's unit matrix holds only the placeholder row; its unit rows are
  // main matrix rows.
  assert.equal(Number(lines.unit_shard_count), 0);
  assert.deepEqual(unitRows, [{ name: "none", packages: "" }]);
  assert.equal(Number(lines.unit_rows_on_main_count), unitOf(facade).shards.length + unitOf(facade).suites.length);
  assert.equal(mainRows.length, facade.main_unit_shards.length + facade.main_feature_unit_shards.length);
  for (const suite of facade.main_feature_unit_shards) {
    assert.ok(mainRows.some((row) => row.name === suite.name && row.packages === suite.package_flags), `main matrix row for ${suite.name}`);
  }

  // A heavy-chain package defers its suites with its unit tests.
  const mob = planFor(["crates/meerkat-mob/src/lib.rs"]);
  assert.deepEqual(mob.unit_feature_shards, [], "no pull-request feature suite may compile meerkat-mob");
  // meerkat-mob's default lane and its feature suite run as archived lanes:
  // built once, executed in partitions.
  assert.ok(
    !mob.main_feature_unit_shards.some((suite) => suite.packages[0] === "meerkat-mob"),
    "mob's suite leaves the packed feature rows",
  );
  assert.deepEqual(
    mob.main_archive_builds.map((build) => build.package_flags),
    ["-p meerkat-mob", "-p meerkat-mob --features experimental-gpt-live,schema"],
    "mob's default lane and feature suite build archives on main",
  );
  const mobGithub = run(["--format", "github", "--", "crates/meerkat-mob/src/lib.rs"]);
  assert.equal(mobGithub.status, 0, mobGithub.stderr);
  const mobLines = Object.fromEntries(mobGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(Number(mobLines.main_archive_build_count), JSON.parse(mobLines.main_archive_build_matrix).include.length);
  assert.equal(Number(mobLines.main_archive_run_count), JSON.parse(mobLines.main_archive_run_matrix).include.length);
  // meerkat-connect builds meerkat-mob through toolkit-mobkit's native host,
  // so a Connect change runs its Console suite on the push, never in the
  // pull request.
  const connectLib = "connect/crates/meerkat-connect/src/lib.rs";
  const connectPr = planFor([connectLib], ["--event", "pull_request"]);
  assert.ok(connectPr.package_model["meerkat-connect"].heavy_chain, "meerkat-connect's unit lane compiles meerkat-mob");
  assert.deepEqual(connectPr.unit_feature_shards, [], "a Connect pull request runs no feature suite");
  assert.ok(
    planFor([connectLib]).main_feature_unit_shards.some((suite) => suite.package_flags === "-p meerkat-connect --features console"),
    "a Connect push runs its Console suite",
  );
  // Its Console process controls (resources::local_round) run a synthetic
  // protocol child under an explicitly selected Python and refuse to guess
  // one, so every unit job selects it, whichever wave or event holds the row.
  // The tests see it only when the shell that runs nextest exported it first:
  // in that step, before the nextest command.
  const unitJobs = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  for (const job of ["unit", "main-unit", "main-unit-rest"]) {
    const jobBody = jobLines(unitJobs, job);
    assert.match(
      jobBody.join("\n"),
      /^ {10}TOOLKIT_SHARED_TEST_PYTHON="\$\(command -v python3\)"\n {10}export TOOLKIT_SHARED_TEST_PYTHON$/m,
      `${job} selects the Python of Connect's Console process controls`,
    );
    const exportAt = jobBody.indexOf("          export TOOLKIT_SHARED_TEST_PYTHON");
    const nextestAt = jobBody.findIndex((line) => line.includes("nextest run"));
    assert.ok(exportAt >= 0 && exportAt < nextestAt, `${job} exports the Python before nextest runs`);
    assert.ok(
      !jobBody.slice(exportAt, nextestAt).some((line) => /^ {6}- /.test(line)),
      `${job} exports the Python in the step that runs nextest`,
    );
  }
  // An unrelated leaf change carries no pull-request suite; docs carry none anywhere.
  assert.deepEqual(planFor(["crates/meerkat-sqlite/src/lib.rs"]).unit_feature_shards, []);
  const docs = planFor(["docs/index.mdx"]);
  assert.deepEqual(docs.unit_feature_shards, []);
  assert.deepEqual(docs.main_feature_unit_shards, []);
}

// Integration suites: a directly changed trigger package runs the suite's
// tests/*.rs binaries; a leaf change and docs run none; workspace mode runs
// every suite; the github output carries the matrix rows.
{
  const names = (plan) => plan.integration_suites.map((suite) => suite.packages[0]);
  assert.deepEqual(
    names(planFor(["crates/meerkat-runtime/src/lib.rs"])),
    ["meerkat-runtime", "meerkat-machine-codegen", "meerkat", "meerkat", "meerkat-integration-tests", "meerkat-authorization"],
  );
  assert.deepEqual(
    names(planFor(["crates/meerkat-machine-schema/src/lib.rs"])),
    ["meerkat-runtime", "meerkat-machine-codegen", "meerkat-machine-kernels", "meerkat-authorization"],
    "a machine schema or DSL change runs the runtime, codegen parity, kernel oracle and authorization suites",
  );
  assert.deepEqual(
    names(planFor(["crates/meerkat-mob/src/lib.rs"])),
    ["meerkat-machine-codegen", "meerkat-integration-tests"],
    "mob runs the codegen parity suite and the gpt-live replays",
  );
  assert.deepEqual(names(planFor(["crates/meerkat-machine-codegen/tests/runtime_alphabet_parity.rs"])), ["meerkat-machine-codegen"]);
  // The store and cold-restart suites follow every store backend change.
  assert.deepEqual(names(planFor(["crates/meerkat-sqlite/src/lib.rs"])), ["meerkat"]);
  const store = planFor(["crates/meerkat-store/src/lib.rs"]);
  assert.deepEqual(names(store), ["meerkat", "meerkat"]);
  assert.equal(store.integration_suites[0].name, "meerkat-store-restart");
  // The governed realm suite follows its store, runtime and MCP owners.
  assert.equal(store.integration_suites[1].name, "meerkat-governed-realm");
  assert.match(store.integration_suites[1].package_flags, /^-p meerkat --features .*\blocal-authorization\b/);
  assert.match(store.integration_suites[1].test_flags, /^--test governed_realm$/);
  assert.match(store.integration_suites[0].package_flags, /^-p meerkat --features .*\bsession-store\b/);
  assert.match(store.integration_suites[0].test_flags, /--test cold_restart_resume_after_compaction\b/);
  assert.deepEqual(names(planFor(["crates/meerkat-tools/src/lib.rs"])), ["meerkat-authorization"]);
  assert.deepEqual(names(planFor(["docs/index.mdx"])), []);
  assert.deepEqual(
    names(planFor(["Cargo.toml"])),
    ["meerkat-runtime", "meerkat-machine-codegen", "xtask", "meerkat-machine-kernels", "meerkat", "meerkat", "meerkat", "meerkat-rpc", "meerkat-integration-tests", "meerkat-authorization"],
    "workspace mode runs every suite",
  );
  // Config layering, core construction and facade changes must execute both
  // regressions; an RPC-only change selects its own test binary.
  const limitNames = (plan) => plan.integration_suites
    .map((suite) => suite.name)
    .filter((name) => ["standalone-turn-lifecycle", "rpc-configured-turn-limit"].includes(name));
  for (const path of [
    "crates/meerkat-core/src/config.rs",
    "crates/meerkat-core/src/agent/builder.rs",
    "crates/meerkat/src/factory.rs",
    "crates/meerkat/tests/standalone_turn_lifecycle.rs",
  ]) {
    assert.deepEqual(limitNames(planFor([path])), ["standalone-turn-lifecycle", "rpc-configured-turn-limit"], `${path} runs both limit regressions`);
  }
  for (const path of [
    "crates/meerkat-rpc/src/session_runtime.rs",
    "crates/meerkat-rpc/tests/configured_turn_limit.rs",
  ]) {
    assert.deepEqual(limitNames(planFor([path])), ["rpc-configured-turn-limit"], `${path} runs the RPC limit regression`);
  }
  for (const path of ["crates/meerkat-openai/src/lib.rs", "docs/index.mdx"]) {
    assert.deepEqual(limitNames(planFor([path])), [], `${path} does not add limit regression lanes`);
  }
  // The gpt-live replays: a public Live path package or a fixture change
  // runs exactly the replay target with its feature.
  const replay = planFor(["crates/meerkat-openai/src/public_live.rs"]);
  assert.deepEqual(names(replay), ["meerkat-integration-tests"]);
  assert.equal(replay.integration_suites[0].name, "gpt-live-replay");
  assert.equal(replay.integration_suites[0].package_flags, "-p meerkat-integration-tests --features gpt-live-replay");
  assert.equal(replay.integration_suites[0].test_flags, "--test gpt_live_replay");
  const fixture = planFor(["tests/integration/fixtures/gpt_live_replay/s104.provider-stream.jsonl"]);
  assert.deepEqual(names(fixture), ["meerkat-integration-tests"], "a fixture-only change runs the replays");
  // Authorization's native model/tool loops and ordinary cost controls are
  // integration binaries, so its default-feature unit row cannot run them.
  for (const path of [
    "crates/meerkat-authorization/src/grant_policy.rs",
    "crates/meerkat-authorization/tests/native_governed_loop.rs",
    "crates/meerkat-authorization/tests/native_cost.rs",
    "crates/meerkat-authorization-contracts/src/grant.rs",
    "crates/meerkat-core/src/agent/state.rs",
    "crates/meerkat-runtime/src/store/execution_custody.rs",
    "crates/meerkat/src/session_factory.rs",
    "crates/meerkat-tools/src/lib.rs",
    "crates/meerkat-llm-core/src/lib.rs",
    "crates/meerkat-anthropic/src/lib.rs",
    "crates/meerkat-auth-core/src/lib.rs",
    "crates/meerkat-models/src/lib.rs",
    "crates/meerkat-machine-schema/src/lib.rs",
    "crates/meerkat-machine-dsl/src/lib.rs",
    "crates/meerkat-machine-dsl-core/src/lib.rs",
    "crates/meerkat-machine-derive/src/lib.rs",
    "crates/meerkat-machine-kernels/src/lib.rs",
  ]) {
    const authorization = planFor([path]);
    const selected = authorization.integration_suites.filter((suite) => suite.packages[0] === "meerkat-authorization");
    assert.equal(selected.length, 1, `${path} selects exactly one native authorization suite`);
    assert.equal(selected[0].package_flags, "-p meerkat-authorization", "ordinary native fixtures keep default features and ignored acceptance opt-in");
  }
  const authorizationGithub = run(["--format", "github", "--", "crates/meerkat-authorization/tests/native_governed_loop.rs"]);
  assert.equal(authorizationGithub.status, 0, authorizationGithub.stderr);
  const authorizationLines = Object.fromEntries(authorizationGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  // The suite, then the push's whole-checkout reader row (xtask's
  // machine-authority contracts, which the closure-tests rows do not build).
  assert.equal(authorizationLines.integration_count, "2");
  assert.deepEqual(JSON.parse(authorizationLines.integration_matrix).include.map((row) => row.packages), [
    "-p meerkat-authorization",
    "-p xtask --features machine-authority",
  ]);
  // xtask's integration tests pin the workflows: an xtask change or a
  // workflow-only edit (a plan with no Rust change) runs them, with the
  // feature its machines_contracts target requires.
  const xtask = planFor(["crates/xtask/src/machines.rs"]);
  assert.deepEqual(names(xtask), ["xtask"]);
  assert.equal(xtask.integration_suites[0].package_flags, "-p xtask --features machine-authority");
  const workflow = planFor([".github/workflows/nightly.yml"]);
  assert.equal(workflow.rust_changed, false);
  assert.deepEqual(names(workflow), ["xtask"], "a workflow-only edit runs the xtask workflow pins");
  // buildbuddy_static_lanes reads the SDK manifests and generated types.
  for (const path of ["sdks/web/package.json", "sdks/typescript/src/generated/types.ts", "sdks/python/meerkat/generated/types.py"]) {
    assert.ok(names(planFor([path])).includes("xtask"), `${path} runs the xtask integration suite`);
  }
  const github = run(["--format", "github", "--", "crates/meerkat-runtime/src/lib.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  // The suites, then the push's whole-checkout reader row.
  const allRows = JSON.parse(lines.integration_matrix).include;
  assert.equal(Number(lines.integration_count), 7);
  assert.deepEqual(allRows.slice(6).map((row) => row.name), ["reader xtask:test:machines_contracts"]);
  const rows = allRows.slice(0, 6);
  assert.deepEqual(rows.map((row) => row.packages.split(" --features")[0]), [
    "-p meerkat-runtime",
    "-p meerkat-machine-codegen",
    "-p meerkat",
    "-p meerkat",
    "-p meerkat-integration-tests",
    "-p meerkat-authorization",
  ]);
  assert.equal(rows[3].packages, "-p meerkat --features session-store,local-authorization,mcp");
  assert.equal(rows[4].packages, "-p meerkat-integration-tests --features gpt-live-replay");
  assert.deepEqual(rows.map((row) => row.tests), [
    undefined,
    undefined,
    "--test cold_restart_resume_after_compaction --test cold_restart_resume --test persistence_contract --test storage_provider_seam --test brain_swap_surface_parity",
    "--test governed_realm",
    "--test gpt_live_replay",
    undefined,
  ]);
  // A docs edit selects no suite: its integration rows are its reader rows
  // (rkat's doc-example test runs in the push's closure-tests rows).
  const docsGithub = run(["--format", "github", "--", "docs/index.mdx"]);
  const docsLines = Object.fromEntries(docsGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(docsLines.integration_count, "2");
  assert.deepEqual(JSON.parse(docsLines.integration_matrix).include.map((row) => row.name), [
    "reader xtask:package",
    "reader xtask:test:machines_contracts",
  ]);
  // With neither, the matrix keeps its placeholder.
  const noneGithub = run(["--format", "github", "--event", "pull_request", "--", "LICENSE-MIT"]);
  const noneLines = Object.fromEntries(noneGithub.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(noneLines.integration_count, "0");
  assert.deepEqual(JSON.parse(noneLines.integration_matrix).include, [{ name: "none", packages: "" }]);
}

// Bazel graph check selection: Bazel-relevant paths, any Cargo manifest, and
// moved or deleted Rust files select it; documentation does not.
{
  const plan = planFor(["crates/meerkat-core/BUILD.bazel"]);
  assert.equal(plan.bazel_graph, true, "a BUILD-only diff selects the Bazel graph check");
  assert.equal(plan.examples_browser, false);
}
{
  for (const path of ["MODULE.bazel", "MODULE.bazel.lock", ".bazelrc", ".bazelversion", "tools/bazel/defs.bzl", "tools/buildbuddy/README.md", "scripts/generate-bazel-rust-builds.mjs", "scripts/rust-source-text.mjs", "scripts/rust-test-selector.mjs", "scripts/non-shipping-features.json", "Cargo.lock"]) {
    assert.equal(planFor([path]).bazel_graph, true, `${path} selects the Bazel graph check`);
  }
}
{
  const plan = planFor(["docs/index.mdx", "README.md"]);
  assert.equal(plan.bazel_graph, false, "a docs-only diff does not select the Bazel graph check");
  assert.equal(plan.examples_browser, false, "a docs-only diff does not select the example suites");
  assert.equal(plan.example_web, false, "a docs-only diff does not select the example web suites");
}
{
  const plan = planFor(["crates/meerkat-core/Cargo.toml"]);
  assert.equal(plan.bazel_graph, true, "a Cargo.toml change selects the Bazel graph check");
}
{
  const plan = planFor(["crates/meerkat-core/src/lib.rs"]);
  assert.equal(plan.bazel_graph, false, "an in-place Rust edit leaves the graph unchanged");
}
{
  // A rename inside crates/: the old path no longer exists, the new one does.
  const plan = planFor(["crates/meerkat-core/src/renamed_away_for_selftest.rs", "crates/meerkat-core/src/lib.rs"]);
  assert.equal(plan.bazel_graph, true, "a crates/ rename selects the Bazel graph check");
  assert.deepEqual(plan.removed_paths, ["crates/meerkat-core/src/renamed_away_for_selftest.rs"]);
}
{
  const result = run(["--format", "github", "--", "crates/meerkat-core/BUILD.bazel"]);
  const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.bazel_graph, "true");
  assert.equal(lines.examples_browser, "false");
}

// Example suites selection: the TypeScript and Python SDKs and example
// sources select them; example documentation does not.
{
  for (const path of ["sdks/typescript/src/index.ts", "sdks/python/meerkat/__init__.py", "examples/037-live-webrtc-web/app.js", "examples/tests/sdk-examples.test.mjs", "examples/package-lock.json"]) {
    assert.equal(planFor([path]).examples_browser, true, `${path} selects the example suites`);
  }
  assert.equal(planFor(["examples/037-live-webrtc-web/README.md"]).examples_browser, false);
  assert.equal(planFor(["crates/meerkat-core/src/lib.rs"]).examples_browser, false);
}

// Example web suites selection: the sdks/web runtime, the crates it builds
// from, and the sources of examples 031, 032 and 033 select them; their
// documentation, other examples and unrelated crates do not.
{
  // Dependency-only Rust changes must run both the browser examples and
  // WASM timer tests, whose workflow jobs share the example_web flag.
  for (const path of [
    "crates/meerkat-core/src/time_compat/wasm.rs",
    "crates/meerkat-core/src/lib.rs",
    "crates/meerkat-runtime/src/lib.rs",
    "crates/meerkat-session/src/lib.rs",
    "crates/meerkat-mob/src/lib.rs",
    "crates/meerkat-machine-kernels/src/lib.rs",
  ]) {
    const plan = planFor([path]);
    assert.equal(plan.mode, "packages", `${path} selects its owning package`);
    assert.equal(plan.packages.length, 1, `${path} does not escalate to the workspace`);
    assert.ok(plan.closure.includes("meerkat-web-runtime"), `${path} reaches the web runtime through dependencies`);
    assert.equal(plan.wasm, true, `${path} selects the WASM build`);
    assert.equal(plan.example_web, true, `${path} selects the browser and WASM timer suites`);
  }
  for (const path of [
    "sdks/web/scripts/build-wasm.mjs",
    "sdks/web/src/index.ts",
    "crates/meerkat-web-runtime/src/lib.rs",
    "crates/meerkat-contracts/src/lib.rs",
    "examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs",
    "examples/032-wasm-webcm-agent/web/package.json",
    "examples/033-the-office-demo-sh/web/tests/regression.cjs",
  ]) {
    assert.equal(planFor([path]).example_web, true, `${path} selects the example web suites`);
  }
  for (const path of [
    "examples/033-the-office-demo-sh/README.md",
    "examples/037-live-webrtc-web/app.js",
    "sdks/typescript/src/index.ts",
    "crates/meerkat-core/README.md",
    "CHANGELOG.md",
  ]) {
    assert.equal(planFor([path]).example_web, false, `${path} does not select the example web suites`);
  }
  for (const path of ["crates/meerkat-rpc/src/lib.rs", "crates/meerkat-cli/src/main.rs"]) {
    const plan = planFor([path]);
    assert.equal(plan.mode, "packages", `${path} still selects its Rust package`);
    assert.ok(!plan.closure.includes("meerkat-web-runtime"), `${path} has no reverse dependency on the web runtime`);
    assert.equal(plan.example_web, false, `${path} does not select the browser and WASM timer suites`);
  }
  for (const path of ["Cargo.lock", ".cargo/config.toml"]) {
    const plan = planFor([path]);
    assert.equal(plan.mode, "workspace", `${path} selects the workspace`);
    assert.equal(plan.example_web, true, `${path} selects the browser and WASM timer suites`);
  }
  for (const path of ["sdks/web/src/index.ts", "crates/meerkat-core/src/time_compat/wasm.rs"]) {
    const result = run(["--format", "github", "--", path]);
    assert.equal(result.status, 0, result.stderr);
    const lines = Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
    assert.equal(lines.example_web, "true", `${path} selects the suites in GitHub output`);
  }
}

// Feature-combination checks (#1687): each clippy shard gives a Bazel-set job
// (its packages under their Bazel test feature set) and a matrix job (their
// other Bazel library/binary sets and their Make feature-matrix rows).
{
  const rows = (plan) => plan.feature_check_jobs.flatMap((job) => job.commands);
  // #1595: meerkat-mob-mcp's lib tests under Bazel's `openai-live` alone.
  const mobMcp = planFor(["crates/meerkat-mob-mcp/src/lib.rs"]);
  assert.deepEqual(
    mobMcp.feature_check_jobs.map((job) => job.name),
    mobMcp.shards.flatMap((shard) => [`${shard.name} bazel set`, `${shard.name} matrix`]).filter((name) =>
      mobMcp.feature_check_jobs.some((job) => job.name === name),
    ),
  );
  assert.equal(mobMcp.feature_check_jobs[0].name, `${mobMcp.shards[0].name} bazel set`);
  assert.equal(mobMcp.feature_check_jobs[0].commands.length, 1, "the Bazel-set job holds only the Bazel row");
  assert.equal(
    rows(mobMcp)[0],
    "-p meerkat-mob-mcp --no-default-features --features meerkat-mob-mcp/openai-live --lib --bins --tests",
  );
  // #1558: `meerkat-mob --no-default-features` from the Make feature matrix,
  // and the production library variant without `test-support`.
  const mob = planFor(["crates/meerkat-mob/src/lib.rs"]);
  assert.match(rows(mob)[0], /^-p meerkat-mob --no-default-features --features \S*meerkat-mob\/test-support\S* --lib --tests$/);
  assert.ok(rows(mob).includes("-p meerkat-mob --no-default-features"), "the Make matrix row for meerkat-mob runs");
  assert.ok(rows(mob).includes("-p meerkat-mob --no-default-features --features runtime-adapter"));
  assert.ok(
    rows(mob).some((row) => /^-p meerkat-mob --no-default-features --features \S+ --lib$/.test(row) && !row.includes("test-support")),
    "the production library set without test-support is checked",
  );
  // A production target selects its release feature set (no test-only
  // feature) under //tools/bazel:release_build; both branches are checked.
  const core = planFor(["crates/meerkat-core/src/lib.rs"]);
  const coreRows = rows(core).filter((row) => row.startsWith("-p meerkat-core ") && row.endsWith(" --lib"));
  assert.ok(
    coreRows.some((row) => !row.includes("test-support")),
    "the release-configuration feature set of meerkat-core is checked",
  );
  assert.ok(
    coreRows.some((row) => row.includes("test-support")),
    "the default-configuration feature set of meerkat-core is checked",
  );
  // A bin-only package gets no --lib; weak dependency features stay out.
  const cli = planFor(["crates/meerkat-cli/src/main.rs"]);
  assert.match(rows(cli)[0], /^-p rkat --no-default-features --features \S+ --bins --tests$/);
  assert.ok(!rows(cli)[0].includes("?"), "weak dependency features are not passed to Cargo");
  for (const row of rows(cli)) {
    assert.match(row, /^-p rkat /, `${row} belongs to the changed package`);
  }
  // A workspace plan checks every package, shard by shard (the weight-packed
  // shards, not the Clippy head split), and runs every Make feature-matrix
  // check row with the shard that holds its package.
  const workspace = planFor(["Cargo.toml"]);
  // Core's packages only: the checks replay Core's Bazel feature sets, and
  // no Bazel target builds a product's crates.
  assert.deepEqual(
    workspace.feature_check_shards.flatMap((shard) => shard.packages).sort(),
    workspace.packages.filter((name) => workspace.package_model[name].product === CORE),
    "feature-check shards partition Core's packages",
  );
  assert.ok(workspace.feature_check_jobs.length <= 2 * workspace.feature_check_shards.length);
  for (const job of workspace.feature_check_jobs) {
    const shard = workspace.feature_check_shards.find(
      (candidate) => job.name === `${candidate.name} bazel set` || job.name === `${candidate.name} matrix`,
    );
    assert.ok(shard, `feature-check job ${job.name} follows a clippy shard`);
    for (const row of job.commands) {
      for (const [, name] of row.matchAll(/-p (\S+)/g)) {
        assert.ok(shard.packages.includes(name), `${job.name}: ${name} belongs to the shard`);
      }
    }
  }
  const bazelChecked = new Set(
    workspace.feature_check_jobs
      .filter((job) => job.name.endsWith(" bazel set"))
      .flatMap((job) => [...job.commands[0].matchAll(/-p (\S+)/g)].map((match) => match[1])),
  );
  for (const name of ["meerkat", "meerkat-mob", "meerkat-mob-mcp", "rkat", "meerkat-core"]) {
    assert.ok(bazelChecked.has(name), `the workspace plan checks ${name} under its Bazel test set`);
  }
  for (const row of [
    "-p meerkat-client --no-default-features",
    "-p meerkat-tools --no-default-features --features comms,mcp",
    "-p meerkat --no-default-features --features session-store,meerkat-store/jsonl",
  ]) {
    assert.ok(rows(workspace).includes(row), `workspace plan runs the Make row ${row}`);
  }
  // A plan without a Rust change has no feature checks; GitHub output keeps
  // a well-formed placeholder matrix behind a zero count.
  const docs = planFor(["docs/guides/deploying.mdx"]);
  assert.deepEqual(docs.feature_check_jobs, []);
  const github = run(["--format", "github", "--", "docs/guides/deploying.mdx"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.feature_check_count, "0");
  assert.deepEqual(JSON.parse(lines.feature_check_matrix).include, [{ name: "none", commands: "" }]);
}

// A change to the lane definitions runs the lanes they define.
{
  for (const path of [
    ".github/workflows/ci.yml",
    "scripts/ci-cargo-lanes.mjs",
    "scripts/ci-cargo-lanes-selftest.mjs",
    "scripts/ci-hook-coverage.mjs",
    "scripts/ci-verify-row-binaries.mjs",
    "scripts/ci-non-rust-readers.mjs",
  ]) {
    const plan = planFor([path]);
    assert.equal(plan.bazel_graph, true, `${path} selects the Bazel graph check`);
    assert.equal(plan.examples_browser, true, `${path} selects the example suites`);
    assert.equal(plan.example_web, true, `${path} selects the example web suites`);
  }
}

// Hook parity: workspace-unified features. meerkat-mcp's form_elicitation
// and lifecycle_contract require test-support, which only the feature
// unification of Core's whole workspace enables (a package-scoped build has
// none), so their row must carry it. Without a lib-row runner the lib tests of the scope run
// in the push-to-main unit lanes (and nightly under the workspace feature
// set); with one, in the push's lib row.
{
  const plan = planFor(["crates/meerkat-mcp/src/lib.rs"]);
  assertLanes(plan, "mcp touch");
  const row = plan.closure_test_rows.find((candidate) => candidate.binaries.includes("meerkat-mcp::form_elicitation"));
  assert.ok(row, "a row runs meerkat-mcp's form_elicitation");
  assert.ok(row.binaries.includes("meerkat-mcp::lifecycle_contract"), "and lifecycle_contract");
  assert.equal(row.package_flags, selectionFlags(CORE), "under Core's whole-workspace resolution, which enables test-support");
  assert.equal(
    checkHookCoverage(plan).map["meerkat-mob::lib::meerkat_mob"],
    "unit lanes (main, nightly): workspace unit lane",
    "without a lib-row runner, the unit lanes run meerkat-mob's lib tests",
  );
  assert.ok(
    plan.main_unit_shards.some((shard) => shard.packages.includes("meerkat-mcp")) &&
      plan.main_archive_builds.some((build) => build.package_flags === "-p meerkat-mob"),
    "the push-to-main unit lanes hold the scope's lib tests",
  );
  const withRunner = planFor(["crates/meerkat-mcp/src/lib.rs"], ["--lib-row-runner", "ubuntu-24.04"]);
  assertLanes(withRunner, "mcp touch, lib-row runner");
  assert.ok(
    withRunner.closure_test_rows.some((candidate) => candidate.lane === "lib" && candidate.packages.includes("meerkat-mob")),
    "with one, meerkat-mob's lib tests run in the push's lib row",
  );
}

// The hook's named lanes keep the hook's own flags.
{
  const plan = planFor(["crates/meerkat-mob/src/lib.rs"]);
  assertLanes(plan, "mob touch");
  const pick = (lane) => {
    const row = plan.closure_test_rows.find((candidate) => candidate.lane === lane);
    assert.ok(row, `the mob plan has a ${lane} row`);
    return { packages: row.package_flags, tests: row.test_flags, profile: row.profile, filter: row.filter, no_tests: row.no_tests };
  };
  assert.deepEqual(pick("headcanonical"), {
    packages: "-p meerkat-mob --features meerkat-mob/test-support",
    tests: "--test cold_restart_mob_resume",
    profile: "ci-integration",
    filter: "binary(cold_restart_mob_resume)",
    no_tests: "fail",
  });
  assert.deepEqual(pick("e2e-fast"), {
    packages: selectionFlags(CORE),
    tests: "--test e2e_fast_lane",
    profile: "ci-unit",
    filter: "binary(e2e_fast_lane)",
    no_tests: "fail",
  });
}

// Readers of non-Rust inputs: a docs-only change has no Cargo lanes but runs
// the tests that read docs/ (and the whole-checkout audits), with the
// workspace feature set, as the hook did on every push.
{
  const plan = planFor(["docs/index.mdx"]);
  assert.equal(plan.rust_changed, false);
  assert.deepEqual(plan.closure, []);
  for (const name of ["rkat", "meerkat-machine-schema", "xtask"]) {
    assert.ok(plan.reader_packages.includes(name), `docs/index.mdx selects its reader ${name}`);
  }
  assertHookParity(plan, "docs reader");
  assert.ok(plan.closure_test_rows.length > 0, "a docs change runs its readers' rows");
  const github = run(["--format", "github", "--", "docs/index.mdx"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  assert.equal(lines.closure_test_count, String(plan.closure_test_rows.length));
  // The rows of both waves (long rows first).
  const rowsOf = (key) => JSON.parse(lines[key]).include.filter((row) => row.name !== "none");
  assert.equal(rowsOf("closure_test_matrix").length + rowsOf("closure_test_rest_matrix").length, plan.closure_test_rows.length);
  assert.equal(Number(lines.closure_test_first_count) + Number(lines.closure_test_rest_count), plan.closure_test_rows.length);
  assert.equal(lines.hook_retained_lanes, "workspace unit lane", "the plan log names the lane it leaves without a row");
  assert.equal(lines.hook_retained_count, String(plan.hook_coverage.hook_retained));
}

// Reader rows: the exact targets that read a changed non-Rust input, where
// no other lane of the plan runs them.
{
  const outputs = (args) => {
    const result = run(["--format", "github", ...args]);
    assert.equal(result.status, 0, result.stderr);
    return Object.fromEntries(result.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  };
  const rowsOf = (plan) => plan.reader_rows.map((row) => `${row.name} | ${row.package_flags} | ${row.test_flags}`);
  // A docs edit on a pull request: the machine-authority docs alignment
  // test and xtask's static lanes, filtered to the tests that read docs/,
  // and xtask's lib and bins (its library reads docs/: the
  // authority-language gate, whose live-docs test is a library test). The
  // package's test targets, like every test target a pull request's unit
  // lanes leave, are the push's closure-tests rows' and named; so is rkat's
  // doc-example test, which compiles meerkat-mob.
  const staticLanes =
    "reader xtask:test:buildbuddy_static_lanes | -p xtask | --test buildbuddy_static_lanes -E test(/(^|::)(deprecated_backend_references_stay_rejected_only|generated_header_truthfulness_is_clean|retired_public_surface_tokens_stay_out_of_unowned_paths|retired_session_control_names_are_absent)$/)";
  const pullRequest = planFor(["docs/index.mdx"], ["--event", "pull_request"]);
  assert.deepEqual(rowsOf(pullRequest), [
    "reader machine-schema:test:docs_machine_authority_alignment | -p meerkat-machine-schema | --test docs_machine_authority_alignment -E test(/(^|::)(doctrine_count_claims_match_canonical_registry)$/)",
    "reader xtask:package | -p xtask | --lib --bins",
    staticLanes,
  ]);
  assert.deepEqual(pullRequest.reader_rows.map((row) => row.reads), [["docs/index.mdx"], ["docs/index.mdx"], ["docs/index.mdx"]]);
  assert.deepEqual(pullRequest.deferred_to_push.reader_rows, ["rkat:test:doc_examples_fixture", "xtask:package test targets"]);
  const prLines = outputs(["--event", "pull_request", "--", "docs/index.mdx"]);
  assert.equal(prLines.integration_count, "3", "the integration lane runs the reader rows");
  assert.deepEqual(JSON.parse(prLines.integration_matrix).include.map((row) => [row.name, row.packages, row.tests]), [
    [
      "reader machine-schema:test:docs_machine_authority_alignment",
      "-p meerkat-machine-schema",
      "--test docs_machine_authority_alignment -E test(/(^|::)(doctrine_count_claims_match_canonical_registry)$/)",
    ],
    ["reader xtask:package", "-p xtask", "--lib --bins"],
    staticLanes.split(" | "),
  ]);
  assert.match(prLines.deferred_to_push, /(^|; )reader rows rkat:test:doc_examples_fixture, xtask:package test targets($|;)/, "the gate prints the readers left to the push");
  // Every push plans xtask's machine-authority contracts, a whole-checkout
  // audit whose test target needs a feature the closure-tests rows do not
  // build, unless an xtask integration suite with that feature runs.
  const machineContracts = "reader xtask:test:machines_contracts | -p xtask --features machine-authority | --test machines_contracts -E test(/(^|::)(canonical_machine_inventory_matches_docs_and_artifact_bundle|live_flow_runtime_reducer_transitions_are_mob_machine_command_gated|live_mob_runtime_catalog_commands_fail_close_on_matching_machine_inputs)$/)";
  // The docs-only push: its closure-tests rows run the readers' test
  // targets they build, rkat's doc-example test among them; with no main
  // unit lane, reader rows run the lib and bin targets.
  const docsPush = planFor(["docs/index.mdx"]);
  assert.deepEqual(rowsOf(docsPush), ["reader xtask:package | -p xtask | --lib --bins", machineContracts]);
  assert.ok(
    docsPush.closure_test_rows.some((row) => row.binaries.includes("rkat::doc_examples_fixture")),
    "the docs-only push runs rkat's doc-example test in its closure-tests rows",
  );
  // A Rust-relevant push runs every package's lib and bins in main unit.
  assert.deepEqual(rowsOf(planFor(["docs/index.mdx", "crates/meerkat-core/src/lib.rs"])), [machineContracts]);
  assert.deepEqual(planFor(["crates/xtask/src/lib.rs"]).reader_rows, [], "xtask's own suite runs its contracts");
  // A pull request that runs xtask's unit lane and integration suite plans
  // no xtask row; one whose suite runs xtask's test targets keeps the lib
  // and bins.
  assert.ok(
    !planFor(["docs/index.mdx", "crates/xtask/src/lib.rs"], ["--event", "pull_request"]).reader_rows.some((row) => row.packages[0] === "xtask"),
    "xtask's own lanes run its readers",
  );
  const sdk = planFor(["sdks/python/meerkat/generated/types.py"], ["--event", "pull_request"]);
  assert.deepEqual(rowsOf(sdk), ["reader xtask:package | -p xtask | --lib --bins"]);
  assert.deepEqual(sdk.deferred_to_push.reader_rows, ["integration-tests:package"]);
  // A CHANGELOG edit runs xtask's static lanes, filtered to their readers.
  assert.deepEqual(rowsOf(planFor(["CHANGELOG.md"], ["--event", "pull_request"])), [
    "reader xtask:test:buildbuddy_static_lanes | -p xtask | --test buildbuddy_static_lanes -E test(/(^|::)(deprecated_backend_references_stay_rejected_only|generated_header_truthfulness_is_clean|retired_public_surface_tokens_stay_out_of_unowned_paths)$/)",
  ]);
  // A path only the whole-checkout readers select (live audits): a pull
  // request leaves them to the push, whose closure-tests rows run the test
  // targets they build, and a reader row the one they do not.
  const license = planFor(["LICENSE-MIT"], ["--event", "pull_request"]);
  assert.deepEqual([license.reader_rows, license.deferred_to_push.reader_rows], [[], []]);
  assert.deepEqual(rowsOf(planFor(["LICENSE-MIT"])), [machineContracts]);
  // That row runs in the integration lane, which waits for the stagger job
  // (the rest wave), and in no first-wave matrix: it never takes a runner
  // before the push's long rows.
  const licenseLines = outputs(["--", "LICENSE-MIT"]);
  assert.deepEqual(JSON.parse(licenseLines.integration_matrix).include.map((row) => row.name), ["reader xtask:test:machines_contracts"]);
  for (const key of ["shard_matrix", "main_unit_shard_matrix", "closure_test_matrix", "feature_check_matrix"]) {
    assert.doesNotMatch(licenseLines[key], /machines_contracts/, `${key} holds no reader row`);
  }
  assert.ok(
    jobLines(readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8"), "integration").includes("    needs: [changes, stagger]"),
    "the integration lane waits for the stagger job",
  );
  // A test target whose required features the closure-tests rows do not
  // build runs with them on the push; one that needs live-provider
  // credentials runs in no CI lane and is named, not deferred.
  const browser = "tests/live_smoke/browser/harness/gpt-live-peer-e2e.mjs";
  const browserPush = planFor([browser]);
  assert.deepEqual(rowsOf(browserPush), [
    "reader integration-tests:package | -p meerkat-integration-tests | --lib --bins",
    "reader integration-tests:test:gpt_live_replay | -p meerkat-integration-tests --features gpt-live-replay | --test gpt_live_replay",
    machineContracts,
  ]);
  assert.deepEqual(browserPush.reader_not_run, ["integration-tests:test:gpt_live_public_e2e"]);
  const browserPr = planFor([browser], ["--event", "pull_request"]);
  assert.deepEqual(
    [browserPr.reader_rows, browserPr.deferred_to_push.reader_rows, browserPr.reader_not_run],
    [[], ["integration-tests:package", "integration-tests:test:gpt_live_replay"], ["integration-tests:test:gpt_live_public_e2e"]],
  );
  const browserLines = outputs(["--event", "pull_request", "--", browser]);
  assert.equal(browserLines.reader_not_run, "integration-tests:test:gpt_live_public_e2e", "the gate names the readers no lane runs");
  assert.doesNotMatch(browserLines.deferred_to_push, /gpt_live_public_e2e/, "and does not call them deferred");
  // An in-crate doc the crate compiles in selects the crate like its code
  // (no reader row: the crate's own lanes run); other in-crate docs select
  // nothing.
  for (const event of ["pull_request", "push"]) {
    // Only the push's whole-checkout reader row (above) remains.
    const wholeCheckout = event === "push" ? [machineContracts] : [];
    const skill = planFor(["crates/meerkat-hooks/skills/hook-authoring/SKILL.md"], ["--event", event]);
    assert.deepEqual([skill.mode, skill.packages, rowsOf(skill)], ["packages", ["meerkat-hooks"], wholeCheckout], `${event}: a compiled-in skill`);
    for (const doc of ["crates/meerkat-core/README.md", "crates/meerkat-mob-mcp/LIVE_DELEGATION.md"]) {
      const plan = planFor([doc], ["--event", event]);
      assert.deepEqual([plan.mode, rowsOf(plan)], ["none", wholeCheckout], `${event}: ${doc}`);
    }
  }
}

// Core's whole-workspace lanes select Core's members only, each product's
// lanes that product's packages: a bare `--workspace` row would compile
// Core's crates with the products' feature requests, and a product's test
// binary counts only in a row with its own product's selection.
{
  const plan = planFor(["crates/meerkat-mob-mcp/src/lib.rs"], ["--event", "push"]);
  const baseline = checkHookCoverage(plan);
  assert.deepEqual([baseline.missing, baseline.errors], [[], []], "the planned rows cover the hook and the products");
  assert.ok(baseline.product_binaries > 0, "the push's scope reaches the products' test binaries");
  for (const product of PRODUCTS.filter((entry) => entry.name !== CORE)) {
    const rows = plan.closure_test_rows.filter((row) => row.product === product.name);
    assert.ok(rows.length > 0, `${product.name}'s test binaries run in rows of their own`);
    for (const row of rows) {
      assert.equal(row.package_flags, selectionFlags(product.name), `${row.name} selects exactly ${product.name}'s packages`);
    }
  }
  const mutate = (change) => {
    const copy = JSON.parse(JSON.stringify(plan));
    change(copy);
    return checkHookCoverage(copy);
  };
  const coreRow = (copy) => copy.closure_test_rows.find((row) => row.product === CORE && row.lane === "tests");
  const bare = mutate((copy) => {
    coreRow(copy).package_flags = "--workspace";
  });
  assert.ok(
    bare.errors.some((error) => error.includes("must exclude exactly the other products' packages")),
    "a bare --workspace row is refused",
  );
  const partial = mutate((copy) => {
    coreRow(copy).package_flags = selectionFlags(CORE).split(" --exclude ").slice(0, -1).join(" --exclude ");
  });
  assert.ok(
    partial.errors.some((error) => error.includes("must exclude exactly the other products' packages")),
    "a Core row that leaves one product package in is refused",
  );
  const metaRow = (copy) => copy.closure_test_rows.find((row) => row.product === "meta");
  const metaKey = (() => {
    const [binary] = metaRow(plan).binaries;
    const [name, test] = binary.split("::");
    return `${name}::test::${test}`;
  })();
  const borrowed = mutate((copy) => {
    metaRow(copy).package_flags = selectionFlags(CORE);
  });
  assert.ok(
    borrowed.missing.includes(metaKey) || borrowed.errors.some((error) => error.includes(metaRow(plan).name)),
    "a product's binary does not count in a row with Core's selection",
  );
}

// The coverage checker on mutated plans: each mutation is a way a row change
// could narrow what CI runs, and each must be reported. The plan has the lib
// row (a runner named for it), so the lib mutations have a row to change.
{
  const plan = planFor(["crates/meerkat-mcp/src/lib.rs"], ["--lib-row-runner", "ubuntu-24.04"]);
  const baseline = checkHookCoverage(plan);
  assert.deepEqual([baseline.missing, baseline.errors], [[], []], "the planned rows cover the hook");
  const mutate = (change) => {
    const copy = JSON.parse(JSON.stringify(plan));
    change(copy);
    return checkHookCoverage(copy);
  };
  const mcpRow = (copy) => copy.closure_test_rows.find((row) => row.binaries.includes("meerkat-mcp::form_elicitation"));
  // A dropped row leaves its binaries uncovered.
  const dropped = mutate((copy) => {
    copy.closure_test_rows = copy.closure_test_rows.filter((row) => row !== mcpRow(copy));
  });
  assert.ok(dropped.missing.includes("meerkat-mcp::test::form_elicitation"), "a dropped row is a gap");
  // A package-scoped row is not the hook's build: Cargo refuses the
  // required-features targets, and nothing in the row counts.
  const scoped = mutate((copy) => {
    mcpRow(copy).package_flags = "-p meerkat-mcp";
  });
  assert.ok(scoped.errors.some((error) => error.includes("form_elicitation requires test-support")), "Cargo would refuse the row");
  assert.ok(scoped.missing.includes("meerkat-mcp::test::account_selection"), "the scoped set does not cover the hook");
  // A package-scoped lib row is a gap even with the workspace's own
  // features: its dependencies are not the hook's.
  const libScoped = mutate((copy) => {
    const row = copy.closure_test_rows.find((candidate) => candidate.lane === "lib");
    row.package_flags = "-p meerkat-mob --features meerkat-mob/runtime-adapter,meerkat-mob/schema,meerkat-mob/test-support";
    delete row.filter;
  });
  assert.ok(libScoped.missing.includes("meerkat-mob::lib::meerkat_mob"), "meerkat-mob's lib under its scoped set is a gap");
  // HeadCanonical under the workspace set is not the hook's HeadCanonical.
  const headCanonical = mutate((copy) => {
    const row = copy.closure_test_rows.find((candidate) => candidate.lane === "headcanonical");
    row.package_flags = "-p meerkat-mob --features meerkat-mob/test-support,meerkat-mob/schema";
  });
  assert.ok(headCanonical.missing.includes("HeadCanonical::meerkat-mob::cold_restart_mob_resume"), "HeadCanonical keeps the hook's flags");
  // A profile whose default-filter drops a binary does not run it.
  const filtered = mutate((copy) => {
    copy.closure_test_rows.find((candidate) => candidate.lane === "e2e-fast").profile = "ci-integration";
  });
  assert.ok(filtered.missing.includes("meerkat-integration-tests::test::e2e_fast_lane"), "ci-integration drops e2e_fast_lane");
  // A row's binary list is what its job checks nextest against: it must be
  // exactly what the row runs.
  const overlisted = mutate((copy) => {
    copy.closure_test_rows.find((row) => row.lane === "lib").binaries.push("meerkat-machine-derive");
  });
  assert.ok(overlisted.errors.some((error) => error.includes("lists binaries it does not run (meerkat-machine-derive)")), "a row cannot list a binary it does not run");
  const underlisted = mutate((copy) => {
    const row = copy.closure_test_rows.find((candidate) => candidate.lane === "lib");
    delete row.filter;
  });
  assert.ok(underlisted.errors.some((error) => error.includes("runs binaries it does not list")), "a row cannot run binaries it does not list");
  // A narrower scope than the closure is caught by the selftest's own
  // test-scope assertion (the checker trusts plan.test_scope).
  assert.throws(() => assertHookParity({ ...plan, test_scope: plan.packages }, "narrowed scope"));
  // Every row-executing step is bound: with no workflow, each one is missing.
  const unbound = checkHookCoverage(plan, { workflowText: "" });
  assert.equal(unbound.errors.length, rowStepBindings().length, "every row-executing step is bound");
  assert.deepEqual(rowStepBindings().map((spec) => spec.job), ["closure-tests", "closure-tests-rest"], "both closure-tests waves are bound");
}

// What a plan may leave without a row: the lib binaries of a lane of the
// lane script, nothing else; and a row runs where its bound job runs.
{
  const plan = planFor(["crates/meerkat-mcp/src/lib.rs"]);
  const baseline = checkHookCoverage(plan);
  assert.deepEqual([baseline.missing, baseline.errors], [[], []], "the rows and the kind(lib) lane cover the lane script");
  assert.ok(baseline.hook_retained > 0, "the plan leaves lib binaries to the unit lanes");
  const mutate = (change) => {
    const copy = JSON.parse(JSON.stringify(plan));
    change(copy);
    return checkHookCoverage(copy);
  };
  const unretained = mutate((copy) => {
    copy.hook_retained_lanes = [];
  });
  assert.ok(unretained.missing.includes("meerkat-mcp::lib::meerkat_mcp"), "a lib binary that neither runs is a gap");
  // A retained lane exempts lib binaries only: leaving the integration lane
  // without rows exempts none of its tests, and the lib binaries lose their
  // lane.
  const integrationKept = mutate((copy) => {
    copy.hook_retained_lanes = ["workspace integration lane"];
    copy.closure_test_rows = copy.closure_test_rows.filter((row) => !row.binaries.includes("meerkat-mcp::form_elicitation"));
  });
  assert.ok(integrationKept.missing.includes("meerkat-mcp::test::form_elicitation"), "the integration lane's tests still need rows");
  assert.ok(integrationKept.missing.includes("meerkat-mcp::lib::meerkat_mcp"), "and the lib binaries their own lane");
  const unknownLane = mutate((copy) => {
    copy.hook_retained_lanes = ["workspace lib lane"];
  });
  assert.ok(
    unknownLane.errors.some((error) => error.includes("leaves \"workspace lib lane\" without a row, but the lane script has no such workspace lane")),
    "a lane the lane script does not have is refused",
  );
  assert.ok(unknownLane.missing.includes("meerkat-mcp::lib::meerkat_mcp"), "and exempts nothing");
  const elsewhere = mutate((copy) => {
    copy.closure_test_rows[0].runner = "r1-c3d-30";
  });
  assert.ok(
    elsewhere.errors.some((error) => error.includes("names runner r1-c3d-30, but job closure-tests runs on ubuntu-24.04")),
    "a row cannot name a runner its job does not run on",
  );
  // The planner fails the same way for a lib-row runner no bound job runs on,
  // and refuses an empty label.
  const r1 = run(["--format", "json", "--lib-row-runner", "r1-c3d-30", "--", "crates/meerkat-mcp/src/lib.rs"]);
  assert.equal(r1.status, 1, "a lib row on an unbound runner fails the plan");
  assert.match(r1.stderr, /row lib names runner r1-c3d-30, but job closure-tests runs on ubuntu-24\.04/);
  const empty = run(["--format", "json", "--lib-row-runner", "", "--", "crates/meerkat-mcp/src/lib.rs"]);
  assert.equal(empty.status, 1, "an empty lib-row runner is an error");
  assert.match(empty.stderr, /--lib-row-runner requires a runner label/);
}

// The bound steps: what a closure-tests row lists is what it runs, and no
// other nextest command, selection or missing binary check can hide in the
// steps the coverage model reads. Each mutation edits the real ci.yml text.
{
  const workflow = readFileSync(resolve(root, ".github/workflows/ci.yml"), "utf8");
  assert.deepEqual(bindRowSteps(workflow), [], "the real workflow binds");
  // closure-tests and its rest-wave copy (long rows first) are bound the
  // same way: an anchor in their shared text occurs once in each, the
  // first in closure-tests, and the same mutation of the copy is refused too.
  const jobs = rowStepBindings().map((spec) => spec.job);
  const mutated = (from, to) => {
    const parts = workflow.split(from);
    assert.ok(parts.length === 2 || parts.length === jobs.length + 1, `mutation anchor is unique or once per bound job: ${from}`);
    const mutate = (index) => bindRowSteps(parts.slice(0, index + 1).join(from) + to + parts.slice(index + 1).join(from));
    if (parts.length === jobs.length + 1) {
      for (let index = 1; index < jobs.length; index += 1) {
        assert.ok(mutate(index).some((error) => error.includes(`job ${jobs[index]}:`)), `the same mutation of ${jobs[index]} is refused: ${from}`);
      }
    }
    return mutate(0);
  };
  const run = '          ./scripts/repo-cargo nextest run "${selection[@]}" --no-tests="${NO_TESTS}" --no-fail-fast --status-level slow --final-status-level slow\n';
  const runOnly = mutated(run, run.replace('"${selection[@]}"', `"\${selection[@]}" -E 'none()'`));
  assert.ok(runOnly.some((error) => error.includes("job closure-tests: the step runs")), "a run-only selection change is refused");
  const ownSelection = mutated(run, run.replace('"${selection[@]}"', '${PACKAGES} "${test_flags[@]}" --profile "${PROFILE}"'));
  assert.ok(ownSelection.some((error) => error.includes("job closure-tests: the step runs")), "a run that does not use the listed selection is refused");
  const narrowed = mutated(
    '          selection=(${PACKAGES} "${test_flags[@]}" --profile "${PROFILE}" "${filter_args[@]}" "${partition_args[@]}")\n',
    '          selection=(${PACKAGES} "${test_flags[@]}" --profile "${PROFILE}" "${filter_args[@]}" "${partition_args[@]}")\n          [[ "${NO_TESTS}" == warn ]] && filter_args=(-E "none()")\n',
  );
  assert.ok(narrowed.some((error) => error.includes("job closure-tests: the step runs")), "a later selection change is refused");
  const unchecked = mutated('          node scripts/ci-verify-row-binaries.mjs "${RUNNER_TEMP}/closure-test-list.json" "${partition_args[@]}" ${BINARIES}\n', "");
  assert.ok(unchecked.some((error) => error.includes("job closure-tests: the step runs")), "a step without the binary check is refused");
  // A partitioned row's check must know its partition, and its partition
  // cannot be changed or dropped after the input mapping.
  const unpartitionedCheck = mutated(
    '          node scripts/ci-verify-row-binaries.mjs "${RUNNER_TEMP}/closure-test-list.json" "${partition_args[@]}" ${BINARIES}\n',
    '          node scripts/ci-verify-row-binaries.mjs "${RUNNER_TEMP}/closure-test-list.json" --partition count:1/1 ${BINARIES}\n',
  );
  assert.ok(unpartitionedCheck.some((error) => error.includes("job closure-tests: the step runs")), "an altered binary check partition is refused");
  const partitionLine = '          if [[ -n "${PARTITION}" ]]; then partition_args=(--partition "${PARTITION}"); fi\n';
  const repartitioned = mutated(partitionLine, partitionLine.replace('--partition "${PARTITION}"', "--partition count:1/2"));
  assert.ok(repartitioned.some((error) => error.includes("job closure-tests: the step runs")), "an altered partition is refused");
  const unpartitioned = mutated(partitionLine, "");
  assert.ok(unpartitioned.some((error) => error.includes("job closure-tests: the step runs")), "a dropped partition is refused");
  const partitionMapping = "          MIN_STACK: ${{ matrix.rust_min_stack }}\n          PARTITION: ${{ matrix.partition }}\n";
  const partitionRemapped = mutated(partitionMapping, "          MIN_STACK: ${{ matrix.rust_min_stack }}\n          PARTITION: count:1/2\n");
  assert.ok(partitionRemapped.some((error) => error.includes("step env PARTITION")), "an altered PARTITION mapping is refused");
  // An input reassigned before it is read leaves every pinned line intact:
  // the line that reassigns it names it, and is refused.
  const selectionLine = '          selection=(${PACKAGES} "${test_flags[@]}" --profile "${PROFILE}" "${filter_args[@]}" "${partition_args[@]}")\n';
  const repackaged = mutated(selectionLine, `          PACKAGES='-p meerkat-integration-tests'\n${selectionLine}`);
  assert.ok(repackaged.some((error) => error.includes("job closure-tests: the step runs")), "a PACKAGES reassignment is refused");
  const reprofiled = mutated(selectionLine, `          export PROFILE=ci-pr\n${selectionLine}`);
  assert.ok(reprofiled.some((error) => error.includes("job closure-tests: the step runs")), "a PROFILE export is refused");
  const listLine = '          ./scripts/repo-cargo nextest list "${selection[@]}" --message-format json >"${RUNNER_TEMP}/closure-test-list.json"\n';
  const reread = mutated(listLine, `          read -r NO_TESTS <<<"warn"\n${listLine}`);
  assert.ok(reread.some((error) => error.includes("job closure-tests: the step runs")), "a NO_TESTS read is refused");
  // The test-thread stack: its MIN_STACK mapping and the line that exports it
  // are pinned, and no later line can override RUST_MIN_STACK.
  const mapping = "          NO_TESTS: ${{ matrix.no_tests }}\n          MIN_STACK: ${{ matrix.rust_min_stack }}\n";
  const remapped = mutated(mapping, "          NO_TESTS: ${{ matrix.no_tests }}\n          MIN_STACK: 1048576\n");
  assert.ok(remapped.some((error) => error.includes("step env MIN_STACK")), "an altered MIN_STACK mapping is refused");
  const stackExport = '          # The hook runs every lane on an 8 MiB test-thread stack.\n          if [[ -n "${MIN_STACK}" ]]; then export RUST_MIN_STACK="${MIN_STACK}"; fi\n';
  const reexported = mutated(stackExport, stackExport.replace('RUST_MIN_STACK="${MIN_STACK}"', "RUST_MIN_STACK=1048576"));
  assert.ok(reexported.some((error) => error.includes("job closure-tests: the step runs")), "an altered RUST_MIN_STACK export is refused");
  const unexported = mutated(stackExport, "");
  assert.ok(unexported.some((error) => error.includes("job closure-tests: the step runs")), "a removed RUST_MIN_STACK export is refused");
  const overridden = mutated(listLine, `          export RUST_MIN_STACK=1048576\n${listLine}`);
  assert.ok(overridden.some((error) => error.includes("job closure-tests: the step runs")), "a later RUST_MIN_STACK override is refused");
  const moved = mutated("      - name: Closure tests (the row's targets, workspace feature set)\n", "      - name: Closure tests (moved)\n");
  assert.ok(moved.some((error) => error.includes("job closure-tests: step")), "the rows' step is bound by name");
  const wrongMatrix = mutated(
    "      matrix: ${{ fromJSON(needs.changes.outputs.closure_test_matrix) }}\n",
    "      matrix: ${{ fromJSON(needs.changes.outputs.integration_matrix) }}\n",
  );
  assert.ok(wrongMatrix.some((error) => error.includes("its matrix is not")), "the closure-tests job runs the closure matrix");
  const wrongRestMatrix = mutated(
    "      matrix: ${{ fromJSON(needs.changes.outputs.closure_test_rest_matrix) }}\n",
    "      matrix: ${{ fromJSON(needs.changes.outputs.closure_test_matrix) }}\n",
  );
  assert.ok(wrongRestMatrix.some((error) => error.includes("job closure-tests-rest: its matrix is not")), "the rest wave runs the rest matrix");
  const otherRunner = mutated(
    "    if: ${{ needs.changes.result == 'success' && needs.changes.outputs.closure_test_first_count != '0' }}\n    runs-on: ubuntu-24.04\n",
    "    if: ${{ needs.changes.result == 'success' && needs.changes.outputs.closure_test_first_count != '0' }}\n    runs-on: ubuntu-22.04\n",
  );
  assert.ok(otherRunner.some((error) => error.includes("job closure-tests: it does not run on ubuntu-24.04")), "the job's runner is bound");
  const rewired = mutated(
    "          TESTS: ${{ matrix.tests }}\n          PROFILE: ${{ matrix.profile }}\n",
    "          TESTS: ${{ matrix.packages }}\n          PROFILE: ${{ matrix.profile }}\n",
  );
  assert.ok(rewired.some((error) => error.includes("step env TESTS")), "the step reads each selection input from its own matrix field");
  const envSelection = mutated(
    "          BINARIES: ${{ matrix.binaries }}\n",
    "          BINARIES: ${{ matrix.binaries }}\n          NEXTEST_RUN_IGNORED: only\n",
  );
  assert.ok(envSelection.some((error) => error.includes("sets a NEXTEST_ variable")), "a NEXTEST_ variable in a bound job is refused");
}

// The hook's selection is read from the hook script, not copied: a changed
// hook filter changes what CI must cover, and a shape the model cannot read
// fails closed.
{
  const hookText = readFileSync(resolve(root, HOOK_SCRIPT), "utf8");
  const hook = hookModel(hookText);
  assert.equal(hook.buildFilter, "kind(lib) or kind(test)");
  assert.deepEqual(hook.headCanonical, { package: "meerkat-mob", test: "cold_restart_mob_resume", features: ["test-support"], profile: "fast" });
  assert.equal(hook.e2eFast.binary, "e2e_fast_lane");
  assert.deepEqual(
    hook.lanes.map((lane) => [lane.label, lane.profile, lane.filter]),
    [
      ["workspace unit lane", "default", "kind(lib)"],
      ["workspace integration lane", "fast", "kind(test)"],
      ["e2e-fast lane", "default", "binary(e2e_fast_lane)"],
    ],
  );
  const plan = planFor(["Cargo.lock"]);
  assert.deepEqual(checkHookCoverage(plan, { hookText }).missing, []);
  const mutatedHook = (from, to) => {
    assert.equal(hookText.split(from).length, 2, `hook mutation anchor is unique: ${from}`);
    return hookText.replace(from, to);
  };
  // Widening only a lane's filter changes nothing (the build inventory lists
  // lib and test binaries); widening both makes bin tests hook coverage.
  const laneOnly = checkHookCoverage(plan, {
    hookText: mutatedHook("    -E 'kind(lib)' --no-tests=fail --no-fail-fast \\\n", "    -E 'kind(lib) or kind(bin)' --no-tests=fail --no-fail-fast \\\n"),
  });
  assert.deepEqual(laneOnly.missing, [], "a lane cannot run binaries the hook's build does not list");
  const widened = checkHookCoverage(plan, {
    hookText: mutatedHook("    -E 'kind(lib)' --no-tests=fail --no-fail-fast \\\n", "    -E 'kind(lib) or kind(bin)' --no-tests=fail --no-fail-fast \\\n")
      .replace("-E 'kind(lib) or kind(test)' >", "-E 'kind(lib) or kind(test) or kind(bin)' >"),
  });
  assert.ok(widened.missing.some((key) => key.includes("::bin::")), "a hook that also builds and runs bin tests needs rows for them");
  const otherLane = checkHookCoverage(plan, {
    hookText: mutatedHook("    -E 'binary(e2e_fast_lane)' --no-tests=fail --no-fail-fast \\\n", "    -E 'binary(e2e_system_lane)' --no-tests=fail --no-fail-fast \\\n"),
  });
  assert.ok(otherLane.missing.includes("meerkat-integration-tests::test::e2e_system_lane"), "a different e2e lane in the hook needs its row");
  assert.throws(
    () => checkHookCoverage(plan, { hookText: mutatedHook("    -E 'kind(lib)' --no-tests=fail --no-fail-fast \\\n", "    -E 'kind(lib) and not test(=slow)' --no-tests=fail --no-fail-fast \\\n") }),
    /cannot be decided per binary/,
    "a hook filter the model cannot decide per binary fails closed",
  );
  assert.throws(
    () => hookModel(mutatedHook("    -E 'kind(lib)' --no-tests=fail --no-fail-fast \\\n", "    -E 'kind(lib)' --no-tests=fail --no-fail-fast --run-ignored all \\\n")),
    /shape the coverage model does not read/,
    "a hook lane with an extra selection flag fails closed",
  );
}

// The matrices the closure-tests job and its rest wave run are the plan's
// rows, field by field, each row in its wave.
{
  const plan = planFor(["crates/meerkat-mob/src/lib.rs"]);
  const github = run(["--format", "github", "--", "crates/meerkat-mob/src/lib.rs"]);
  assert.equal(github.status, 0, github.stderr);
  const lines = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
  const inWave = (names) => plan.closure_test_rows.filter((row) => names.includes(row.name));
  const rowsOf = (key) => JSON.parse(lines[key]).include.filter((row) => row.name !== "none");
  assert.deepEqual(
    [...rowsOf("closure_test_matrix"), ...rowsOf("closure_test_rest_matrix")],
    [...inWave(plan.waves.closure_tests.first), ...inWave(plan.waves.closure_tests.rest)].map((row) => ({
      name: row.name,
      packages: row.package_flags,
      tests: row.test_flags,
      profile: row.profile,
      filter: row.filter ?? "",
      binaries: row.binaries.join(" "),
      no_tests: row.no_tests,
      rust_min_stack: String(row.rust_min_stack),
      partition: row.partition ?? "",
    })),
  );
}

// The filter subset and the profiles: ci-integration selects exactly fast's
// binaries; ci-pr and ci-unit select every binary (nextest's `inherits`
// does not carry a default-filter); a test() predicate cannot be decided per
// binary and is refused rather than guessed.
{
  const nextest = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  assert.equal(profileFilter(nextest, "ci-integration"), profileFilter(nextest, "fast"));
  assert.equal(profileFilter(nextest, "ci-pr"), "all()");
  assert.equal(profileFilter(nextest, "ci-unit"), "all()");
  const fast = parseFilter(profileFilter(nextest, "fast"));
  assert.equal(fast({ kind: "test", binary: "e2e_fast_lane", package: "meerkat-integration-tests" }), false);
  assert.equal(fast({ kind: "test", binary: "phase1_wiring", package: "meerkat-integration-tests" }), true);
  const both = parseFilter("kind(test) & (binary(=a) | package(b))");
  assert.equal(both({ kind: "test", binary: "a", package: "x" }), true);
  assert.equal(both({ kind: "lib", binary: "a", package: "x" }), false);
  assert.equal(both({ kind: "test", binary: "c", package: "b" }), true);
  assert.throws(() => parseFilter("test(=foo)"), /cannot be decided per binary/);
  assert.throws(() => parseFilter("binary(~e2e)"), /unsupported name matcher/);
  assert.throws(() => profileFilter(nextest, "no-such-profile"));
}

// featureClosure infers the features of a package-scoped build (HeadCanonical's
// own command, any package-scoped row the checker reads). That inference must
// never claim a feature Cargo does not enable in a package-scoped build.
{
  const metadata = readMetadata();
  const packages = workspacePackages(metadata);
  const unified = workspaceFeatures(metadata, packages);
  for (const pkg of packages) {
    const defaults = Object.hasOwn(pkg.features ?? {}, "default") ? ["default"] : [];
    const inferred = featureClosure(pkg, new Set(defaults));
    const tree = spawnSync(
      join(root, "scripts/repo-cargo"),
      ["tree", "-p", pkg.name, "--depth", "0", "-e", "normal,build,dev", "--prefix", "none", "-f", "{p}|{f}"],
      { cwd: root, encoding: "utf8" },
    );
    assert.equal(tree.status, 0, `cargo tree -p ${pkg.name}: ${tree.stderr}`);
    const first = tree.stdout.split("\n")[0];
    const actual = new Set(first.slice(first.indexOf("|") + 1).split(",").filter(Boolean));
    for (const feature of inferred) {
      assert.ok(actual.has(feature), `${pkg.name}: inferred default feature ${feature} is enabled by Cargo`);
      assert.ok(unified.get(pkg.name).has(feature), `${pkg.name}: ${feature} is in the workspace set`);
    }
  }
}

// The job's runtime check: nextest must list exactly the planned binaries,
// and no test of theirs may be filtered out by a profile.
{
  const verifier = resolve(here, "ci-verify-row-binaries.mjs");
  const scratch = mkdtempSync(join(tmpdir(), "ci-verify-row-binaries-"));
  const verify = (suites, expected) => {
    const listPath = join(scratch, "list.json");
    writeFileSync(listPath, JSON.stringify({ "rust-suites": suites }));
    return spawnSync("node", [verifier, listPath, ...expected], { encoding: "utf8" });
  };
  const suite = (testcases, status = "listed") => ({ status, testcases });
  const match = { "filter-match": { status: "matches" } };
  const ignored = { "filter-match": { status: "mismatch", reason: "ignored" } };
  const defaultFiltered = { "filter-match": { status: "mismatch", reason: "default-filter" } };
  try {
    assert.equal(verify({ "p::a": suite({ t1: match, t2: ignored }), p: suite({ t3: match }) }, ["p::a", "p"]).status, 0);
    assert.equal(verify({ "p::a": suite({ t1: match }) }, ["p::a", "p::b"]).status, 1, "a missing binary fails");
    assert.equal(verify({ "p::a": suite({ t1: match }), "q::a": suite({ t1: match }) }, ["p::a"]).status, 1, "an extra binary fails");
    assert.equal(
      verify({ "p::a": suite({ t1: match }), q: suite({ t1: { "filter-match": { status: "mismatch", reason: "expression" } } }), r: suite({}, "skipped") }, ["p::a"]).status,
      0,
      "a listed binary none of whose tests are selected (another member's lib under --workspace) passes",
    );
    assert.equal(verify({ "p::a": suite({ t1: defaultFiltered }) }, ["p::a"]).status, 1, "a profile-filtered test fails");
    assert.equal(verify({ "p::a": suite({}, "skipped") }, ["p::a"]).status, 1, "a skipped binary fails");
    // A partitioned row leaves its other partitions' tests to its sibling
    // rows, and only a row that names its partition may.
    const otherPartition = { "filter-match": { status: "mismatch", reason: "partition" } };
    const partitioned = verify({ "p::a": suite({ t1: match, t2: otherPartition }) }, ["--partition", "count:1/2", "p::a"]);
    assert.equal(partitioned.status, 0, partitioned.stderr);
    assert.match(partitioned.stdout, /1 in the other partitions of count:1\/2/);
    assert.equal(verify({ "p::a": suite({ t1: match, t2: otherPartition }) }, ["p::a"]).status, 1, "an unnamed partition fails");
    assert.equal(
      verify({ "p::a": suite({ t1: otherPartition }) }, ["--partition", "count:2/2", "p::a"]).status,
      1,
      "a partition that selects nothing fails",
    );
    assert.equal(
      verify({ "p::a": suite({ t1: match, t2: defaultFiltered }) }, ["--partition", "count:1/2", "p::a"]).status,
      1,
      "a partitioned row still fails a profile-filtered test",
    );
    assert.equal(verify({ "p::a": suite({ t1: match }) }, ["--partition", "hash:1/2", "p::a"]).status, 2, "only count partitions");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// Products (ci/products.json). A change inside a product's root plans that
// product's rows and its dependents' and none of Core's: no Core package,
// no Core guard lane, feature suite or whole-workspace unit sweep, no
// whole-checkout reader. Rows never mix products. A product with a changed
// package gets its published-floor check; a Core change reaching a product
// only through the closure gives that product a downstream type-check row,
// or leaves it to the push when meerkat-mob's build is invalidated.
{
  const productOf = (plan, name) => plan.package_model[name].product;
  const productNames = PRODUCTS.map((product) => product.name).filter((name) => name !== CORE);
  const members = (plan, product) => Object.keys(plan.package_model).filter((name) => productOf(plan, name) === product);
  const assertNoCore = (plan, label) => {
    for (const name of [...plan.packages, ...plan.closure, ...plan.test_scope]) {
      assert.notEqual(productOf(plan, name), CORE, `${label}: ${name} is Core's`);
    }
    for (const shard of [...plan.shards, ...plan.main_unit_shards, ...plan.main_feature_unit_shards]) {
      assert.notEqual(shard.product, CORE, `${label}: ${shard.name} is a Core row`);
    }
    assert.deepEqual(plan.main_archive_builds, [], `${label}: no Core archive build`);
    assert.equal(plan.core_changed, false, `${label}: no Core lane`);
    assert.equal(plan.governed_jsonl, false, `${label}: no governed JSONL`);
    assert.equal(plan.mcp_confinement_positive, false, `${label}: no confinement lane`);
    assert.deepEqual(plan.feature_check_jobs, [], `${label}: no Core feature check`);
    assert.equal(plan.wasm, false, `${label}: no wasm check`);
    assert.equal(plan.sdk_host, false, `${label}: no Core SDK suite`);
  };
  const assertRowsSingleProduct = (plan, label) => {
    for (const row of [...plan.shards, ...plan.unit_shards, ...plan.main_unit_shards, ...plan.closure_test_rows]) {
      const products = new Set(row.packages.map((name) => productOf(plan, name)));
      assert.equal(products.size, 1, `${label}: ${row.name} mixes products ${[...products].join(", ")}`);
      assert.equal(row.product, [...products][0], `${label}: ${row.name} names its product`);
    }
  };
  const workspacePlan = planFor(["Cargo.toml"]);
  for (const product of productNames) {
    const owned = members(workspacePlan, product);
    if (owned.length === 0) continue;
    // The product's package with the widest build closure.
    const [pkg] = [...owned].sort(
      (a, b) => workspacePlan.package_model[b].dependency_closure - workspacePlan.package_model[a].dependency_closure || a.localeCompare(b),
    );
    const productRoot = PRODUCTS.find((entry) => entry.name === product).root;
    const libPath = `${productRoot}/crates/${pkg}/src/lib.rs`;
    const changed = existsSync(resolve(root, libPath)) ? libPath : `${productRoot}/crates/${pkg}/src/main.rs`;
    for (const event of ["pull_request", "push"]) {
      const plan = planFor([changed], ["--event", event]);
      const label = `${product} ${event} (${changed})`;
      assert.equal(plan.rust_changed, true, `${label}: a product Rust change is build-relevant`);
      assert.equal(plan.mode, "packages", `${label}: never the whole workspace`);
      assertNoCore(plan, label);
      assertRowsSingleProduct(plan, label);
      assert.deepEqual(plan.products.changed, [product], `${label}: the product is named`);
      assert.ok(plan.products.floor_checks.includes(product), `${label}: the product's floor check runs`);
      assert.deepEqual(plan.reader_rows, [], `${label}: no Core reader row`);
      if (event === "push") {
        assert.deepEqual(
          unitLanesOf(plan.main_unit_shards, label).flatMap((shard) => shard.packages).sort(),
          [...plan.closure].sort(),
          `${label}: the main unit sweep covers the product's closure only`,
        );
        // The feature suites of the closure's packages, and only those, each
        // on its product's test stack.
        assert.deepEqual(
          plan.main_feature_unit_shards.map((row) => row.package_flags).sort(),
          FEATURE_UNIT_SUITES.filter((suite) => plan.closure.includes(suite.package))
            .map((suite) => featureSuiteFlags(suite)).sort(),
          `${label}: the feature suites of the product's closure`,
        );
        for (const row of plan.main_feature_unit_shards) {
          const owner = productOf(plan, row.packages[0]);
          assert.equal(row.product, owner, `${label}: ${row.name} names its product`);
          const stack = PRODUCTS.find((entry) => entry.name === owner)?.testStack ?? null;
          if (stack) assert.equal(row.rust_min_stack, stack, `${label}: ${row.name} runs on ${owner}'s test stack`);
        }
        // A binary no hosted runner can judge gets no row and is named.
        for (const { binary } of HOSTED_RUNNER_EXCLUDED_BINARIES) {
          const inScope = plan.test_scope.includes(binary.split("::")[0]);
          assert.equal(
            plan.hosted_runner_excluded.some((entry) => entry.binary === binary),
            inScope,
            `${label}: ${binary} is named exactly when its package is in the test scope`,
          );
          assert.ok(
            !plan.closure_test_rows.some((row) => row.binaries.includes(binary)),
            `${label}: no closure-tests row runs ${binary}`,
          );
        }
        assert.equal(plan.deferred_to_push, null);
      } else {
        assert.equal(plan.deferred_to_push.governed_jsonl, false, `${label}: leaves no Core guard lane to the push`);
        assert.equal(plan.deferred_to_push.mcp_confinement_positive, false, `${label}: leaves no Core guard lane to the push`);
      }
    }
  }
  // An unowned Rust path inside a product root (an example outside the crate
  // directories) selects that product's packages, not the whole workspace.
  for (const product of PRODUCTS.filter((entry) => entry.name !== CORE)) {
    const plan = planFor([`${product.root}/examples/unowned-selftest/main.rs`], ["--event", "pull_request"]);
    const owned = members(plan, product.name);
    if (owned.length === 0) continue;
    assert.equal(plan.mode, "packages", `${product.name}: an unowned product Rust path stays inside the product`);
    assert.deepEqual(plan.packages, owned.sort(), `${product.name}: every package of the product`);
    assert.match(plan.products.escalations[product.name], /Rust path outside any /);
    assertNoCore(plan, `${product.name} unowned path`);
    // The product's own build configuration does the same.
    const config = planFor([`${product.root}/clippy.toml`], ["--event", "pull_request"]);
    assert.deepEqual(config.packages, owned.sort(), `${product.name}: its clippy.toml selects its packages`);
    assertNoCore(config, `${product.name} clippy.toml`);
  }
  // A non-Rust product path plans no Rust lane; Connect's selects its checks.
  for (const product of PRODUCTS.filter((entry) => entry.name !== CORE)) {
    const plan = planFor([`${product.root}/docs/selftest-only.md`], ["--event", "pull_request"]);
    assert.equal(plan.rust_changed, false, `${product.name} docs: no Rust lane`);
    assert.deepEqual(plan.products.changed, [product.name]);
    assert.deepEqual(plan.reader_packages, [], `${product.name} docs: the whole-checkout readers leave product roots alone`);
    assert.deepEqual(plan.products.lanes, product.name === "connect" ? ["connect_checks"] : [], `${product.name} docs lanes`);
    assert.deepEqual(plan.products.floor_checks, [], `${product.name} docs: no floor check`);
  }
  // A Core change outside meerkat-mob's build that reaches a product gives
  // it one downstream type-check row (every target); one that invalidates
  // meerkat-mob's build leaves it to the push; neither runs a floor check.
  {
    const outside = planFor(["crates/meerkat-mob-mcp/src/lib.rs"], ["--event", "pull_request"]);
    const downstream = outside.shards.filter((shard) => shard.downstream);
    const reached = productNames.filter((product) => outside.closure.some((name) => productOf(outside, name) === product));
    assert.ok(reached.length > 0, "meerkat-mob-mcp reaches a product");
    assert.deepEqual(downstream.map((shard) => shard.product).sort(), [...reached].sort(), "one downstream row per reached product");
    for (const shard of downstream) {
      assert.equal(shard.targets, "--all-targets", `${shard.name} lints every target`);
      assert.match(shard.name, /^downstream /);
    }
    assert.deepEqual(outside.products.floor_checks, [], "published Core did not change: no floor check");
    assertRowsSingleProduct(outside, "mob-mcp pull request");
    const invalidating = planFor(["crates/meerkat-core/src/lib.rs"], ["--event", "pull_request"]);
    assert.deepEqual(invalidating.shards.filter((shard) => shard.downstream), [], "no downstream row when meerkat-mob's build is invalidated");
    assert.deepEqual(invalidating.deferred_to_push.downstream_type_check.sort(), [...reached].sort(), "the push type-checks them");
    assert.match(
      run(["--format", "github", "--event", "pull_request", "--", "crates/meerkat-core/src/lib.rs"]).stdout,
      /^deferred_to_push=.*the downstream type-check of .* \(mob chain invalidated\)$/m,
    );
    const push = planFor(["crates/meerkat-core/src/lib.rs"], ["--event", "push"]);
    assert.equal(push.core_changed, true);
    for (const product of reached) {
      assert.ok(push.closure.some((name) => productOf(push, name) === product), `the push's closure holds ${product}`);
    }
    assertRowsSingleProduct(push, "meerkat-core push");
  }
  // The root manifest and lock feed every floor check; a global path that
  // changes no product's published dependencies (the Makefile) runs none.
  {
    const lock = planFor(["Cargo.toml"], ["--event", "pull_request"]);
    assert.deepEqual(lock.products.floor_checks, productNames.filter((product) => members(lock, product).length > 0));
    const make = planFor(["Makefile"], ["--event", "pull_request"]);
    assert.equal(make.mode, "workspace");
    assert.deepEqual(make.products.floor_checks, []);
    const github = run(["--format", "github", "--event", "pull_request", "--", "Cargo.toml"]);
    assert.equal(github.status, 0, github.stderr);
    const out = Object.fromEntries(github.stdout.trim().split("\n").map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]));
    assert.equal(out.floor_check_count, String(lock.products.floor_checks.length));
    assert.deepEqual(JSON.parse(out.floor_check_matrix).include.map((row) => row.product), lock.products.floor_checks);
    assert.equal(out.core_changed, "true");
    assert.match(out.rustdoc_flags, /^--workspace( --exclude \S+)+$/, "the rustdoc lane documents Core's members");
  }
}

// Bad arguments fail loudly.
{
  const result = run(["--format", "yaml", "--", "crates/meerkat-core/src/lib.rs"]);
  assert.notEqual(result.status, 0, "invalid format is an error");
}

console.log("ci-cargo-lanes fail-closed contracts hold");
