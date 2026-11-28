#!/usr/bin/env node
// The Nightly's integration-fast rows (nightly.yml: workspace-int-plan,
// workspace-int, workspace-int-aggregate). `make test-int` built every test
// harness of Core's workspace in one job; that build no longer fits a hosted
// runner's disk. The rows build the same harnesses a few at a time.
//
//   matrix <planner.json> --rows <rows.json>
//       Reads a whole-workspace plan of scripts/ci-cargo-lanes.mjs
//       (--format json) and prints the job matrix: Core's closure rows as the
//       planner packs them, without the e2e-fast row (the Nightly's e2e-fast
//       job runs that binary), plus one row for the bin harnesses, which no
//       closure row names. Writes the rows the aggregate must account for.
//   report --row ID --name NAME --kind KIND --binaries "ID ..."
//          --list <nextest-list.json> --log <nextest-run.log> --exit CODE
//          --out <report.json>
//       One row's report: what nextest listed and selected for the row's
//       binaries, and the count line its run printed.
//   aggregate --rows-dir DIR --reports-dir DIR [--rows-result RESULT]
//             [--summary FILE]
//       Prints every row's counts and the totals, and exits 1 unless every
//       planned row reported a completed, successful run of exactly the tests
//       nextest selected for it.
//
// What the rows keep of `make test-int` (scripts/run-build-backend-lane):
// - its package selection, which the planner's whole-workspace rows carry
//   (Core's `--workspace` and `--exclude` list with every member's test
//   features);
// - its nextest profile, `fast`: the planner's rows name the push lane's
//   profile, which ends a test after 240 s (960 s for a few); `fast` only
//   reports a slow test, so no test gets a shorter limit here than it had;
// - its HeadCanonical step: the kill-window filter in its own
//   package-scoped build (the planner's row runs that whole binary);
// - its bin harnesses: `--tests` built and ran them, and no closure row or
//   other Nightly job does, so they get the explicit row below.
// What it leaves elsewhere: the lib harnesses. `make test-int` ran them a
// second time; the workspace-unit job (`make test-unit`) runs them from the
// same selection.
// What differs on a failing night (scripts/ci-nightly-int-row.sh):
// - a row runs every test it selected (`--no-fail-fast`): the aggregate
//   needs each row's whole count. `make test-int` had nextest's default and
//   stopped starting tests after the first failure;
// - the HeadCanonical row does not wait for the other rows. `make test-int`
//   ran its HeadCanonical step only after its first command had passed; the
//   step has a build and a store of its own, so nothing in it depends on
//   that order.
//
// Counts printed here are nextest's own, per run. Nothing in this file
// asserts a number of tests.

import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

export const NIGHTLY_PROFILE = "fast";
// The push lane's profile, the only one a planned Core row may name: a row
// with another profile is a row this file does not know how to keep at the
// monolith's limits.
export const PLANNED_ROW_PROFILE = "ci-integration";
export const E2E_FAST_BINARY = "meerkat-integration-tests::e2e_fast_lane";
export const HEAD_CANONICAL_BINARY = "meerkat-mob::cold_restart_mob_resume";
// scripts/run-build-backend-lane, run_head_canonical_kill_window.
export const HEAD_CANONICAL_FILTER = "test(mob_cold_restart_resume_after_kill_between_commit_points)";
// The bin targets of Core's workspace with test = true, as nextest names
// their harnesses. `--bins` under Core's selection builds exactly these; the
// row's binary check (scripts/ci-verify-row-binaries.mjs) fails when nextest
// selects a test of a bin harness that is not listed here.
export const BIN_HARNESSES = [
  "mcp-test-server::bin/mcp-test-server",
  "meerkat-contracts::bin/emit-schemas",
  "meerkat-integration-tests::bin/e2e_artifacts",
  "meerkat-integration-tests::bin/voice_fixtures",
  "meerkat-mcp-server::bin/rkat-mcp",
  "meerkat-mob-mcp::bin/rkat-mob-mcp",
  "meerkat-rest::bin/rkat-rest",
  "meerkat-rpc::bin/rkat-rpc",
  "rkat::bin/rkat",
  "surface-build-fixtures::bin/embedded_min",
  "surface-build-fixtures::bin/runtime_backed_min",
  "xtask::bin/xtask",
];
// Sizing. Every row runs on the monolith's runner label under the monolith's
// limit: a row builds a part of what `make test-int` built and runs a part
// of its tests. The bin row is sized by its own two constants: rkat's
// harness links the whole CLI, so it is the row to resize first.
export const ROW_RUNNER = "ubuntu-24.04";
export const ROW_TIMEOUT_MINUTES = 240;
export const BIN_ROW_RUNNER = "ubuntu-24.04";
export const BIN_ROW_TIMEOUT_MINUTES = 240;
export const BIN_ROW_NAME = "bin harnesses";
export const REPORT_SCHEMA = 1;

