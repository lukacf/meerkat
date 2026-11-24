#!/usr/bin/env node
// Contracts of the nightly workflow's two example jobs.
//
// example-standalone runs the standalone example workspaces 034 and 035
// (the two targets of the examples Makefile's `standalone`, then example
// 035's script fixtures) and meta-example-006 runs Meta example pack 006's
// keyless script. Both are measured and reported: they stand in nightly.yml,
// at the end of one of its chains, and in no push or pull-request workflow,
// so no gate reads them and a failing example test cannot hold a release.
// Each timed step writes into the job summary when it started and ended and
// each part's seconds, a build apart from the rest, and exits with its
// commands' failure; this file runs both steps' scripts with stand-ins.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { accessSync, chmodSync, constants, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(resolve(root, path), "utf8");
const nightly = read(".github/workflows/nightly.yml");
const CONDITION = "    if: ${{ !cancelled() && !inputs.graph_only }}";

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
// (which also keeps them out of a graph-only dispatch).
const standalone = jobLines(nightly, "example-standalone");
const meta = jobLines(nightly, "meta-example-006");
assert.ok(standalone.includes("    name: Standalone examples"));
assert.ok(standalone.includes("    needs: meta-docs-lag"), "the standalone examples join the release-packaging chain's end");
assert.ok(standalone.includes(CONDITION), "they run every night, after a red predecessor too, and not on a graph-only dispatch");
assert.ok(meta.includes("    needs: example-standalone"), "pack 006 follows them in the same chain");
assert.ok(meta.includes(CONDITION));
const needs = nightly.split("\n").filter((line) => /^    needs: /.test(line));
assert.equal(needs.filter((line) => line === "    needs: example-standalone").length, 1, "only pack 006 waits for the standalone examples");
assert.equal(needs.filter((line) => line.includes("meta-example-006")).length, 0, "no job waits for pack 006: no gate reads either job");

// 2. Nightly only: no push or pull-request workflow runs them or names them.
for (const [path, absent] of [
  [".github/workflows/ci.yml", [/make -C examples (standalone|codemob|mdm)/, /Standalone examples/, /example-standalone/]],
  [".github/workflows/meta-ci.yml", [/006-long-term-agents/]],
]) {
  for (const pattern of absent) assert.doesNotMatch(read(path), pattern, `${path} must not hold ${pattern}: these examples are measured nightly and gate nothing`);
}

// 3. The steps: nothing lets a test step be skipped or fail unseen. The
// only tolerated failure in either job is the compiler cache's install.
for (const [name, job, steps] of [
  ["example-standalone", standalone, ["Standalone example tests (034 and 035), timed", "Example 035 script fixtures"]],
  ["meta-example-006", meta, ["Long-term agents example (keyless), timed"]],
]) {
  assert.deepEqual(job.filter((line) => line.includes("continue-on-error")), ["        continue-on-error: true"], `${name}: one tolerated step`);
  assert.deepEqual(
    job.filter((line) => /^        if: /.test(line)),
    ["        if: ${{ steps.sccache.outcome == 'failure' }}"],
    `${name}: one conditional step, the compile without the cache`,
  );
  for (const step of steps) {
    const lines = stepLines(job, step);
    assert.ok(!lines.some((line) => /continue-on-error|^        if: /.test(line)), `${name}: "${step}" always runs and its failure fails the job`);
  }
  assert.equal(job.indexOf(`      - name: ${steps[steps.length - 1]}`), job.findLastIndex((line) => /^      - /.test(line)), `${name}: its last step is "${steps[steps.length - 1]}"`);
}
assert.deepEqual(stepLines(standalone, "Example 035 script fixtures"), [
  "      - name: Example 035 script fixtures",
  "        working-directory: examples/035-mdm-tux-rs",
  "        run: python3 scripts/test-scripts.py",
]);

