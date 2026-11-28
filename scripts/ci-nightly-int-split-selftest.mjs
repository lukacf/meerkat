#!/usr/bin/env node
// Contracts of the Nightly's integration-fast rows (nightly.yml:
// workspace-int-plan, workspace-int, workspace-int-aggregate;
// scripts/ci-nightly-int-split.mjs, scripts/ci-nightly-int-row.sh). The plan
// job runs this file as its first step after the checkout, like the nightly
// example jobs run theirs: the rows exist in no other workflow, and the lane
// self-test target every pull request runs stays as it is.
//
// Synthetic plans, lists and logs and a stub Cargo only: nothing here builds,
// lists or runs a test.
//
// Run: node scripts/ci-nightly-int-split-selftest.mjs

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  BIN_HARNESSES,
  BIN_ROW_NAME,
  BIN_ROW_RUNNER,
  BIN_ROW_TIMEOUT_MINUTES,
  HEAD_CANONICAL_FILTER,
  NIGHTLY_PROFILE,
  ROW_RUNNER,
  ROW_TIMEOUT_MINUTES,
  SplitError,
  aggregate,
  nightlyRows,
  parseCountLine,
  rowProblems,
  rowReport,
} from "./ci-nightly-int-split.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(resolve(root, path), "utf8");
const CONDITION = "    if: ${{ !cancelled() && !inputs.graph_only }}";

// --- 1. The workflow: three jobs in one chain, bounded, reporting ---------

const nightly = read(".github/workflows/nightly.yml");

function jobLines(job) {
  const lines = nightly.split("\n");
  const start = lines.indexOf(`  ${job}:`);
  assert.ok(start >= 0, `nightly.yml has the job ${job}`);
  const next = lines.findIndex((line, index) => index > start && /^  [A-Za-z0-9_-]+:\s*$/.test(line));
  return lines.slice(start, next === -1 ? lines.length : next);
}

const planJob = jobLines("workspace-int-plan");
const rowsJob = jobLines("workspace-int");
const aggregateJob = jobLines("workspace-int-aggregate");
for (const [name, job, need] of [
  ["workspace-int-plan", planJob, "feature-matrix"],
  ["workspace-int", rowsJob, "workspace-int-plan"],
  ["workspace-int-aggregate", aggregateJob, "workspace-int"],
]) {
  assert.ok(job.includes(`    needs: ${need}`), `${name} needs ${need} alone: one chain, no fan-out`);
  assert.ok(job.includes(CONDITION), `${name} runs under the nightly condition`);
}
// The account's hosted slots are shared with pull requests and main pushes:
// at most four rows at once, and one failed row does not stop the others.
assert.ok(rowsJob.includes("      max-parallel: 4"), "the rows run at most four at once");
assert.ok(rowsJob.includes("      fail-fast: false"), "a failed row does not cancel the other rows");
assert.ok(rowsJob.includes("    runs-on: ${{ matrix.runner }}"), "a row runs on the runner its matrix entry names");
assert.ok(rowsJob.includes("    timeout-minutes: ${{ matrix.timeout_minutes }}"), "a row has the limit its matrix entry names");
assert.ok(rowsJob.includes('      CARGO_BUILD_JOBS: "2"'), "the rows keep the monolith's two Cargo jobs");
assert.ok(rowsJob.includes("        run: bash scripts/ci-nightly-int-row.sh"), "the rows run the row script as a whole step");
for (const step of [
  "Free runner disk",
  "Show the runner's size and add swap (workspace integration-fast tests)",
  "Show disk use before the workspace integration-fast tests",
  "Keep the disk reading taken before the workspace integration-fast tests",
  "Show memory after the workspace integration-fast tests",
  "Keep the row's report and the disk reading taken after its tests",
]) {
  assert.ok(rowsJob.includes(`      - name: ${step}`), `the rows job has the step "${step}"`);
}
assert.ok(
  planJob.includes("        run: node scripts/ci-nightly-int-split-selftest.mjs"),
  "the plan job runs this file as a step",
);
assert.ok(
  planJob.some((line) => line.includes("node scripts/ci-cargo-lanes.mjs --format json --event push --head HEAD")),
  "the plan job asks the planner for the whole workspace (a push plan without a base)",
);
assert.ok(
  aggregateJob.some((line) => line.includes("ROWS_RESULT: ${{ needs.workspace-int.result }}")),
  "the aggregate is told how the rows job ended",
);
assert.ok(
  aggregateJob.some((line) => line.includes("node scripts/ci-nightly-int-split.mjs aggregate")),
  "the aggregate job runs the aggregate",
);
assert.ok(
  !nightly.split("\n").some((line) => /^\s+(run: )?make test-int\s*$/.test(line)),
  "no Nightly step runs `make test-int` any more: its build does not fit the runner's disk",
);
assert.ok(!read("Makefile").includes("ci-nightly-int-split-selftest"), "the Makefile does not run this file");