export class SplitError extends Error {}

function slug(name) {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}

function binariesOf(row) {
  const value = row.binaries;
  const list = Array.isArray(value) ? value : String(value ?? "").split(/\s+/).filter(Boolean);
  if (list.length === 0) throw new SplitError(`row ${JSON.stringify(row.name)} names no binary`);
  return list;
}

/// The Nightly's rows from a planner plan: `{ matrix, rows }`.
export function nightlyRows(plan) {
  if (plan?.mode !== "workspace") {
    throw new SplitError(`the plan is not a whole-workspace plan (mode ${JSON.stringify(plan?.mode)})`);
  }
  const planned = (plan.closure_test_rows ?? []).filter((row) => row.product === "core");
  if (planned.length === 0) throw new SplitError("the plan has no closure row of Core");
  const e2e = planned.filter((row) => binariesOf(row).includes(E2E_FAST_BINARY));
  if (e2e.length !== 1 || binariesOf(e2e[0]).length !== 1) {
    throw new SplitError(`expected exactly one row that names ${E2E_FAST_BINARY} alone, found ${e2e.length}`);
  }
  const head = planned.filter((row) => row.lane === "headcanonical");
  if (head.length !== 1 || binariesOf(head[0]).join(" ") !== HEAD_CANONICAL_BINARY) {
    throw new SplitError(`expected exactly one HeadCanonical row that names ${HEAD_CANONICAL_BINARY} alone, found ${head.length}`);
  }
  const include = [];
  let selection = null;
  for (const row of planned) {
    if (row === e2e[0]) continue;
    const binaries = binariesOf(row);
    if (row.profile !== PLANNED_ROW_PROFILE) {
      throw new SplitError(`row ${JSON.stringify(row.name)} has the profile ${JSON.stringify(row.profile)}, not ${PLANNED_ROW_PROFILE}`);
    }
    const entry = {
      id: slug(row.name),
      name: row.name,
      kind: "tests",
      packages: row.package_flags,
      tests: row.test_flags,
      profile: NIGHTLY_PROFILE,
      filter: "",
      binaries: binaries.join(" "),
      no_tests: row.no_tests,
      rust_min_stack: String(row.rust_min_stack),
      partition: row.partition ?? "",
      check: "binaries",
      runner: ROW_RUNNER,
      timeout_minutes: ROW_TIMEOUT_MINUTES,
    };
    if (row === head[0]) {
      entry.kind = "head-canonical";
      entry.filter = HEAD_CANONICAL_FILTER;
      entry.no_tests = "fail";
      entry.check = "selected";
    } else {
      if (row.lane !== "tests" || (row.filter ?? "") !== "") {
        throw new SplitError(`row ${JSON.stringify(row.name)} is not a plain test row (lane ${JSON.stringify(row.lane)}, filter ${JSON.stringify(row.filter ?? "")})`);
      }
      if (!/^--workspace( |$)/.test(row.package_flags)) {
        throw new SplitError(`row ${JSON.stringify(row.name)} does not select Core's workspace`);
      }
      if (selection !== null && selection !== row.package_flags) {
        throw new SplitError(`row ${JSON.stringify(row.name)} has a package selection of its own`);
      }
      selection = row.package_flags;
      if (entry.partition !== "") entry.kind = "canary";
    }
    include.push(entry);
  }
  if (selection === null) throw new SplitError("the plan has no whole-workspace test row of Core");
  const stack = include.find((entry) => entry.kind === "tests")?.rust_min_stack ?? include[0].rust_min_stack;
  include.push({
    id: slug(BIN_ROW_NAME),
    name: BIN_ROW_NAME,
    kind: "bins",
    packages: selection,
    tests: "--bins",
    profile: NIGHTLY_PROFILE,
    filter: "",
    binaries: BIN_HARNESSES.join(" "),
    no_tests: "fail",
    rust_min_stack: stack,
    partition: "",
    check: "binaries",
    runner: BIN_ROW_RUNNER,
    timeout_minutes: BIN_ROW_TIMEOUT_MINUTES,
  });
  const ids = new Set();
  for (const entry of include) {
    if (entry.id === "" || ids.has(entry.id)) throw new SplitError(`row id ${JSON.stringify(entry.id)} is empty or used twice`);
    ids.add(entry.id);
  }
  const rows = include.map((entry) => ({
    id: entry.id,
    name: entry.name,
    kind: entry.kind,
    binaries: entry.binaries.split(" "),
    partition: entry.partition,
  }));
  return { matrix: { include }, rows };
}

