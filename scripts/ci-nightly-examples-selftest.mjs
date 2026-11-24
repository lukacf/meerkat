#!/usr/bin/env node
// Contracts of the nightly workflow's two example jobs.
//
// example-standalone runs the standalone example workspaces 034 and 035
// (the two targets of the examples Makefile's `standalone`, then example
// 035's script fixtures) and meta-example-006 runs Meta example pack 006's
// keyless script. Both are measured and reported: they stand in nightly.yml,
// at the end of one of its chains, and in no push or pull-request workflow,
// so no gate reads them and a failing example test cannot hold a release.
// Each timed step writes its seconds into the job summary and exits with its
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

// 1. Where the jobs stand: the end of one chain, under the nightly condition.
const standalone = jobLines(nightly, "example-standalone");
const meta = jobLines(nightly, "meta-example-006");
assert.ok(standalone.includes("    name: Standalone examples"));
assert.ok(standalone.includes("    needs: meta-docs-lag"), "the standalone examples join the release-packaging chain's end");
assert.ok(standalone.includes(CONDITION), "they run every night, after a red predecessor too");
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

// 4. The two targets the step runs are the examples Makefile's `standalone`,
// each with its example's own manifest and committed lock.
const examples = read("examples/Makefile");
assert.match(examples, /^standalone: codemob mdm$/m);
for (const [target, manifest] of [["codemob", "examples/034-codemob-mcp/Cargo.toml"], ["mdm", "examples/035-mdm-tux-rs/Cargo.toml"]]) {
  assert.ok(examples.includes(`${target}:\n\tcd "$(ROOT)" && "$(CARGO)" test --locked --manifest-path ${manifest}\n`), `${target} tests ${manifest}`);
}
const timedStep = stepLines(standalone, "Standalone example tests (034 and 035), timed");
assert.ok(timedStep.includes("        shell: bash"), "errexit and pipefail: a failing target is seen through the pipe");
for (const command of ["            make -C examples codemob", "            make -C examples mdm"]) {
  assert.ok(timedStep.includes(command), `the step runs \`${command.trim()}\` as a whole line`);
}

// 5. The timed step, run as the runner runs a `shell: bash` step, with a
// stand-in for make: both targets always run, each row and Cargo's own
// "Finished" lines reach the summary, and the step fails when a target does.
const bash = (text, cwd, env) => spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", text], { cwd, env, encoding: "utf8" });
const fakeMake = [
  "#!/usr/bin/env bash",
  'echo "$*" >> "${STAND_IN_CALLS}"',
  "echo '   Compiling example v0.0.0' >&2",
  "printf '\\033[1m\\033[32m    Finished\\033[0m `test` profile [unoptimized] target(s) in 1m 02s\\n' >&2",
  "echo 'test result: ok. 1 passed; 0 failed'",
  'if [[ "${3-}" == "${STAND_IN_FAILS-}" ]]; then exit 2; fi',
  "",
].join("\n");
for (const [label, fails, status, results] of [
  ["both targets pass", "", 0, ["passed", "passed"]],
  ["example 034 fails", "codemob", 1, ["failed", "passed"]],
  ["example 035 fails", "mdm", 1, ["passed", "failed"]],
]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    mkdirSync(join(scratch, "bin"));
    writeFileSync(join(scratch, "bin/make"), fakeMake);
    chmodSync(join(scratch, "bin/make"), 0o755);
    const env = {
      PATH: `${join(scratch, "bin")}:${process.env.PATH}`, RUNNER_TEMP: scratch, GITHUB_STEP_SUMMARY: join(scratch, "summary"),
      STAND_IN_CALLS: join(scratch, "calls"), STAND_IN_FAILS: fails,
    };
    const run = bash(script(timedStep), scratch, env);
    assert.equal(run.status, status, `${label}: the step's status (${run.stderr})`);
    assert.deepEqual(readFileSync(env.STAND_IN_CALLS, "utf8").trim().split("\n"), ["-C examples codemob", "-C examples mdm"], `${label}: both targets run`);
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, "utf8").trim().split("\n");
    assert.deepEqual(summary.slice(0, 4), ["### Standalone examples", "", "| part | seconds | result |", "| --- | --- | --- |"]);
    const cargo = (target) => `| Cargo, in ${target}: Finished \`test\` profile [unoptimized] target(s) in 1m 02s | | |`;
    assert.match(summary[4], new RegExp(`^\\| make -C examples codemob \\| \\d+ \\| ${results[0]} \\|$`), label);
    assert.equal(summary[5], cargo("codemob"), `${label}: Cargo's own line, without colour codes`);
    assert.match(summary[6], new RegExp(`^\\| make -C examples mdm \\| \\d+ \\| ${results[1]} \\|$`), label);
    assert.equal(summary[7], cargo("mdm"));
    assert.match(summary[8], /^\| both targets \| \d+ \| \|$/);
    assert.equal(summary.length, 9, label);
    assert.match(run.stdout, /test result: ok\. 1 passed/, `${label}: the targets' output still reaches the log`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// 6. Pack 006's step, the same way, with a stand-in for the pack's script.
const metaStep = stepLines(meta, "Long-term agents example (keyless), timed");
assert.ok(metaStep.includes("            meta/examples/006-long-term-agents/examples.sh"), "the step runs the pack's script as a whole line");
for (const [label, code, status, result] of [["the script passes", 0, 0, "passed"], ["the script fails", 3, 1, "failed"]]) {
  const scratch = mkdtempSync(join(tmpdir(), "nightly-examples-"));
  try {
    mkdirSync(join(scratch, "meta/examples/006-long-term-agents"), { recursive: true });
    const fakeScript = join(scratch, "meta/examples/006-long-term-agents/examples.sh");
    writeFileSync(fakeScript, `#!/usr/bin/env bash\necho '[long-term-agents] stand-in'\nexit ${code}\n`);
    chmodSync(fakeScript, 0o755);
    const env = { PATH: process.env.PATH, GITHUB_STEP_SUMMARY: join(scratch, "summary") };
    const run = bash(script(metaStep), scratch, env);
    assert.equal(run.status, status, `${label}: the step's status (${run.stderr})`);
    const summary = readFileSync(env.GITHUB_STEP_SUMMARY, "utf8").trim().split("\n");
    assert.equal(summary[0], "### Meta long-term agents example");
    assert.match(summary[2], new RegExp(`^seconds: \\d+; result: ${result}$`), label);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
const packScript = resolve(root, "meta/examples/006-long-term-agents/examples.sh");
accessSync(packScript, constants.X_OK);
const packText = readFileSync(packScript, "utf8");
assert.ok(packText.includes("No provider key is needed."), "the pack's script says it is keyless");
for (const key of ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"]) {
  assert.ok(!packText.includes(key), `the pack's script names no ${key}`);
}

console.log("nightly example jobs: contracts hold");