// 4. Said where a reader of a red night looks: the failure route, the two
// owners, and what these jobs do not run. In the workflow's comment and in
// the reference page's row.
const told = nightly.slice(nightly.indexOf("  # Examples 034 and 035 are Cargo workspaces of their own"), nightly.indexOf("  example-standalone:\n")).replace(/\n\s*# ?/g, " ");
const page = read("docs/reference/build-and-ci.mdx");
const row = page.split("\n").find((line) => line.includes("`make -C examples standalone`"));
assert.ok(row, "the reference page has the nightly examples' row");
for (const [label, text] of [["the workflow's comment", told], ["the reference page's row", row]]) {
  assert.match(text, /"Nightly failures" issue/, `${label} names where a failure is reported`);
  assert.match(text, /Examples owner's/, `${label} names the owner of a failing example test`);
  assert.match(text, /CI owner's/, `${label} names the owner of a runner, workflow or environment failure`);
  assert.match(text, /locked all-target Clippy/, `${label} says the lint is not run`);
  assert.match(text, /gets no result of these tests before it merges/, `${label} says what a pull request does not get`);
}
assert.match(told, /names no one/, "the comment says the failure workflow notifies no owner");
// What the comment says of the failure workflow stays true: one line with
// the run's address and the failed jobs' names, and no mention.
const failure = read(".github/workflows/nightly-failure-issue.yml");
assert.ok(failure.includes('body="Run ${GITHUB_SERVER_URL}/${GH_REPO}/actions/runs/${RUN_ID} failed: ${failed}"'), "the failure workflow writes the run and the failed jobs' names");
assert.doesNotMatch(failure.replace(/^\s*#.*$/gm, ""), /@[A-Za-z0-9-]+|--assignee|--label/, "the failure workflow mentions, assigns and labels no one: update the example jobs' comment when it does");

// 5. The two targets the step runs are the examples Makefile's `standalone`,
// each with its example's own manifest and committed lock.
const examples = read("examples/Makefile");
assert.match(examples, /^standalone: codemob mdm$/m);
for (const [target, manifest] of [["codemob", "examples/034-codemob-mcp/Cargo.toml"], ["mdm", "examples/035-mdm-tux-rs/Cargo.toml"]]) {
  assert.ok(examples.includes(`${target}:\n\tcd "$(ROOT)" && "$(CARGO)" test --locked --manifest-path ${manifest}\n`), `${target} tests ${manifest}`);
}
const timedStep = stepLines(standalone, "Standalone example tests (034 and 035), timed");
assert.ok(timedStep.includes("        shell: bash"), "errexit and pipefail");
for (const command of ["            make -C examples codemob", "            make -C examples mdm"]) {
  assert.ok(timedStep.includes(command), `the step runs \`${command.trim()}\` as a whole line`);
}

// 6. The timed step, run as the runner runs a `shell: bash` step, with
// stand-ins for make and the compiler cache: both targets always run; the
// summary holds the start and end, each target's seconds with its build and
// the rest apart, Cargo's own "Finished" lines without colour codes, the
// total and the cache's own counts ("unknown" when it reports none); the
// step fails when a target does.
const bash = (text, cwd, env) => spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", text], { cwd, env, encoding: "utf8" });
const fakeMake = [
  "#!/usr/bin/env bash",
  'echo "$*" >> "${FAKE_CALLS}"',
  'builds=1; if [[ "${3-}" == mdm ]]; then builds=2; fi',
  "for ((build = 0; build < builds; build++)); do",
  "  echo '   Compiling example v0.0.0' >&2",
  "  printf '\\033[1m\\033[32m    Finished\\033[0m `test` profile [unoptimized] target(s) in 1m 02s\\n' >&2",
  "  echo 'test result: ok. 1 passed; 0 failed'",
  "done",
  'if [[ "${3-}" == "${FAKE_FAILS-}" ]]; then exit 2; fi',
  "",
].join("\n");
const fakeCache = (reports) => [
  "#!/usr/bin/env bash",
  ...(reports ? ["echo 'Compile requests                    15'", "echo 'Cache hits                          12'", "echo 'Cache hits (Rust)                   12'", "echo 'Cache misses                         3'"] : ["exit 1"]),
  "",
].join("\n");
const cargoLine = (target) => `| Cargo, in ${target}: Finished \`test\` profile [unoptimized] target(s) in 1m 02s | | |`;
for (const [label, fails, reports, status, results, cache] of [
  ["both targets pass", "", true, 0, ["passed", "passed"], "hits 12, misses 3"],
  ["example 034 fails", "codemob", true, 1, ["failed", "passed"], "hits 12, misses 3"],
  ["example 035 fails and the cache reports nothing", "mdm", false, 1, ["passed", "failed"], "unknown"],
]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    mkdirSync(join(scratch, "bin"));
    for (const [name, text] of [["make", fakeMake], ["sccache", fakeCache(reports)]]) {
      writeFileSync(join(scratch, "bin", name), text);
      chmodSync(join(scratch, "bin", name), 0o755);
    }
    const env = {
      PATH: `${join(scratch, "bin")}:${process.env.PATH}`, RUNNER_TEMP: scratch, GITHUB_STEP_SUMMARY: join(scratch, "summary"),
      FAKE_CALLS: join(scratch, "calls"), FAKE_FAILS: fails,
    };
    const run = bash(script(timedStep), scratch, env);
    assert.equal(run.status, status, `${label}: the step's status (${run.stderr})`);
    assert.deepEqual(readFileSync(env.FAKE_CALLS, "utf8").trim().split("\n"), ["-C examples codemob", "-C examples mdm"], `${label}: both targets run`);
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, "utf8").replace(/\n+$/, "").split("\n");
    const seconds = (text, result) => new RegExp(`^\\| ${text} \\| \\d+ \\| ${result} \\|$`);
    const expected = [
      "### Standalone examples",
      "",
      /^started \d\d:\d\d:\d\dZ \(UTC\); queue time is not printed here: read it from the job record \(created to started\)$/,
      "",
      "| part | seconds | result |",
      "| --- | --- | --- |",
      seconds("make -C examples codemob", results[0]),
      /^\| make -C examples codemob: build \| \d+ \| \|$/,
      /^\| make -C examples codemob: tests and the rest \| \d+ \| \|$/,
      cargoLine("codemob"),
      seconds("make -C examples mdm", results[1]),
      /^\| make -C examples mdm: build \| \d+ \| \|$/,
      /^\| make -C examples mdm: tests and the rest \| \d+ \| \|$/,
      cargoLine("mdm"),
      cargoLine("mdm"),
      /^\| both targets \| \d+ \| \|$/,
      `| compiler cache, the whole job | ${cache} | |`,
      "",
      /^ended \d\d:\d\d:\d\dZ \(UTC\)$/,
    ];
    assert.equal(summary.length, expected.length, `${label}: the summary's lines:\n${summary.join("\n")}`);
    expected.forEach((want, index) => {
      if (want instanceof RegExp) assert.match(summary[index], want, `${label}: summary line ${index + 1}`);
      else assert.equal(summary[index], want, `${label}: summary line ${index + 1}`);
    });
    assert.equal((run.stdout.match(/test result: ok\. 1 passed/g) || []).length, 3, `${label}: the targets' output still reaches the log`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// 7. Pack 006's step, the same way: the gateway's build is timed apart from
// the pack's script, the script does not run after a failed build, and the
// step fails when either does.
const metaStep = stepLines(meta, "Long-term agents example (keyless), timed");
assert.ok(metaStep.includes("            ./scripts/repo-cargo build --locked -p meerkat-meta --bin rpc_gateway"), "the step builds the gateway through the root wrapper, as a whole line");
assert.ok(metaStep.includes("            meta/examples/006-long-term-agents/examples.sh"), "the step runs the pack's script as a whole line");
const packScript = resolve(root, "meta/examples/006-long-term-agents/examples.sh");
accessSync(packScript, constants.X_OK);
const packText = readFileSync(packScript, "utf8");
assert.ok(packText.includes('"$REPO_ROOT/scripts/repo-cargo" build --locked -p meerkat-meta --bin rpc_gateway'), "the script's own build of the gateway has the step's flags, so it compiles nothing after the step's");
assert.ok(packText.includes("No provider key is needed."), "the pack's script says it is keyless");
for (const key of ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
  assert.ok(!packText.includes(key), `the pack's script names no ${key}`);
}
assert.equal((packText.match(/^for run in 1 2; do$/gm) || []).length, 2, "the script runs each language's program twice");
for (const [label, buildCode, scriptCode, status, built, ran, calls] of [
  ["both pass", 0, 0, 0, "passed", "passed", ["build", "script"]],
  ["the gateway does not build", 101, 0, 1, "failed", "not run", ["build"]],
  ["the script fails", 0, 3, 1, "passed", "failed", ["build", "script"]],
]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    mkdirSync(join(scratch, "scripts"));
    mkdirSync(join(scratch, "meta/examples/006-long-term-agents"), { recursive: true });
    const log = join(scratch, "calls");
    for (const [path, word, code] of [["scripts/repo-cargo", "build", buildCode], ["meta/examples/006-long-term-agents/examples.sh", "script", scriptCode]]) {
      writeFileSync(join(scratch, path), `#!/usr/bin/env bash\necho ${word} >> "${log}"\necho "$*" >> "${log}.arguments"\nexit ${code}\n`);
      chmodSync(join(scratch, path), 0o755);
    }
    const env = { PATH: process.env.PATH, GITHUB_STEP_SUMMARY: join(scratch, "summary") };
    const run = bash(script(metaStep), scratch, env);
    assert.equal(run.status, status, `${label}: the step's status (${run.stderr})`);
    assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), calls, `${label}: what ran`);
    assert.equal(readFileSync(`${log}.arguments`, "utf8").split("\n")[0], "build --locked -p meerkat-meta --bin rpc_gateway", `${label}: the wrapper's arguments`);
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, "utf8").replace(/\n+$/, "").split("\n");
    assert.equal(summary[0], "### Meta long-term agents example");
    assert.match(summary[2], /^started \d\d:\d\d:\d\dZ \(UTC\); queue time is not printed here: read it from the job record \(created to started\)$/);
    assert.match(summary[6], new RegExp(`^\\| gateway build \\| \\d+ \\| ${built} \\|$`), label);
    assert.match(summary[7], new RegExp(`^\\| the script: Python and TypeScript, twice each \\| \\d+ \\| ${ran} \\|$`), label);
    assert.match(summary[9], /^ended \d\d:\d\d:\d\dZ \(UTC\)$/);
    assert.equal(summary.length, 10, label);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

console.log("nightly example jobs: contracts hold");