const ESCAPES = /\u001b\[[0-9;]*[A-Za-z]/g;
const SUMMARY = /^\s*Summary \[\s*[0-9.]+s\] ([0-9]+)(?:\/([0-9]+))? tests? run: (.*)$/;

/// nextest's last count line in a run's log, or null. `run` of `planned`
/// tests ran (they differ only when the run was cut short); the named counts
/// are the line's own words.
export function parseCountLine(text) {
  let found = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(ESCAPES, "").replace(/^[0-9T:.-]+Z /, "");
    const match = SUMMARY.exec(line);
    if (!match) continue;
    const counts = { passed: 0, failed: 0, skipped: 0 };
    const other = {};
    // "12 passed (2 slow, 1 flaky), 1 failed, 1 timed out, 30 skipped"
    for (const part of match[3].replace(/\([^)]*\)/g, "").split(",")) {
      const item = /^\s*([0-9]+) ([a-z][a-z ]*?)\s*$/.exec(part);
      if (!item) continue;
      const count = Number(item[1]);
      if (Object.hasOwn(counts, item[2])) counts[item[2]] = count;
      else other[item[2]] = count;
    }
    found = {
      line: line.trim(),
      run: Number(match[1]),
      planned: Number(match[2] ?? match[1]),
      ...counts,
      other,
    };
  }
  return found;
}

/// One row's report from nextest's list of the row and its run log.
export function rowReport({ row, list, log, exitCode }) {
  const report = {
    schema: REPORT_SCHEMA,
    id: row.id,
    name: row.name,
    kind: row.kind,
    exit_code: exitCode,
    listed: false,
    binaries: {},
    tests: 0,
    selected: 0,
    ignored: 0,
    filtered: 0,
    count_line: null,
  };
  const suites = list?.["rust-suites"];
  if (suites && typeof suites === "object") {
    report.listed = true;
    for (const id of row.binaries) {
      const cases = Object.values(suites[id]?.testcases ?? {});
      const counts = { listed: Object.hasOwn(suites, id), tests: cases.length, selected: 0, ignored: 0, filtered: 0 };
      for (const testCase of cases) {
        const match = testCase["filter-match"] ?? {};
        if (match.status === "matches") counts.selected += 1;
        else if (match.reason === "ignored") counts.ignored += 1;
        else counts.filtered += 1;
      }
      report.binaries[id] = counts;
      for (const key of ["tests", "selected", "ignored", "filtered"]) report[key] += counts[key];
    }
  }
  if (typeof log === "string") report.count_line = parseCountLine(log);
  return report;
}

/// Why a row's report does not show a completed, successful run of what
/// nextest selected for it; empty when it does.
export function rowProblems(row, report) {
  if (!report) return ["no report: the row did not run to its report step"];
  const problems = [];
  if (report.schema !== REPORT_SCHEMA) problems.push(`report schema ${JSON.stringify(report.schema)}`);
  if (report.exit_code !== 0) problems.push(`the row exited ${report.exit_code}`);
  if (!report.listed) problems.push("nextest listed nothing (the build did not finish)");
  for (const id of row.binaries) {
    if (report.listed && !report.binaries?.[id]?.listed) problems.push(`nextest did not list ${id}`);
  }
  const line = report.count_line;
  if (!line) {
    problems.push("the run printed no count line");
  } else {
    if (line.run !== line.planned) problems.push(`the run was cut short: ${line.run} of ${line.planned} tests ran`);
    if (line.failed !== 0 || Object.values(line.other).some((count) => count !== 0)) {
      problems.push(`the count line reports tests that did not pass: ${line.line}`);
    }
    if (line.passed !== line.run) problems.push(`${line.passed} of ${line.run} tests passed`);
    if (report.listed && line.run !== report.selected) {
      problems.push(`${line.run} tests ran, nextest selected ${report.selected}`);
    }
  }
  if (report.listed && report.selected === 0 && row.kind !== "tests") problems.push("no test was selected");
  return problems;
}

