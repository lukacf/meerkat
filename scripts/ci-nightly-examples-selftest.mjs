#!/usr/bin/env node
// Contracts of the nightly workflow's two example jobs.
//
// example-standalone runs the standalone example workspaces 034 and 035
// (the two targets of the examples Makefile's `standalone`, then example
// 035's script fixtures) and meta-example-006 runs Meta example pack 006's
// keyless script. Both are measured and reported: they stand in nightly.yml,
// at the end of one of its chains, and in no push or pull-request workflow,
// so no gate reads them and a failing example test cannot hold a release.
//
// This file runs as the first step of those two jobs and in no push or
// pull-request lane (the Makefile's lane self-test target does not name it):
// it pins files of the examples, and a change of an example must not be
// able to fail the CI gate through it. Run it by hand after a change of
// the two jobs: node scripts/ci-nightly-examples-selftest.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  accessSync, chmodSync, constants, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(resolve(root, path), "utf8");
const nightly = read(".github/workflows/nightly.yml");
const nightlyLines = nightly.split("\n");
const CONDITION = "    if: ${{ !cancelled() && !inputs.graph_only }}";
const SELF = "scripts/ci-nightly-examples-selftest.mjs";
const NAMES = ["Standalone examples", "Meta long-term agents example (keyless)"];
const TIMED = "Standalone example tests (034 and 035) and 035's script fixtures, timed";
const PACK = "Long-term agents example (keyless), timed";

function jobLines(workflow, job) {
  const lines = workflow.split("\n");
  const start = lines.indexOf(`  ${job}:`);
  assert.ok(start >= 0, `nightly.yml has the job ${job}`);
  const next = lines.findIndex((line, index) => index > start && /^  [A-Za-z0-9_-]+:\s*$/.test(line));
  return lines.slice(start, next === -1 ? lines.length : next);
}

function stepLines(job, name) {
  const start = job.indexOf(`      - name: ${name}`);
  assert.ok(start >= 0, `the job has the step "${name}"`);
  const next = job.findIndex((line, index) => index > start && (/^      - /.test(line) || /^  \S/.test(line)));
  const lines = job.slice(start, next === -1 ? job.length : next);
  while (lines.length && lines[lines.length - 1].trim() === "") lines.pop();
  return lines;
}

function script(step) {
  const at = step.indexOf("        run: |");
  assert.ok(at >= 0, "the step has a script");
  return step.slice(at + 1).map((line) => line.slice(10)).join("\n") + "\n";
}

