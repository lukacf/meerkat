#!/usr/bin/env node
// Focused source/fixture checks only. Do not invoke the broad planner, Cargo,
// a provider or a hosted workflow. Budget requests go to an owned fake gh.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { EXAMPLE_035_COMMANDS, EXAMPLE_035_MANIFEST } from "./ci-standalone-example.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const text = (path) => readFileSync(resolve(root, path), "utf8");
const workflow = text(".github/workflows/ci.yml");
const planner = text("scripts/ci-cargo-lanes.mjs");
const projection = text("scripts/ci-release-projection-selftest.mjs");
const gateModel = text("scripts/ci-gate-consistency-selftest.mjs");
const scratch = mkdtempSync(join(tmpdir(), "standalone035-timeout-"));
function between(source, start, end) {
  const a = source.indexOf(start); assert(a >= 0, start);
  const b = source.indexOf(end, a + start.length); assert(b >= 0, end);
  return source.slice(a, b);
}
let count = 0;
function check(label, fn) { fn(); console.log(`timeout contract ${++count}: ${label}`); }
const helper = { assert };
vm.runInNewContext(between(projection, "function extractStepEnv(", '\nconsole.log("release-projection'), helper);
const gateRun = helper.extractStepRun(workflow, "Check lane results");
const base = { BUMP_PARENT: "a".repeat(40), RUN_ID: 4242 };
vm.runInNewContext(between(projection, "  const gateEnv = {", "  const gate = (change)") + "\nthis.env = waveEnv(gateEnv);", base);
const gate = (change) => spawnSync("bash", ["-c", gateRun], {
  env: { PATH: process.env.PATH, ...base.env, MODE: "none", RELEASE_PROJECTION: "false", EXAMPLE_WEB_RESULT: "success", WASM_TIMERS_RESULT: "success", ...change },
  encoding: "utf8",
});
try {
  check("actual planner block emits event-specific commands, deferral and strict flags", () => {
    const block = between(planner, "  if (!releaseProjection) {\n    selectExample035Dependencies", "  // Core's own lanes");
    const parser = {};
    vm.runInNewContext(between(planner, "function parseArgs(argv) {", "\nfunction git(argsList"), parser);
    for (const requested of ["pull_request", "push", "workflow_dispatch"]) for (const selected of [false, true]) for (const releaseProjection of [false, true]) {
      const event = parser.parseArgs(["--event", requested]).event;
      const c = { args: { event }, releaseProjection, deferToPush: event === "pull_request" && !releaseProjection,
        standalone: { selected, reasons: ["bound input"], deferredReasons: [] },
        result: { mode: "packages", packages: ["untouched"], unrelated: ["sentinel"] },
        deferred: { unrelated: 7 }, selectExample035Dependencies() {}, EXAMPLE_035_COMMANDS, EXAMPLE_035_MANIFEST };
      vm.runInNewContext(block, c);
      const tests = selected && !releaseProjection;
      assert.equal(c.result.example_035, tests);
      assert.equal(c.result.example_035_lint, tests && event === "push");
      assert.equal(c.deferred.example_035_clippy, tests && event === "pull_request");
      assert.deepEqual(Array.from(c.result.example_035_commands), !tests ? [] : event === "push" ? EXAMPLE_035_COMMANDS : EXAMPLE_035_COMMANDS.slice(1));
      assert.deepEqual(c.result.unrelated, ["sentinel"]); assert.deepEqual(c.result.packages, ["untouched"]); assert.equal(c.deferred.unrelated, 7);
      const d = { ...c };
      vm.runInNewContext(between(planner, "  const deferred = {", "  const downstreamRows"), d);
      vm.runInNewContext(between(planner, "function deferredSummary(", "\nfunction githubOutput("), d);
      const summary = d.deferredSummary({ closure_test_rows: 0, integration_suites: [], reader_rows: [], unit_feature_suites: [], clippy_packages: [], clippy_test_targets: [], example_035_clippy: c.deferred.example_035_clippy });
      if (tests && event === "pull_request") {
        assert.match(summary, /not linted on this pull request/); assert.match(summary, /hosted lint follows merge on main push/);
        assert(summary.includes(EXAMPLE_035_COMMANDS[0]));
      } else assert.equal(summary, "");
    }
    assert.equal(parser.parseArgs([]).event, "push"); assert.throws(() => parser.parseArgs(["--event", "invalid"]));
  });

  check("actual job condition and gate model allow ended lint but not workflow cancellation", () => {
    const c = { assert };
    vm.runInNewContext(between(gateModel, "function readJobs(", "// --- The planner, the jobs and the gate. ---"), c);
    const jobs = c.readJobs(workflow), test = jobs.find((j) => j.name === "example-035-tests");
    const outputNames = jobs.find((j) => j.name === "changes").outputs;
    assert.deepEqual(Array.from(test.needs), ["changes", "example-035"]);
    let rows = 0;
    for (const event of ["pull_request", "push", "workflow_dispatch", "unknown"]) for (const lint of ["success", "failure", "cancelled", "skipped", "", "in_progress"]) for (const selected of ["true", "false"]) for (const cancelled of [false, true]) for (const changes of ["success", "failure"]) {
      const outputs = new Map([["example_035", selected]]), results = new Map([["changes", changes], ["example-035", lint]]);
      const actual = c.evaluate(test.if, c.contextFor(event, outputNames, outputs, results, cancelled));
      const expected = !cancelled && changes === "success" && selected === "true" && (event === "pull_request" ? lint === "skipped" : ["push", "workflow_dispatch"].includes(event) && ["success", "failure", "cancelled"].includes(lint));
      assert.equal(actual, expected, JSON.stringify({ event, lint, selected, cancelled, changes })); rows++;
    }
    assert.equal(rows, 192);
    vm.runInNewContext(between(gateModel, "function simulate(", "function runGate("), c);
    const lintJob = jobs.find((j) => j.name === "example-035");
    const changesJob = jobs.find((j) => j.name === "changes");
    let lintRows = 0;
    for (const flag of ["true", "false", "", "TRUE", "1", "true\\n"]) for (const changes of ["success", "failure", "skipped"]) {
      const results = c.simulate([changesJob, lintJob], "push", outputNames, new Map([["example_035_lint", flag]]), new Map([["changes", changes]]));
      // GitHub string equality is case-insensitive; the gate separately rejects
      // noncanonical TRUE, so job condition acceptance cannot make it green.
      assert.equal(results.get("example-035"), changes === "success" && flag.toLowerCase() === "true" ? "success" : "skipped");
      lintRows++;
    }
    assert.equal(lintRows, 18);
    assert.match(test.if, /always\(\).*!cancelled\(\)/);
  });

  check("actual full gate rejects spoofed events and every missing required phase", () => {
    let rows = 0;
    for (const actual of ["pull_request", "push", "workflow_dispatch"]) for (const selected of ["true", "false"]) {
      const lintSelected = actual === "pull_request" ? "false" : selected;
      const env = { ACTUAL_EVENT: actual, PLAN_EVENT: actual === "pull_request" ? actual : "push", IS_PULL_REQUEST: String(actual === "pull_request"), IS_MAIN_PUSH: String(actual === "push"),
        EXAMPLE_035: selected, EXAMPLE_035_LINT: lintSelected };
      for (const tests of ["success", "failure", "cancelled", "skipped", ""]) for (const lint of ["success", "failure", "cancelled", "skipped", ""]) {
        const r = gate({ ...env, EXAMPLE_035_RESULT: tests, EXAMPLE_035_LINT_RESULT: lint });
        const pass = (selected === "true" ? tests === "success" : ["success", "skipped"].includes(tests)) && (lintSelected === "true" ? lint === "success" : ["success", "skipped"].includes(lint));
        assert.equal(r.status, pass ? 0 : 1, JSON.stringify({ env, tests, lint, stdout: r.stdout, stderr: r.stderr })); rows++;
      }
      const good = { ...env, EXAMPLE_035_RESULT: "success", EXAMPLE_035_LINT_RESULT: lintSelected === "true" ? "success" : "skipped" };
      for (const plan of ["", "unknown", actual === "pull_request" ? "push" : "pull_request"]) assert.equal(gate({ ...good, PLAN_EVENT: plan }).status, 1, "actual event wins with rust_changed=false");
      for (const key of ["EXAMPLE_035", "EXAMPLE_035_LINT"]) for (const bad of ["", "TRUE", "1", "unknown"]) assert.equal(gate({ ...good, [key]: bad }).status, 1);
      assert.equal(gate({ ...good, EXAMPLE_035: "false", EXAMPLE_035_LINT: "true" }).status, 1);
      assert.equal(gate({ ...good, EXAMPLE_035_LINT: actual === "pull_request" ? "true" : selected === "true" ? "false" : "true" }).status, 1);
    }
    assert.equal(rows, 150);
    assert.equal(gate({ ACTUAL_EVENT: "unknown", PLAN_EVENT: "unknown" }).status, 1);
    for (const key of ["EXAMPLE_035", "EXAMPLE_035_LINT"]) assert.equal(gate({ ...base.env, [key]: "true" }).status, 1, "projection forces both false");
  });

  check("existing budget code counts serial work and ignores absent or skipped PR lint", () => {
    const c = { assert, workflow, root, scratch, join, resolve, mkdirSync, writeFileSync, readFileSync, chmodSync, spawnSync, process, REPOSITORY: "fixture/repository", RUN_ID: 4242 };
    vm.runInNewContext(between(projection, "function extractStepEnv(", '\nconsole.log("release-projection'), c);
    vm.runInNewContext(between(projection, "  const budgetStep = ", "  // A pull request on a busy account:") + "\nthis.runBudget = runBudget; this.job = job; this.budgetEnv = budgetEnv; this.budgetStep = budgetStep;", c);
    assert.equal(c.budgetStep.trimEnd(), c.extractStepRun(text(".github/workflows/meta-ci.yml"), "Enforce lane execution budget").trimEnd());
    assert.equal(c.budgetEnv.LANE_UPSTREAMS, "Example035 standalone Rust + Python=Example035 standalone Clippy");
    const lintName = "Example035 standalone Clippy", testName = "Example035 standalone Rust + Python";
    const classification = c.job("Change classification", 0, 5, 35);
    const baseJobs = [classification, c.job(testName, 36, 46, 446), c.job("Unrelated", 36, 40, 140)];
    const absent = c.runBudget([0], baseJobs);
    const skipped = c.runBudget([0], [...baseJobs, { ...c.job(lintName, 36, 36, 36), conclusion: "skipped" }]);
    assert.equal(absent.status, 0, absent.stdout + absent.stderr);
    assert.equal(skipped.status, 0, skipped.stdout + skipped.stderr);
    assert.deepEqual(JSON.parse(JSON.stringify(absent.outputs)), JSON.parse(JSON.stringify(skipped.outputs)));
    assert.equal(absent.outputs.exec_path_seconds, "430"); assert.equal(absent.outputs.queue_seconds, "15");
    const chain = (lintSeconds, testSeconds, lintConclusion = "success", delay = 0) => [classification,
      { ...c.job(lintName, 36, 46, 46 + lintSeconds), conclusion: lintConclusion },
      c.job(testName, 47 + lintSeconds + delay, 57 + lintSeconds + delay, 57 + lintSeconds + delay + testSeconds),
      c.job("Unrelated", 36, 40, 140)];
    const summed = c.runBudget([0], chain(500, 400));
    assert.equal(summed.status, 0, summed.stdout + summed.stderr); assert.equal(summed.outputs.exec_path_seconds, "930"); assert.equal(summed.outputs.queue_seconds, "25");
    for (const conclusion of ["success", "failure", "cancelled"]) {
      const over = c.runBudget([0], chain(600, 500, conclusion));
      assert.equal(over.status, 1); assert.equal(over.outputs.exec_path_seconds, "1130");
    }
    const ceiling = c.runBudget([0], chain(100, 100, "success", 1300));
    assert.equal(ceiling.status, 1); assert.equal(ceiling.outputs.exec_path_seconds, "230");
    assert(Number(ceiling.outputs.terminal_excluding_queue_seconds) > 1200);
    assert.match(summed.stdout, /\| Unrelated \| 1 \| 4 \| 100 \| 130 \|/);
  });

  check("docs state the PR lint deferral and exact local target", () => {
    const docs = text("docs/reference/build-and-ci.mdx");
    for (const line of ["An example-only pull request does not lint the example.", "merge on the ordinary main push", EXAMPLE_035_COMMANDS[0]]) assert(docs.includes(line), line);
  });
  console.log(`${count} timeout contract groups passed; no broad planner, native or hosted execution`);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