function newest(dir, pattern) {
  // Artifact directories end in -<run id>-<run attempt>; a rerun of failed
  // jobs leaves the earlier attempt's reports of the rows it did not rerun.
  const found = new Map();
  if (!existsSync(dir)) return found;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const match = pattern.exec(entry.name);
    if (!entry.isDirectory() || !match) continue;
    const attempt = Number(match[2]);
    const previous = found.get(match[1]);
    if (!previous || attempt > previous.attempt) found.set(match[1], { attempt, path: join(dir, entry.name) });
  }
  return found;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

/// The aggregate: `{ text, ok }` for the rows file and the row reports found
/// under `reportsDir`.
export function aggregate({ rowsDir, reportsDir, rowsResult }) {
  const plans = newest(rowsDir, /^(nightly-int-plan)-[0-9]+-([0-9]+)$/);
  const plan = plans.get("nightly-int-plan");
  if (!plan || !existsSync(join(plan.path, "nightly-int-rows.json"))) {
    return { ok: false, text: "nightly integration-fast rows: NO PLAN (the plan job left no rows file); nothing can be accounted for\n" };
  }
  const rows = readJson(join(plan.path, "nightly-int-rows.json"));
  const reports = newest(reportsDir, /^nightly-int-row-(.+)-[0-9]+-([0-9]+)$/);
  const lines = [];
  const total = { tests: 0, selected: 0, ignored: 0, filtered: 0, run: 0, passed: 0, failed: 0, skipped: 0 };
  let bad = 0;
  lines.push("| row | binaries | listed | selected | ignored | filtered out | run | passed | failed | skipped | state |");
  lines.push("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |");
  const named = new Map();
  let bins = null;
  for (const row of rows) {
    for (const id of row.binaries) named.set(id, [...(named.get(id) ?? []), row]);
    const found = reports.get(row.id);
    const file = found && join(found.path, "report.json");
    const report = file && existsSync(file) ? readJson(file) : null;
    const problems = rowProblems(row, report);
    if (problems.length > 0) bad += 1;
    const line = report?.count_line ?? null;
    const cell = (value) => (value === null || value === undefined ? "-" : String(value));
    lines.push(
      `| ${row.name} | ${row.binaries.length} | ${cell(report?.listed ? report.tests : null)} | ${cell(report?.listed ? report.selected : null)} | ` +
        `${cell(report?.listed ? report.ignored : null)} | ${cell(report?.listed ? report.filtered : null)} | ${cell(line?.run)} | ${cell(line?.passed)} | ` +
        `${cell(line?.failed)} | ${cell(line?.skipped)} | ${problems.length === 0 ? "ok" : `NOT OK: ${problems.join("; ")}`} |`,
    );
    if (report?.listed) for (const key of ["tests", "selected", "ignored", "filtered"]) total[key] += report[key];
    if (line) for (const key of ["run", "passed", "failed", "skipped"]) total[key] += line[key];
    if (row.kind === "bins") bins = { report, line };
  }
  lines.push(
    `| all ${rows.length} rows | | ${total.tests} | ${total.selected} | ${total.ignored} | ${total.filtered} | ${total.run} | ${total.passed} | ${total.failed} | ${total.skipped} | ` +
      `${bad === 0 ? "ok" : `${bad} NOT OK`} |`,
  );
  const notes = [];
  notes.push("Counts are nextest's for this run: `listed`, `selected`, `ignored` and `filtered out` from its list of each row's binaries, the rest from the count line of the row's run. `skipped` is the count line's own (ignored tests and tests outside the selection).");
  notes.push("Lib harnesses: not in these rows. The workspace-unit job of this workflow (`make test-unit`) runs them from the same package selection.");
  const canaryBinaries = new Set(rows.filter((row) => row.kind === "canary").flatMap((row) => row.binaries));
  const twice = [...named.entries()].filter(([id, users]) => users.length > 1 && !canaryBinaries.has(id));
  notes.push(
    twice.length === 0
      ? "Duplicates: no binary is named by two rows."
      : `Duplicates (a binary two rows name; their counts are both in the total): ${twice.map(([id, users]) => `${id} in ${users.map((row) => `"${row.name}"`).join(" and ")}`).join("; ")}. ` +
          "`make test-int` ran the HeadCanonical binary twice in the same way: once in the workspace selection, once package-scoped under the kill-window filter.",
  );
  if (canaryBinaries.size > 0) {
    notes.push(`Partitions, not duplicates: ${[...canaryBinaries].join(", ")} runs as count partitions, each test in one row.`);
  }
  notes.push(
    bins?.line
      ? `Bin harnesses: ${bins.line.run} tests ran in the "${BIN_ROW_NAME}" row (${bins.report.tests} listed, ${bins.report.ignored} ignored).`
      : `Bin harnesses: the "${BIN_ROW_NAME}" row reported no count.`,
  );
  let ok = bad === 0;
  if (rowsResult !== undefined && rowsResult !== "success") {
    ok = false;
    notes.push(`NOT OK: the rows job ended ${JSON.stringify(rowsResult)}, not "success".`);
  }
  const head = `## Nightly integration-fast rows: ${ok ? "every row accounted for" : "NOT accounted for"}`;
  return { ok, text: `${head}\n\n${lines.join("\n")}\n\n${notes.map((note) => `- ${note}`).join("\n")}\n` };
}