// --- 2. The limits the rows run under are the monolith's ------------------

// `make test-int` and its HeadCanonical step, as the lane script has them:
// the rows copy this profile and this filter.
const lane = read("scripts/run-build-backend-lane");
assert.ok(
  lane.includes(`"\${cargo_bin}" nextest run "\${core_workspace[@]}" --tests --profile ${NIGHTLY_PROFILE} `),
  `make test-int runs the profile ${NIGHTLY_PROFILE}`,
);
assert.ok(lane.includes(`-E '${HEAD_CANONICAL_FILTER}'`), "the HeadCanonical step has the kill-window filter");

// nextest's configuration, by its text: `fast` reports a slow test and never
// ends one, in the profile and in every override of its own; and it selects
// what the planned rows' profile selects.
const nextest = read(".config/nextest.toml");
function sections(header) {
  const found = [];
  let current = null;
  for (const line of nextest.split("\n")) {
    if (/^\[/.test(line)) {
      current = line.trim() === header ? [] : null;
      if (current) found.push(current);
    } else if (current) {
      current.push(line);
    }
  }
  return found.map((lines) => lines.filter((line) => !/^\s*#/.test(line)).join("\n"));
}
const fast = sections(`[profile.${NIGHTLY_PROFILE}]`);
assert.equal(fast.length, 1, "nextest.toml has one fast profile");
assert.match(fast[0], /^slow-timeout = \{ period = "60s" \}$/m, "fast reports a slow test each minute");
assert.doesNotMatch(fast[0], /terminate-after/, "fast ends no slow test");
for (const override of sections(`[[profile.${NIGHTLY_PROFILE}.overrides]]`)) {
  assert.doesNotMatch(override, /slow-timeout|terminate-after/, "no override of fast sets a time limit");
}
const defaultFilter = (text) => /^default-filter = (.*)$/m.exec(text)?.[1];
assert.ok(defaultFilter(fast[0]), "fast has a default-filter");
assert.equal(
  defaultFilter(sections("[profile.ci-integration]")[0] ?? ""),
  defaultFilter(fast[0]),
  "fast selects what the planned rows' profile selects",
);

// --- 3. The matrix from a plan --------------------------------------------

const SELECTION = "--workspace --exclude product-a --features core/workspace-tests";
function planned(overrides = {}) {
  return {
    name: "tests mob 1/2+3",
    lane: "tests",
    product: "core",
    package_flags: SELECTION,
    test_flags: "--test a --test b",
    profile: "ci-integration",
    binaries: ["mob::a", "mob::b", "meerkat-mob::cold_restart_mob_resume"],
    no_tests: "warn",
    rust_min_stack: 8388608,
    ...overrides,
  };
}
function plan(rows) {
  return { mode: "workspace", closure_test_rows: rows };
}
const CANARY = (k) => planned({ name: `tests meerkat canaries ${k}/2`, test_flags: "--test canary", binaries: ["meerkat::canary"], no_tests: "fail", partition: `count:${k}/2` });
const HEAD = planned({
  name: "HeadCanonical",
  lane: "headcanonical",
  package_flags: "-p meerkat-mob --features meerkat-mob/test-support",
  test_flags: "--test cold_restart_mob_resume",
  filter: "binary(cold_restart_mob_resume)",
  binaries: ["meerkat-mob::cold_restart_mob_resume"],
  no_tests: "fail",
});
const E2E = planned({ name: "e2e-fast", lane: "e2e-fast", test_flags: "--test e2e_fast_lane", profile: "ci-unit", filter: "binary(e2e_fast_lane)", binaries: ["meerkat-integration-tests::e2e_fast_lane"], no_tests: "fail" });
const META = planned({ name: "tests meta 1/1", product: "meta", package_flags: "-p meta", binaries: ["meta::x"] });
const GOOD = [planned(), planned({ name: "tests runtime 1/1", test_flags: "--test c", binaries: ["runtime::c"] }), CANARY(1), CANARY(2), HEAD, E2E, META];

const { matrix, rows } = nightlyRows(plan(GOOD));
assert.deepEqual(
  matrix.include.map((entry) => [entry.id, entry.kind]),
  [
    ["tests-mob-1-2-3", "tests"],
    ["tests-runtime-1-1", "tests"],
    ["tests-meerkat-canaries-1-2", "canary"],
    ["tests-meerkat-canaries-2-2", "canary"],
    ["headcanonical", "head-canonical"],
    ["bin-harnesses", "bins"],
  ],
  "Core's rows without e2e-fast, then the bin row; no other product's row",
);
for (const entry of matrix.include) {
  assert.equal(entry.profile, NIGHTLY_PROFILE, `${entry.name}: the monolith's profile, not the push lane's`);
  assert.equal(entry.rust_min_stack, "8388608", `${entry.name}: the 8 MiB stack`);
  for (const key of ["id", "name", "kind", "packages", "tests", "profile", "binaries", "no_tests", "check", "runner"]) {
    assert.ok(typeof entry[key] === "string" && entry[key] !== "", `${entry.name}: ${key} is set`);
  }
  assert.ok(Number.isInteger(entry.timeout_minutes) && entry.timeout_minutes > 0, `${entry.name}: a limit in minutes`);
}
const first = matrix.include[0];
assert.deepEqual(
  [first.packages, first.tests, first.filter, first.binaries, first.no_tests, first.partition, first.check, first.runner, first.timeout_minutes],
  [SELECTION, "--test a --test b", "", "mob::a mob::b meerkat-mob::cold_restart_mob_resume", "warn", "", "binaries", ROW_RUNNER, ROW_TIMEOUT_MINUTES],
  "a test row keeps the planner's selection, targets and binaries",
);
assert.equal(matrix.include[2].partition, "count:1/2", "a canary row keeps its partition");
const head = matrix.include[4];
assert.deepEqual(
  [head.packages, head.tests, head.filter, head.no_tests, head.check],
  ["-p meerkat-mob --features meerkat-mob/test-support", "--test cold_restart_mob_resume", HEAD_CANONICAL_FILTER, "fail", "selected"],
  "HeadCanonical keeps its own build and the monolith's filter, failing when nothing is selected",
);
const bins = matrix.include[5];
assert.deepEqual(
  [bins.name, bins.packages, bins.tests, bins.filter, bins.binaries, bins.no_tests, bins.check, bins.runner, bins.timeout_minutes],
  [BIN_ROW_NAME, SELECTION, "--bins", "", BIN_HARNESSES.join(" "), "fail", "binaries", BIN_ROW_RUNNER, BIN_ROW_TIMEOUT_MINUTES],
  "the bin row: Core's selection with --bins, the twelve harnesses by name, its own sizing",
);
assert.equal(BIN_HARNESSES.length, 12, "twelve bin harnesses");
assert.equal(new Set(BIN_HARNESSES).size, 12, "no bin harness twice");
assert.ok(BIN_HARNESSES.every((id) => /^[a-z0-9-]+::bin\/[a-z0-9_-]+$/.test(id)), "bin harnesses are nextest's bin ids");
assert.deepEqual(
  rows.map((row) => [row.id, row.kind, row.binaries.length, row.partition]),
  matrix.include.map((entry) => [entry.id, entry.kind, entry.binaries.split(" ").length, entry.partition]),
  "the rows file names every row of the matrix",
);

const refuses = (rowsOf, pattern, what) => assert.throws(() => nightlyRows(plan(rowsOf)), (error) => error instanceof SplitError && pattern.test(error.message), what);
assert.throws(() => nightlyRows({ ...plan(GOOD), mode: "packages" }), /not a whole-workspace plan/, "a plan of a part of the workspace is refused");
refuses([META], /no closure row of Core/, "a plan without Core's rows is refused");
refuses(GOOD.filter((row) => row !== E2E), /exactly one row that names meerkat-integration-tests::e2e_fast_lane/, "a plan without the e2e-fast row is refused: the split would not know what it leaves out");
refuses(GOOD.filter((row) => row !== HEAD), /exactly one HeadCanonical row/, "a plan without HeadCanonical is refused");
refuses([...GOOD, planned({ name: "tests new 1/1", profile: "ci-unit", binaries: ["new::x"] })], /has the profile "ci-unit"/, "a row with another profile is refused");
refuses([...GOOD, planned({ name: "tests new 1/1", filter: "not test(x)", binaries: ["new::x"] })], /is not a plain test row/, "a row with a filter is refused: it would leave a test out");
refuses([...GOOD, planned({ name: "tests new 1/1", package_flags: "-p new", binaries: ["new::x"] })], /does not select Core's workspace/, "a package-scoped row is refused");
refuses([...GOOD, planned({ name: "tests new 1/1", package_flags: `${SELECTION} --features x/y`, binaries: ["new::x"] })], /package selection of its own/, "a row with another selection is refused");
refuses([...GOOD, planned({ name: "tests mob 1/2 3" })], /used twice/, "two rows with one id are refused");
refuses([...GOOD, planned({ name: "tests new 1/1", binaries: [] })], /names no binary/, "a row without binaries is refused");

// --- 4. nextest's count line ----------------------------------------------

assert.equal(parseCountLine("no summary here\n"), null);
assert.deepEqual(parseCountLine("     Summary [ 118.690s] 160 tests run: 160 passed (1 slow), 3 skipped\n"), {
  line: "Summary [ 118.690s] 160 tests run: 160 passed (1 slow), 3 skipped",
  run: 160,
  planned: 160,
  passed: 160,
  failed: 0,
  skipped: 3,
  other: {},
});
assert.deepEqual(
  (({ run, planned: of, passed, failed, skipped, other }) => ({ run, of, passed, failed, skipped, other }))(
    parseCountLine("\u001b[32;1m     Summary\u001b[0m [   0.055s] \u001b[1m5\u001b[0m/\u001b[1m9\u001b[0m tests run: \u001b[1m2\u001b[0m \u001b[32;1mpassed\u001b[0m (1 slow, 1 flaky), 2 failed, 1 timed out, 4 skipped\n"),
  ),
  { run: 5, of: 9, passed: 2, failed: 2, skipped: 4, other: { "timed out": 1 } },
  "colour escapes, a cut-short run, flaky and timed-out tests",
);
assert.equal(parseCountLine("     Summary [   0.041s] 1 test run: 1 passed, 0 skipped\n").run, 1, "the singular form");
assert.equal(
  parseCountLine("Summary [ 1.0s] 1 test run: 1 passed, 0 skipped\nSummary [ 2.0s] 7 tests run: 7 passed, 0 skipped\n").run,
  7,
  "the last count line of the log",
);

// --- 5. A row's report ----------------------------------------------------

const MATCHES = { status: "matches" };
const testCase = (match) => ({ ignored: match.reason === "ignored", "filter-match": match });
const suite = (cases) => ({ status: "listed", testcases: Object.fromEntries(Object.entries(cases).map(([name, match]) => [name, testCase(match)])) });
const ROW = { id: "tests-x-1-1", name: "tests x 1/1", kind: "tests", binaries: ["x::a", "x::b"], partition: "" };
const LIST = {
  "rust-suites": {
    "x::a": suite({ one: MATCHES, two: MATCHES, three: { status: "mismatch", reason: "ignored" } }),
    "x::b": suite({ four: MATCHES, five: { status: "mismatch", reason: "default-filter" } }),
    "y::other": suite({ six: { status: "mismatch", reason: "expression" } }),
  },
};
const LOG = "     Summary [   1.000s] 3 tests run: 3 passed, 2 skipped\n";
const good = rowReport({ row: ROW, list: LIST, log: LOG, exitCode: 0 });
assert.deepEqual(
  [good.listed, good.tests, good.selected, good.ignored, good.filtered, good.count_line.run, good.exit_code],
  [true, 5, 3, 1, 1, 3, 0],
  "the report counts the row's own binaries",
);
assert.deepEqual(good.binaries["x::a"], { listed: true, tests: 3, selected: 2, ignored: 1, filtered: 0 });
assert.deepEqual(rowProblems(ROW, good), [], "a completed green row has no problem");
const problemOf = (report) => rowProblems(ROW, report).join(" | ");
assert.match(problemOf(null), /no report/);
assert.match(problemOf(rowReport({ row: ROW, list: LIST, log: LOG, exitCode: 100 })), /exited 100/);
assert.match(problemOf(rowReport({ row: ROW, list: null, log: null, exitCode: 101 })), /listed nothing.*no count line/);
assert.match(problemOf(rowReport({ row: ROW, list: LIST, log: "nothing\n", exitCode: 0 })), /no count line/);
assert.match(problemOf(rowReport({ row: ROW, list: LIST, log: "Summary [ 1.0s] 2 tests run: 2 passed, 3 skipped\n", exitCode: 0 })), /2 tests ran, nextest selected 3/);
assert.match(problemOf(rowReport({ row: ROW, list: LIST, log: "Summary [ 1.0s] 2/3 tests run: 2 passed, 2 skipped\n", exitCode: 0 })), /cut short: 2 of 3/);
assert.match(problemOf(rowReport({ row: ROW, list: LIST, log: "Summary [ 1.0s] 3 tests run: 2 passed, 1 failed, 2 skipped\n", exitCode: 0 })), /did not pass/);
assert.match(problemOf(rowReport({ row: ROW, list: LIST, log: "Summary [ 1.0s] 3 tests run: 2 passed, 1 timed out, 2 skipped\n", exitCode: 0 })), /did not pass/);
assert.match(problemOf(rowReport({ row: { ...ROW, binaries: ["x::a", "x::gone"] }, list: LIST, log: LOG, exitCode: 0 })), /2 tests ran|did not list/);
assert.match(
  rowProblems({ ...ROW, kind: "bins" }, rowReport({ row: ROW, list: { "rust-suites": { "x::a": suite({}), "x::b": suite({}) } }, log: "Summary [ 0.0s] 0 tests run: 0 passed, 0 skipped\n", exitCode: 0 })).join(" | "),
  /no test was selected/,
  "a bin, canary or HeadCanonical row that selects nothing is not accounted for",
);

// --- 6. The aggregate -----------------------------------------------------

const scratch = mkdtempSync(join(tmpdir(), "ci-nightly-int-split-"));
try {
  const AGG_ROWS = [
    { id: "tests-mob-1-1", name: "tests mob 1/1", kind: "tests", binaries: ["mob::a", "meerkat-mob::cold_restart_mob_resume"], partition: "" },
    { id: "canary-1-2", name: "canaries 1/2", kind: "canary", binaries: ["meerkat::canary"], partition: "count:1/2" },
    { id: "canary-2-2", name: "canaries 2/2", kind: "canary", binaries: ["meerkat::canary"], partition: "count:2/2" },
    { id: "headcanonical", name: "HeadCanonical", kind: "head-canonical", binaries: ["meerkat-mob::cold_restart_mob_resume"], partition: "" },
    { id: "bin-harnesses", name: BIN_ROW_NAME, kind: "bins", binaries: ["rkat::bin/rkat"], partition: "" },
  ];
  const listOf = (row, selected) => ({ "rust-suites": Object.fromEntries(row.binaries.map((id, index) => [id, suite(index === 0 ? Object.fromEntries([...Array(selected).keys()].map((n) => [`t${n}`, MATCHES]).concat([["ig", { status: "mismatch", reason: "ignored" }]])) : {})])) });
  const reportOf = (row, selected, exitCode = 0, run = selected) =>
    rowReport({ row, list: listOf(row, selected), log: `Summary [ 1.0s] ${run} tests run: ${run} passed, 1 skipped\n`, exitCode });
  function lay(name, { attempt = 1, plan: withPlan = true, reports }) {
    const dir = join(scratch, name);
    if (withPlan) {
      mkdirSync(join(dir, `nightly-int-plan-77-${attempt}`), { recursive: true });
      writeFileSync(join(dir, `nightly-int-plan-77-${attempt}`, "nightly-int-rows.json"), JSON.stringify(AGG_ROWS));
    }
    for (const [id, at, report] of reports) {
      mkdirSync(join(dir, `nightly-int-row-${id}-77-${at}`), { recursive: true });
      writeFileSync(join(dir, `nightly-int-row-${id}-77-${at}`, "report.json"), JSON.stringify(report));
    }
    return dir;
  }
  const allGood = AGG_ROWS.map((row, index) => [row.id, 1, reportOf(row, index + 1)]);
  let dir = lay("good", { reports: allGood });
  let result = aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: "success" });
  assert.equal(result.ok, true, result.text);
  assert.match(result.text, /^## Nightly integration-fast rows: every row accounted for$/m);
  assert.match(result.text, /^\| tests mob 1\/1 \| 2 \| 2 \| 1 \| 1 \| 0 \| 1 \| 1 \| 0 \| 1 \| ok \|$/m, "a row's line: binaries, listed, selected, ignored, filtered, run, passed, failed, skipped");
  assert.match(result.text, /^\| all 5 rows \| \| 20 \| 15 \| 5 \| 0 \| 15 \| 15 \| 0 \| 5 \| ok \|$/m, "the totals");
  assert.match(result.text, /Lib harnesses: not in these rows\. The workspace-unit job/);
  assert.match(result.text, /Duplicates .*meerkat-mob::cold_restart_mob_resume in "tests mob 1\/1" and "HeadCanonical"/);
  assert.match(result.text, /Partitions, not duplicates: meerkat::canary/);
  assert.match(result.text, /Bin harnesses: 5 tests ran in the "bin harnesses" row \(6 listed, 1 ignored\)\./);
  assert.equal(aggregate({ rowsDir: dir, reportsDir: dir }).ok, true, "without the rows job's result the reports decide");
  for (const ended of ["failure", "cancelled", "skipped"]) {
    result = aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: ended });
    assert.equal(result.ok, false, `a rows job that ended ${ended} is not accounted for`);
    assert.match(result.text, new RegExp(`the rows job ended "${ended}"`));
  }
  dir = lay("missing", { reports: allGood.slice(1) });
  result = aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: "success" });
  assert.equal(result.ok, false, "a planned row without a report is not accounted for");
  assert.match(result.text, /^\| tests mob 1\/1 \| 2 \| - \| - \| - \| - \| - \| - \| - \| - \| NOT OK: no report/m);
  assert.match(result.text, /1 NOT OK/);
  dir = lay("failed", { reports: [...allGood.slice(1), [AGG_ROWS[0].id, 1, reportOf(AGG_ROWS[0], 1, 100)]] });
  assert.equal(aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: "success" }).ok, false, "a row that exited non-zero is not accounted for");
  dir = lay("short", { reports: [...allGood.slice(1), [AGG_ROWS[0].id, 1, reportOf(AGG_ROWS[0], 4, 0, 3)]] });
  assert.equal(aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: "success" }).ok, false, "a row that ran fewer tests than selected is not accounted for");
  // A rerun of failed jobs: the second attempt's report of the rerun row
  // replaces the first, and the rows it did not rerun keep theirs.
  dir = lay("rerun", { reports: [...allGood.slice(1), [AGG_ROWS[0].id, 1, reportOf(AGG_ROWS[0], 1, 100)], [AGG_ROWS[0].id, 2, reportOf(AGG_ROWS[0], 1)]] });
  assert.equal(aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: "success" }).ok, true, "the newest attempt's report of a row counts");
  dir = lay("no-plan", { plan: false, reports: allGood });
  result = aggregate({ rowsDir: dir, reportsDir: dir, rowsResult: "success" });
  assert.equal(result.ok, false, "without the plan's rows nothing is accounted for");
  assert.match(result.text, /NO PLAN/);

  // --- 7. The row script against a stub Cargo -----------------------------

  const repo = join(scratch, "repo");
  mkdirSync(join(repo, "scripts"), { recursive: true });
  for (const name of ["ci-nightly-int-row.sh", "ci-nightly-int-split.mjs"]) copyFileSync(resolve(root, "scripts", name), join(repo, "scripts", name));
  symlinkSync(resolve(root, "scripts/ci-verify-row-binaries.mjs"), join(repo, "scripts/ci-verify-row-binaries.mjs"));
  const stub = (name, lines) => {
    writeFileSync(join(repo, "scripts", name), ["#!/usr/bin/env bash", ...lines, ""].join("\n"));
    chmodSync(join(repo, "scripts", name), 0o755);
  };
  stub("mcp-test-server-fixture", ['printf "%s\\n" "$*" >>"${STUB_DIR}/fixture-args"', 'if [[ -n "${STUB_FIXTURE:-}" ]]; then echo "${STUB_FIXTURE}"; fi']);
  stub("repo-cargo", [
    'printf "%s\\n" "$*" >>"${STUB_DIR}/cargo-args"',
    'printf "%s|%s\\n" "${RUST_MIN_STACK:-unset}" "${MEERKAT_MCP_TEST_SERVER:-unset}" >>"${STUB_DIR}/cargo-env"',
    'if [[ "$2" == "list" ]]; then cat "${STUB_DIR}/list.json"; exit "${STUB_LIST_EXIT:-0}"; fi',
    'if [[ "$2" == "run" ]]; then cat "${STUB_DIR}/run.log" >&2; exit "${STUB_RUN_EXIT:-0}"; fi',
    "exit 64",
  ]);
  chmodSync(join(repo, "scripts/ci-nightly-int-row.sh"), 0o755);
  // The real binary check runs from the stand-in checkout. It imports the
  // planner, which asks git for the checkout's root: point git at this one.
  const gitDir = spawnSync("git", ["rev-parse", "--absolute-git-dir"], { cwd: root, encoding: "utf8" }).stdout.trim();
  assert.ok(gitDir, "this checkout has a git directory");
  let caseNumber = 0;
  function runRow({ list, log, env = {} }) {
    caseNumber += 1;
    const stubDir = join(scratch, `row-${caseNumber}`);
    mkdirSync(stubDir, { recursive: true });
    writeFileSync(join(stubDir, "list.json"), typeof list === "string" ? list : JSON.stringify(list));
    writeFileSync(join(stubDir, "run.log"), log ?? "");
    const run = spawnSync("bash", ["scripts/ci-nightly-int-row.sh"], {
      cwd: repo,
      encoding: "utf8",
      env: {
        PATH: process.env.PATH,
        HOME: stubDir,
        GIT_DIR: gitDir,
        GIT_WORK_TREE: root,
        STUB_DIR: stubDir,
        PACKAGES: "--workspace --exclude product-a",
        TESTS: "--test a --test b",
        PROFILE: "fast",
        FILTER: "",
        BINARIES: "x::a x::b",
        NO_TESTS: "warn",
        MIN_STACK: "8388608",
        PARTITION: "",
        ROW_ID: "tests-x-1-1",
        ROW_NAME: "tests x 1/1",
        ROW_KIND: "tests",
        ROW_CHECK: "binaries",
        REPORT_DIR: join(stubDir, "report"),
        ...env,
      },
    });
    const file = join(stubDir, "report", "report.json");
    const lines = (name) => (existsSync(join(stubDir, name)) ? readFileSync(join(stubDir, name), "utf8").trim().split("\n") : []);
    return { run, report: existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : null, cargo: lines("cargo-args"), cargoEnv: lines("cargo-env"), fixture: lines("fixture-args") };
  }
  const ROW_LIST = { "rust-suites": { "x::a": LIST["rust-suites"]["x::a"], "x::b": suite({ four: MATCHES }) } };

  let row = runRow({ list: ROW_LIST, log: LOG, env: { STUB_FIXTURE: "/stub/mcp-test-server" } });
  assert.equal(row.run.status, 0, row.run.stdout + row.run.stderr);
  assert.deepEqual(row.cargo, [
    "nextest list --workspace --exclude product-a --test a --test b --profile fast --message-format json",
    "nextest run --workspace --exclude product-a --test a --test b --profile fast --no-tests=warn --no-fail-fast --status-level slow --final-status-level slow",
  ]);
  assert.deepEqual(row.cargoEnv, ["8388608|/stub/mcp-test-server", "8388608|/stub/mcp-test-server"], "the stack and the fixture reach the list and the run");
  assert.deepEqual(row.fixture, ["--if-needed --workspace --exclude product-a"]);
  assert.deepEqual([row.report.exit_code, row.report.selected, row.report.ignored, row.report.count_line.run], [0, 3, 1, 3]);
  assert.deepEqual(rowProblems(ROW, row.report), [], "the report of a green row is accounted for");
  assert.match(row.run.stdout, /row binaries verified: 2 listed as planned, 3 tests selected, 1 ignored/);
  assert.match(row.run.stdout, /row report "tests x 1\/1": 3 selected, 1 ignored, 0 filtered out; Summary .* ok/);

  row = runRow({ list: ROW_LIST, log: "     Summary [   1.000s] 3 tests run: 2 passed, 1 failed, 1 skipped\n", env: { STUB_RUN_EXIT: "100" } });
  assert.equal(row.run.status, 100, "a red run is the row's exit status");
  assert.deepEqual([row.report.exit_code, row.report.count_line.failed], [100, 1], "and its report says so");

  row = runRow({ list: "", log: LOG, env: { STUB_LIST_EXIT: "101" } });
  assert.equal(row.run.status, 101, "a build that fails is the row's exit status");
  assert.deepEqual([row.report.exit_code, row.report.listed, row.report.count_line], [101, false, null], "and the row still reports, with that status");
  assert.equal(row.cargo.length, 1, "nothing runs after a failed build");

  row = runRow({ list: { "rust-suites": { ...ROW_LIST["rust-suites"], "y::extra": suite({ six: MATCHES }) } }, log: LOG });
  assert.equal(row.run.status, 1, "a selected test of a binary the row does not name stops the row");
  assert.match(row.run.stderr, /nextest selects 1 test\(s\) of y::extra/);
  assert.equal(row.cargo.length, 1, "nothing runs after a failed binary check");
  assert.deepEqual([row.report.exit_code, row.report.count_line], [1, null]);

  row = runRow({ list: { "rust-suites": { "x::a": suite({ one: MATCHES, two: { status: "mismatch", reason: "default-filter" } }), "x::b": suite({ four: MATCHES }) } }, log: LOG });
  assert.equal(row.run.status, 1, "a test of the row's binaries that the profile drops stops the row");
  assert.match(row.run.stderr, /x::a two is filtered out \(default-filter\)/);

  row = runRow({
    list: { "rust-suites": { "m::k": suite({ kill: MATCHES, other: { status: "mismatch", reason: "expression" } }) } },
    log: "     Summary [   1.000s] 1 test run: 1 passed, 1 skipped\n",
    env: { PACKAGES: "-p m --features m/test-support", TESTS: "--test k", FILTER: HEAD_CANONICAL_FILTER, BINARIES: "m::k", NO_TESTS: "fail", ROW_ID: "headcanonical", ROW_NAME: "HeadCanonical", ROW_KIND: "head-canonical", ROW_CHECK: "selected" },
  });
  assert.equal(row.run.status, 0, row.run.stdout + row.run.stderr);
  assert.equal(row.cargo[1], `nextest run -p m --features m/test-support --test k --profile fast -E ${HEAD_CANONICAL_FILTER} --no-tests=fail --no-fail-fast --status-level slow --final-status-level slow`);
  assert.deepEqual([row.report.selected, row.report.filtered, row.report.count_line.run], [1, 1, 1], "a row that selects by name has no binary check and reports what was selected");

  row = runRow({
    list: { "rust-suites": { "c::canary": suite({ p1: MATCHES, p2: { status: "mismatch", reason: "partition" } }) } },
    log: "     Summary [   1.000s] 1 test run: 1 passed, 1 skipped\n",
    env: { TESTS: "--test canary", BINARIES: "c::canary", NO_TESTS: "fail", PARTITION: "count:1/2", ROW_ID: "canary-1-2", ROW_NAME: "canaries 1/2", ROW_KIND: "canary" },
  });
  assert.equal(row.run.status, 0, row.run.stdout + row.run.stderr);
  assert.equal(row.cargo[1], "nextest run --workspace --exclude product-a --test canary --profile fast --partition count:1/2 --no-tests=fail --no-fail-fast --status-level slow --final-status-level slow");
  assert.match(row.run.stdout, /1 in the other partitions of count:1\/2/);

  row = runRow({ list: ROW_LIST, log: LOG, env: { ROW_KIND: "" } });
  assert.equal(row.run.status, 1, "an incomplete row stops before anything runs");
  assert.match(row.run.stdout, /incomplete nightly integration-fast row: ROW_KIND is empty/);
  assert.equal(row.cargo.length, 0);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

process.stdout.write("nightly integration-fast split contracts hold\n");