// 1. Where the jobs stand: the end of one chain, under the nightly condition
// (which also keeps them out of a graph-only dispatch), on the one schedule.
const standalone = jobLines(nightly, "example-standalone");
const meta = jobLines(nightly, "meta-example-006");
assert.ok(standalone.includes(`    name: ${NAMES[0]}`));
assert.ok(meta.includes(`    name: ${NAMES[1]}`));
assert.ok(standalone.includes("    needs: meta-docs-lag"), "the standalone examples join the release-packaging chain's end");
assert.ok(standalone.includes(CONDITION), "they run every night, after a red predecessor too, and not on a graph-only dispatch");
assert.ok(meta.includes("    needs: example-standalone"), "pack 006 follows them in the same chain");
assert.ok(meta.includes(CONDITION));
// Every line of the workflow outside its comments that names a job id,
// whatever its form: one job waits for the standalone examples and none for
// pack 006 (a list item or an inline list would be a line here too).
const naming = (id) => nightlyLines.filter((line) => line.includes(id) && !/^\s*#/.test(line));
assert.deepEqual(
  naming("example-standalone"),
  ["  example-standalone:", "          shared-key: nightly-example-standalone", "    needs: example-standalone"],
  "only pack 006 waits for the standalone examples",
);
assert.deepEqual(
  naming("meta-example-006"),
  ["  meta-example-006:", "          shared-key: nightly-meta-example-006"],
  "no job waits for pack 006: no gate reads either job",
);
const onAt = nightlyLines.indexOf("on:");
const onEnd = nightlyLines.findIndex((line, index) => index > onAt && /^[A-Za-z]/.test(line));
assert.deepEqual(
  nightlyLines.slice(onAt + 1, onEnd).filter((line) => /^  [a-z_]+:/.test(line)).map((line) => line.trim()),
  ["schedule:", "workflow_dispatch:"],
  "the nightly workflow starts on its schedule and by hand, and on no push or pull request",
);
assert.equal(nightlyLines.filter((line) => /^\s*- cron:/.test(line)).length, 1, "one schedule");
// No environment that reaches the two jobs names a secret. Every job
// inherits the workflow's top-level environment: it holds no secret or token
// expression and no name of a credential. The jobs' own environment is
// pinned below (the compiler wrapper alone, and no step has one).
const envAt = nightlyLines.indexOf("env:");
const envEnd = nightlyLines.findIndex((line, index) => index > envAt && /^[A-Za-z]/.test(line));
const inherited = nightlyLines.slice(envAt + 1, envEnd).filter((line) => /^  [A-Za-z_][A-Za-z0-9_]*:/.test(line));
assert.ok(envAt > 0 && inherited.length > 0, "the nightly workflow has a top-level environment");
for (const line of inherited) {
  assert.doesNotMatch(line, /secrets\.|github\.token/, `the workflow's top-level environment names no secret or token: ${line.trim()}`);
  assert.doesNotMatch(line.trim().split(":")[0], /KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL/i, `the workflow's top-level environment has no name of a credential: ${line.trim()}`);
}
for (const [name, job, limit] of [["example-standalone", standalone, 150], ["meta-example-006", meta, 90]]) {
  assert.deepEqual(job.filter((line) => /timeout-minutes/.test(line)), [`    timeout-minutes: ${limit}`], `${name}: its limit (set it from the first nights' summaries, here too)`);
  // The checkout's read and nothing else: not the workflow's identity token.
  const at = job.indexOf("    permissions:");
  assert.ok(at > 0 && job[at + 1] === "      contents: read" && !/^      \S/.test(job[at + 2]), `${name}: permissions are contents: read alone`);
  assert.equal(job.filter((line) => /^\s*permissions:/.test(line)).length, 1, `${name}: one permissions block`);
  assert.ok(!job.some((line) => /write-all|read-all|id-token|secrets\./.test(line) && !/^\s*#/.test(line)), `${name}: no wider permission and no secret`);
  const env = job.indexOf("    env:");
  assert.deepEqual(job.slice(env + 1, job.indexOf("    steps:")), ["      RUSTC_WRAPPER: sccache"], `${name}: its environment is the compiler wrapper alone`);
  assert.equal(job.filter((line) => /^\s+env:/.test(line)).length, 1, `${name}: no step has an environment of its own`);
}

// 2. Nightly only: no other workflow runs them or names them, and the lane
// self-test target that every push and pull request runs does not run this
// file. Both jobs run it themselves, first after the checkout.
const workflows = readdirSync(resolve(root, ".github/workflows")).filter((name) => /\.ya?ml$/.test(name) && name !== "nightly.yml");
assert.ok(workflows.includes("ci.yml") && workflows.includes("meta-ci.yml"));
for (const name of workflows) {
  const text = read(`.github/workflows/${name}`);
  for (const pattern of [/make -C examples\b/, /examples\/03[45]-/, /006-long-term-agents/, /example-standalone/, /meta-example-006/, /ci-nightly-examples-selftest/]) {
    assert.doesNotMatch(text, pattern, `${name} must not hold ${pattern}: these examples are measured nightly and gate nothing`);
  }
  if (name !== "nightly-failure-issue.yml") {
    for (const job of NAMES) assert.ok(!text.includes(job), `${name} must not name the job "${job}"`);
  }
}
assert.ok(!read("Makefile").includes("ci-nightly-examples-selftest"), "the Makefile must not run this file: its lane self-test target is required by the CI gate");
for (const [name, job] of [["example-standalone", standalone], ["meta-example-006", meta]]) {
  const steps = job.filter((line) => /^      - /.test(line));
  assert.equal(steps[0], "      - name: Checkout", `${name}: the checkout comes first`);
  assert.deepEqual(
    stepLines(job, "Contracts of the nightly example jobs"),
    ["      - name: Contracts of the nightly example jobs", `        run: node ${SELF}`],
    `${name}: this file runs as a step`,
  );
  assert.equal(steps[1], "      - name: Contracts of the nightly example jobs", `${name}: and right after the checkout`);
}

// 3. The steps: nothing lets a test step be skipped or fail unseen. The
// only tolerated failure in either job is the compiler cache's install.
for (const [name, job, step] of [["example-standalone", standalone, TIMED], ["meta-example-006", meta, PACK]]) {
  assert.deepEqual(job.filter((line) => line.includes("continue-on-error")), ["        continue-on-error: true"], `${name}: one tolerated step`);
  assert.deepEqual(
    job.filter((line) => /^\s+if: /.test(line)),
    [CONDITION, "        if: ${{ steps.sccache.outcome == 'failure' }}"],
    `${name}: the job's condition and one conditional step, the compile without the cache`,
  );
  const lines = stepLines(job, step);
  assert.ok(!lines.some((line) => /continue-on-error|^        if: /.test(line)), `${name}: "${step}" always runs and its failure fails the job`);
  assert.equal(lines[1], "        shell: bash", `${name}: "${step}" runs under bash with errexit and pipefail`);
  assert.equal(lines.filter((line) => /^        [a-z-]+:/.test(line)).join("|"), "        shell: bash|        run: |", `${name}: "${step}" has a shell and a script and no other key`);
  assert.equal(job.indexOf(`      - name: ${step}`), job.findLastIndex((line) => /^      - /.test(line)), `${name}: its last step is "${step}"`);
}

// 4. Said where a reader of a red night looks: the route of a failure and
// what these jobs do not run. In each job's comment, in the reference
// page's row and in the failure workflow's own words.
const comment = (from, to) => nightly.slice(nightly.indexOf(from), nightly.indexOf(to)).replace(/\n\s*# ?/g, " ");
const told = comment("  # Examples 034 and 035 are Cargo workspaces of their own", "  example-standalone:\n");
const toldMeta = comment("  # Meta's example pack 006 (long-term agents)", "  meta-example-006:\n");
const page = read("docs/reference/build-and-ci.mdx");
const row = page.split("\n").find((line) => line.includes("`make -C examples standalone`"));
assert.ok(row, "the reference page has the nightly examples' row");
for (const [label, text, patterns] of [
  ["the standalone job's comment", told, [
    /"Nightly failures" issue/, /The CI owner reads the nightly run's result and that issue/, /stays with the CI owner/,
    /reports to the maintainers with the run, the job, the revision and the failing command or assertion/,
    /they route it to the Examples owner, who returns a fix or a blocker and reads the issue in their regular review as a backstop/,
    /Nothing mentions or assigns anyone/, /gets no result of these tests before it merges/,
    /any lint of examples 034 and 035/, /`make check`, an all-target cargo check, is not run either/, /any feature set but the default/,
    /container suites/, /the examples Makefile's other targets/, /in no push or pull-request lane/,
  ]],
  ["pack 006's comment", toldMeta, [
    /read by no gate/, /stays with the CI owner/, /a content failure goes through the maintainers to the Examples owner/,
    /Not run here: every other Meta example pack/,
  ]],
  ["the reference page's row", row, [
    /"Nightly failures" issue/, /stays with the CI owner/, /to the maintainers with the run, the job, the revision and the failing command or assertion/,
    /Examples owner/, /gets no result of these tests before it merges/, /any lint of 034 and 035/, /all-target check/,
    /any feature set but the default/, /container suites/, /every other Meta example pack/,
    /queue seconds are not printed there: a first night's evaluation reads them from the job record, created to started/,
  ]],
]) {
  for (const pattern of patterns) assert.match(text, pattern, `${label} says ${pattern}`);
}
// The failure workflow: its trigger and rights as they were, one line for
// every failed job, the route's words for these two, and nobody mentioned,
// assigned or labelled.
const failure = read(".github/workflows/nightly-failure-issue.yml");
assert.ok(failure.includes("on:\n  workflow_run:\n    workflows: [Nightly]\n    types: [completed]\n\npermissions:\n  actions: read\n  issues: write\n\njobs:\n"), "the failure workflow's trigger and permissions");
assert.ok(failure.includes('body="Run ${GITHUB_SERVER_URL}/${GH_REPO}/actions/runs/${RUN_ID} failed: ${failed}"'), "the failure workflow writes the run and the failed jobs' names");
assert.ok(failure.includes(`            *", ${NAMES[0]}, "* | *", ${NAMES[1]}, "*)\n`), "the failure workflow knows the two jobs by their names");
assert.doesNotMatch(failure.replace(/^\s*#.*$/gm, ""), /@[A-Za-z0-9-]+|--assignee|--label|--add-/, "the failure workflow mentions, assigns and labels no one");
const failureJob = jobLines(failure, "failure-issue");
const failureScript = script(stepLines(failureJob, "Open or update the tracking issue"));
const fakeGh = [
  "#!/usr/bin/env bash",
  'if [[ "$1" == api ]]; then cat "${FAKE_JOBS}"; exit 0; fi',
  'if [[ "$1 $2" == "issue list" ]]; then echo 1913; exit 0; fi',
  'if [[ "$1 $2" == "issue comment" ]]; then printf \'%s\' "$5" > "${FAKE_BODY}"; echo "$*" > "${FAKE_BODY}.arguments"; exit 0; fi',
  "exit 64",
  "",
].join("\n");
const ROUTE = "Example jobs: an infrastructure fault (runner, workflow, environment) stays with the CI owner. A content failure (a failing test or assertion of example 034, 035 or Meta example pack 006) is reported to the maintainers with the run, the job, the revision and the failing command or assertion, and routed to the Examples owner.";
for (const [label, jobs, body] of [
  ["a standalone failure beside another job's", [["Workspace unit tests", "failure"], [NAMES[0], "failure"], ["Lint", "success"]], `Run https://example.invalid/o/r/actions/runs/7 failed: Workspace unit tests, ${NAMES[0]}\n\n${ROUTE}`],
  ["pack 006 timed out", [[NAMES[1], "timed_out"]], `Run https://example.invalid/o/r/actions/runs/7 failed: ${NAMES[1]}\n\n${ROUTE}`],
  ["another job alone", [["Workspace unit tests", "cancelled"], [NAMES[0], "success"]], "Run https://example.invalid/o/r/actions/runs/7 failed: Workspace unit tests"],
  ["a job whose name only holds an example job's name", [[`${NAMES[0]} (old)`, "failure"]], `Run https://example.invalid/o/r/actions/runs/7 failed: ${NAMES[0]} (old)`],
  ["a green night", [[NAMES[0], "success"], [NAMES[1], "success"]], null],
]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    mkdirSync(join(scratch, "bin"));
    writeFileSync(join(scratch, "bin", "gh"), fakeGh);
    chmodSync(join(scratch, "bin", "gh"), 0o755);
    writeFileSync(join(scratch, "jobs.json"), JSON.stringify({ jobs: jobs.map(([name, conclusion]) => ({ name, conclusion })) }));
    const env = {
      PATH: `${join(scratch, "bin")}:${process.env.PATH}`, RUNNER_TEMP: scratch, GH_REPO: "o/r", RUN_ID: "7",
      GITHUB_SERVER_URL: "https://example.invalid", FAKE_JOBS: join(scratch, "jobs.json"), FAKE_BODY: join(scratch, "body"),
    };
    const run = spawnSync("bash", ["-e", "-c", failureScript], { cwd: scratch, env, encoding: "utf8" });
    assert.equal(run.status, 0, `${label}: the failure step's status (${run.stderr})`);
    if (body === null) {
      assert.ok(!existsSync(env.FAKE_BODY), `${label}: no comment`);
    } else {
      assert.equal(readFileSync(env.FAKE_BODY, "utf8"), body, `${label}: the comment`);
      assert.equal(readFileSync(`${env.FAKE_BODY}.arguments`, "utf8").split(" ").slice(0, 4).join(" "), "issue comment 1913 --body", `${label}: one comment on the open issue`);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// 5. The two targets the step runs are the examples Makefile's `standalone`,
// each with its example's own manifest and committed lock.
const examples = read("examples/Makefile");
assert.match(examples, /^standalone: codemob mdm$/m);
for (const [target, manifest] of [["codemob", "examples/034-codemob-mcp/Cargo.toml"], ["mdm", "examples/035-mdm-tux-rs/Cargo.toml"]]) {
  assert.ok(examples.includes(`${target}:\n\tcd "$(ROOT)" && "$(CARGO)" test --locked --manifest-path ${manifest}\n`), `${target} tests ${manifest}`);
}
accessSync(resolve(root, "examples/035-mdm-tux-rs/scripts/test-scripts.py"), constants.R_OK);
const timedStep = stepLines(standalone, TIMED);
for (const command of ["            make -C examples codemob", "            make -C examples mdm", "            (cd examples/035-mdm-tux-rs && python3 scripts/test-scripts.py)"]) {
  assert.ok(timedStep.includes(command), `the step runs \`${command.trim()}\` as a whole line`);
}

// 6. The timed step, run as the runner runs a `shell: bash` step, with
// stand-ins for make, python3 and the compiler cache. All three parts always
// run; the summary holds the start and end, each target's seconds with its
// build and its tests apart, Cargo's own "Finished" lines without colour
// codes, the fixtures, the total, and beside each build cold, warm or
// partly warm from the cache's counters ("unknown" when it reports none or
// they did not move); the step fails when a part does.
const bash = (text, cwd, env) => spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", text], { cwd, env, encoding: "utf8" });
const fakeMake = [
  "#!/usr/bin/env bash",
  'echo "$*" >> "${FAKE_CALLS}"',
  "finished() { printf '\\033[1m\\033[32m    Finished\\033[0m `test` profile [unoptimized] target(s) in 1m 02s\\n' >&2; }",
  // The slow target: one second of build, four of tests, then a Cargo call
  // that has nothing to build: its "Finished" line has no "Compiling" line
  // before it, and the four seconds before it are tests, not build.
  'if [[ -n "${FAKE_SLOW-}" && "${3-}" == mdm ]]; then',
  "  echo '   Compiling example v0.0.0' >&2",
  "  sleep 1",
  "  finished",
  "  echo 'test result: ok. 1 passed; 0 failed'",
  "  sleep 4",
  "  finished",
  "  echo 'test result: ok. 1 passed; 0 failed'",
  "  exit 0",
  "fi",
  'builds=1; if [[ "${3-}" == mdm ]]; then builds=2; fi',
  "for ((build = 0; build < builds; build++)); do",
  "  echo '   Compiling example v0.0.0' >&2",
  "  finished",
  "  echo 'test result: ok. 1 passed; 0 failed'",
  "done",
  'if [[ " ${FAKE_FAILS-} " == *" ${3-} "* ]]; then exit 2; fi',
  "",
].join("\n");
const fakePython = [
  "#!/usr/bin/env bash",
  'echo "python3 $* in $(basename "${PWD}")" >> "${FAKE_CALLS}"',
  'if [[ " ${FAKE_FAILS-} " == *" fixtures "* ]]; then exit 2; fi',
  "",
].join("\n");
// The cache's statistics, one "hits misses" pair of FAKE_CACHE per call, in
// sccache's own layout (with the lines that must not be taken for a counter).
const fakeCache = [
  "#!/usr/bin/env bash",
  'if [[ -z "${FAKE_CACHE-}" ]]; then exit 1; fi',
  'calls=0; if [[ -e "${FAKE_CACHE_CALLS}" ]]; then calls="$(cat "${FAKE_CACHE_CALLS}")"; fi',
  'echo $((calls + 1)) > "${FAKE_CACHE_CALLS}"',
  "IFS=';' read -r -a pairs <<< \"${FAKE_CACHE}\"",
  'pair="${pairs[calls]:-${pairs[-1]}}"',
  "echo 'Compile requests                    15'",
  'echo "Cache hits                          ${pair% *}"',
  "echo 'Cache hits (Rust)                   999'",
  'echo "Cache misses                        ${pair#* }"',
  "echo 'Cache misses (Rust)                 999'",
  "echo 'Cache hits rate                     80.00 %'",
  "",
].join("\n");
const cargoLine = (target) => `| Cargo, in ${target}: Finished \`test\` profile [unoptimized] target(s) in 1m 02s | | | |`;
const pause = (milliseconds) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
for (const [label, fails, cache, status, results, warmth, slow] of [
  ["all three parts pass", "", "0 0;0 300;0 300;200 300", 0, ["passed", "passed", "passed"], ["cold \\(hits 0, misses 300\\)", "warm \\(hits 200, misses 0\\)"]],
  ["example 034 fails", "codemob", "5 5;15 25;15 25;15 25", 1, ["failed", "passed", "passed"], ["partly warm \\(hits 10, misses 20\\)", "unknown"]],
  ["example 035 and the fixtures fail and the cache reports nothing", "mdm fixtures", "", 1, ["passed", "failed", "failed"], ["unknown", "unknown"]],
  ["the fixtures alone fail", "fixtures", "3 0;3 0;3 0;9 0", 1, ["passed", "passed", "failed"], ["unknown", "warm \\(hits 6, misses 0\\)"]],
  ["one second of build, four of tests, then a Cargo call with nothing to build", "", "", 0, ["passed", "passed", "passed"], ["unknown", "unknown"], true],
]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    mkdirSync(join(scratch, "bin"));
    mkdirSync(join(scratch, "examples/035-mdm-tux-rs"), { recursive: true });
    for (const [name, text] of [["make", fakeMake], ["sccache", fakeCache], ["python3", fakePython]]) {
      writeFileSync(join(scratch, "bin", name), text);
      chmodSync(join(scratch, "bin", name), 0o755);
    }
    const env = {
      PATH: `${join(scratch, "bin")}:${process.env.PATH}`, RUNNER_TEMP: scratch, GITHUB_STEP_SUMMARY: join(scratch, "summary"),
      FAKE_CALLS: join(scratch, "calls"), FAKE_FAILS: fails, FAKE_CACHE: cache, FAKE_CACHE_CALLS: join(scratch, "cache-calls"),
      FAKE_SLOW: slow ? "yes" : "",
    };
    // The step counts whole wall-clock seconds: start the slow case just
    // after a second begins, so that its one and four seconds are counted
    // as one and four.
    if (slow) pause(1030 - (Date.now() % 1000));
    const run = bash(script(timedStep), scratch, env);
    assert.equal(run.status, status, `${label}: the step's status (${run.stderr})`);
    assert.deepEqual(
      readFileSync(env.FAKE_CALLS, "utf8").trim().split("\n"),
      ["-C examples codemob", "-C examples mdm", "python3 scripts/test-scripts.py in 035-mdm-tux-rs"],
      `${label}: all three parts run`,
    );
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, "utf8").replace(/\n+$/, "").split("\n");
    const expected = [
      "### Standalone examples",
      "",
      /^started \d\d:\d\d:\d\dZ \(UTC\); queue time is not printed here: read it from the job record \(created to started\)$/,
      "",
      "| part | seconds | result | compiler cache |",
      "| --- | --- | --- | --- |",
      new RegExp(`^\\| make -C examples codemob \\| \\d+ \\| ${results[0]} \\| \\|$`),
      new RegExp(`^\\| make -C examples codemob: build \\| \\d+ \\| \\| ${warmth[0]} \\|$`),
      /^\| make -C examples codemob: tests \| \d+ \| \| \|$/,
      cargoLine("codemob"),
      new RegExp(`^\\| make -C examples mdm \\| \\d+ \\| ${results[1]} \\| \\|$`),
      new RegExp(`^\\| make -C examples mdm: build \\| \\d+ \\| \\| ${warmth[1]} \\|$`),
      /^\| make -C examples mdm: tests \| \d+ \| \| \|$/,
      cargoLine("mdm"),
      cargoLine("mdm"),
      new RegExp(`^\\| example 035 script fixtures \\| \\d+ \\| ${results[2]} \\| \\|$`),
      /^\| all three parts \| \d+ \| \| \|$/,
      "",
      /^ended \d\d:\d\d:\d\dZ \(UTC\)$/,
    ];
    assert.equal(summary.length, expected.length, `${label}: the summary's lines:\n${summary.join("\n")}`);
    expected.forEach((want, index) => {
      if (want instanceof RegExp) assert.match(summary[index], want, `${label}: summary line ${index + 1}`);
      else assert.equal(summary[index], want, `${label}: summary line ${index + 1}`);
    });
    assert.equal((run.stdout.match(/test result: ok\. 1 passed/g) || []).length, 3, `${label}: the targets' output still reaches the log`);
    if (slow) {
      // The four seconds of tests before the second "Finished" line are
      // tests. One second more is allowed for a second's boundary; a split
      // that counts them as build prints build 5 and tests 0.
      const seconds = (line) => Number(line.split("|")[2]);
      const [build, tests] = [seconds(summary[11]), seconds(summary[12])];
      assert.ok(build >= 1 && build <= 2 && tests >= 4 && tests <= 5, `${label}: build 1 and tests 4, printed build ${build} and tests ${tests}`);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// 7. Pack 006's step, the same way. The gateway's build is timed apart and
// says cold or warm; the script gets that binary, does not run after a
// failed build, and runs under the shell's command trace, from which the
// step sums the package installs and the four program runs apart; the
// trace is not printed; the step fails when the build or the script does.
const metaStep = stepLines(meta, PACK);
assert.ok(metaStep.includes("            ./scripts/repo-cargo build --locked -p meerkat-meta --bin rpc_gateway"), "the step builds the gateway through the root wrapper, as a whole line");
assert.ok(metaStep.includes("              bash -x meta/examples/006-long-term-agents/examples.sh 9> \"${trace}\""), "the step runs the pack's script under the shell's trace");
const packScript = resolve(root, "meta/examples/006-long-term-agents/examples.sh");
accessSync(packScript, constants.X_OK);
const packText = readFileSync(packScript, "utf8");
assert.ok(packText.includes('if [[ -z "${META_GATEWAY_BIN:-}" ]]; then'), "the script takes a gateway that is already built");
assert.ok(packText.includes("No provider key is needed."), "the pack's script says it is keyless");
for (const key of ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
  assert.ok(!packText.includes(key), `the pack's script names no ${key}`);
}
assert.equal((packText.match(/^for run in 1 2; do$/gm) || []).length, 2, "the script runs each language's program twice");
// The commands the step's sums know, as the script spells them.
for (const spelling of [
  'python3 -m venv "$WORK/venv"', '"$WORK/venv/bin/python" -m pip install ', "npm install --silent", "npm run build --silent", "npm ci --silent",
  '"$WORK/venv/bin/python" "$PACK_DIR/main.py" >', 'npx tsx "$PACK_DIR/main.ts" >',
]) {
  assert.ok(packText.includes(spelling), `the pack's script still runs \`${spelling}\`: the step's sums match its commands by these words`);
}
// A stand-in for the pack's script with the same command shapes; its tools
// are stand-ins too. With FAKE_SLOW one install and one program run take a
// second each, so both sums must be above zero.
const fakePack = [
  "#!/usr/bin/env bash",
  "set -euo pipefail",
  'echo script >> "${FAKE_LOG}"',
  '[[ -x "${META_GATEWAY_BIN:-}" ]] || { echo "no gateway" >&2; exit 9; }',
  'WORK="$(mktemp -d)"',
  "trap 'rm -rf \"$WORK\"' EXIT",
  'PACK_DIR="$(pwd)/pack"',
  'python3 -m venv "$WORK/venv"',
  '"$WORK/venv/bin/python" -m pip install --quiet sdk',
  "for run in 1 2; do",
  '  EXAMPLE_STATE_DIR="$WORK/state-py" "$WORK/venv/bin/python" "$PACK_DIR/main.py" > "$WORK/py-$run.log"',
  "done",
  "(cd . && npm install --silent --no-fund && npm run build --silent)",
  "(cd . && npm ci --silent --no-fund)",
  "for run in 1 2; do",
  '  (cd . && EXAMPLE_STATE_DIR="$WORK/state-ts" npx tsx "$PACK_DIR/main.ts" > "$WORK/ts-$run.log")',
  "done",
  'exit "${FAKE_SCRIPT_CODE}"',
  "",
].join("\n");
const fakeTool = [
  "#!/usr/bin/env bash",
  'name="$(basename "$0")"',
  'if [[ "${name}" == python3 && "${1-} ${2-}" == "-m venv" ]]; then mkdir -p "$3/bin" && cp "$0" "$3/bin/python"; exit 0; fi',
  'if [[ -n "${FAKE_SLOW-}" && "${name} ${1-}" == "npm ci" ]]; then sleep 1; fi',
  'if [[ -n "${FAKE_SLOW-}" && "${name} ${1-}" == "npx tsx" && ! -e "${FAKE_LOG}.slow" ]]; then : > "${FAKE_LOG}.slow"; sleep 1; fi',
  'echo "${name} $*"',
  "",
].join("\n");
for (const [label, buildCode, scriptCode, slow, cache, status, built, warmth, ran, calls] of [
  ["both pass", 0, 0, "yes", "0 0;0 900", 0, "passed", "cold (hits 0, misses 900)", "passed", ["build", "script"]],
  ["the gateway does not build", 101, 0, "", "7 7;7 7", 1, "failed", "unknown", "not run", ["build"]],
  ["the script fails", 0, 3, "", "", 1, "passed", "unknown", "failed", ["build", "script"]],
]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    for (const dir of ["bin", "scripts", "meta/examples/006-long-term-agents", "target/debug"]) mkdirSync(join(scratch, dir), { recursive: true });
    const log = join(scratch, "calls");
    writeFileSync(join(scratch, "scripts/repo-cargo"), [
      "#!/usr/bin/env bash",
      `if [[ "\${1-}" == --print-env ]]; then echo "CARGO_HOME=${scratch}/home"; echo "CARGO_TARGET_DIR=${scratch}/target"; exit 0; fi`,
      `echo build >> "${log}"`,
      `echo "$*" >> "${log}.arguments"`,
      `if [[ ${buildCode} == 0 ]]; then printf '#!/bin/sh\\n' > "${scratch}/target/debug/rpc_gateway" && chmod +x "${scratch}/target/debug/rpc_gateway"; fi`,
      `exit ${buildCode}`,
      "",
    ].join("\n"));
    writeFileSync(join(scratch, "meta/examples/006-long-term-agents/examples.sh"), fakePack);
    for (const tool of ["python3", "npm", "npx"]) writeFileSync(join(scratch, "bin", tool), fakeTool);
    writeFileSync(join(scratch, "bin/sccache"), fakeCache);
    for (const path of ["scripts/repo-cargo", "meta/examples/006-long-term-agents/examples.sh", "bin/python3", "bin/npm", "bin/npx", "bin/sccache"]) chmodSync(join(scratch, path), 0o755);
    const env = {
      PATH: `${join(scratch, "bin")}:${process.env.PATH}`, RUNNER_TEMP: scratch, GITHUB_STEP_SUMMARY: join(scratch, "summary"),
      FAKE_LOG: log, FAKE_SCRIPT_CODE: String(scriptCode), FAKE_SLOW: slow, FAKE_CACHE: cache, FAKE_CACHE_CALLS: join(scratch, "cache-calls"),
    };
    const run = bash(script(metaStep), scratch, env);
    assert.equal(run.status, status, `${label}: the step's status (${run.stderr})`);
    assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), calls, `${label}: what ran`);
    assert.equal(readFileSync(`${log}.arguments`, "utf8").split("\n")[0], "build --locked -p meerkat-meta --bin rpc_gateway", `${label}: the wrapper's arguments`);
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, "utf8").replace(/\n+$/, "").split("\n");
    const head = [
      "### Meta long-term agents example",
      "",
      /^started \d\d:\d\d:\d\dZ \(UTC\); queue time is not printed here: read it from the job record \(created to started\)$/,
      "",
      "| part | seconds | result | compiler cache |",
      "| --- | --- | --- | --- |",
      new RegExp(`^\\| gateway build \\| \\d+ \\| ${built} \\| ${warmth.replace(/[()]/g, "\\$&")} \\|$`),
      new RegExp(`^\\| the script: Python and TypeScript, twice each \\| \\d+ \\| ${ran} \\| \\|$`),
    ];
    const split = ran === "not run" ? [] : [
      /^\| the script: package installs \(the Python environment, pip, npm\) \| (\d+) \| \| \|$/,
      /^\| the script: the program runs \(4 of 4 seen\) \| (\d+) \| \| \|$/,
      /^\| the script: its other commands \| \d+ \| \| \|$/,
    ];
    const expected = [...head, ...split, "", /^ended \d\d:\d\d:\d\dZ \(UTC\)$/];
    assert.equal(summary.length, expected.length, `${label}: the summary's lines:\n${summary.join("\n")}`);
    expected.forEach((want, index) => {
      if (want instanceof RegExp) assert.match(summary[index], want, `${label}: summary line ${index + 1}`);
      else assert.equal(summary[index], want, `${label}: summary line ${index + 1}`);
    });
    if (slow) {
      for (const [index, part] of [[8, "the package installs"], [9, "the program runs"]]) {
        assert.ok(Number(summary[index].split("|")[2]) >= 1, `${label}: ${part} took a second and the summary counts it: ${summary[index]}`);
      }
    }
    const trace = join(scratch, "pack-006-trace");
    if (ran !== "not run") {
      assert.match(readFileSync(trace, "utf8"), /^\++ \d+ npm ci --silent --no-fund$/m, `${label}: the trace holds each command with its second`);
    }
    assert.doesNotMatch(run.stdout + run.stderr + summary.join("\n"), /-m venv|state-py/, `${label}: the trace is not printed`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

console.log("nightly example jobs: contracts hold");