function option(args, name, required = true) {
  const at = args.indexOf(name);
  if (at === -1 || at + 1 >= args.length) {
    if (required) throw new SplitError(`missing ${name}`);
    return undefined;
  }
  return args[at + 1];
}

function main(argv) {
  const [command, ...args] = argv;
  if (command === "matrix") {
    const { matrix, rows } = nightlyRows(readJson(args[0] ?? ""));
    const out = option(args, "--rows");
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, `${JSON.stringify(rows, null, 1)}\n`);
    process.stdout.write(`${JSON.stringify(matrix)}\n`);
    process.stderr.write(`nightly integration-fast rows: ${rows.length} (${rows.map((row) => `${row.name}: ${row.binaries.length}`).join("; ")})\n`);
    return 0;
  }
  if (command === "report") {
    const row = {
      id: option(args, "--row"),
      name: option(args, "--name"),
      kind: option(args, "--kind"),
      binaries: option(args, "--binaries").split(/\s+/).filter(Boolean),
    };
    if (row.binaries.length === 0) throw new SplitError("--binaries names no binary");
    const listPath = option(args, "--list");
    const logPath = option(args, "--log");
    let list = null;
    try {
      list = existsSync(listPath) ? readJson(listPath) : null;
    } catch {
      list = null;
    }
    const report = rowReport({
      row,
      list,
      log: existsSync(logPath) ? readFileSync(logPath, "utf8") : null,
      exitCode: Number(option(args, "--exit")),
    });
    const out = option(args, "--out");
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, `${JSON.stringify(report, null, 1)}\n`);
    const problems = rowProblems(row, report);
    process.stdout.write(
      `row report "${row.name}": ${report.selected} selected, ${report.ignored} ignored, ${report.filtered} filtered out; ` +
        `${report.count_line ? report.count_line.line : "no count line"}; ${problems.length === 0 ? "ok" : `NOT OK: ${problems.join("; ")}`}\n`,
    );
    return 0;
  }
  if (command === "aggregate") {
    const result = aggregate({
      rowsDir: option(args, "--rows-dir"),
      reportsDir: option(args, "--reports-dir"),
      rowsResult: option(args, "--rows-result", false),
    });
    process.stdout.write(result.text);
    const summary = option(args, "--summary", false);
    if (summary) writeFileSync(summary, result.text, { flag: "a" });
    return result.ok ? 0 : 1;
  }
  throw new SplitError("usage: ci-nightly-int-split.mjs matrix|report|aggregate ... (see the head of the file)");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`ci-nightly-int-split: ${error?.message ?? error}\n`);
    process.exitCode = 2;
  }
}
