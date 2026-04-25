#!/usr/bin/env node
import { spawn } from "node:child_process";
import { appendFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

function usage() {
  console.log(`usage: buildbuddy-simulate-scenarios.mjs [scenario ...]

Scenarios:
  workspace-test     Run the full Bazel workspace test lane.
  workspace-test-rbe Run the remote-compatible workspace test lane.
  workspace-test-local
                     Run the full workspace test lane with local spawns.
  warm-noop          Run warm fast-test and clippy no-op checks.
  same-worktree      Run two same-checkout agents in parallel with distinct lanes.
  same-command       Run two same-checkout agents using the same command in parallel.
  support-file       Run exact selectors for a shared integration-test support file.
  edit-probes        Make real edits in a temporary worktree and time edit lanes.
  edit-probes-warmed Prewarm lanes in a temporary worktree before timing edits.
  prewarm-dev        Run the shared dev-lane prewarm helper.
  prewarm-ci         Run the shared CI-lane prewarm helper.
  multi-worktree     Create two temporary git worktrees and run parallel agents.
  ci-cold            Run CI-like checks with fresh output bases.
  ci-parallel        Run CI-like fast-test and clippy checks in parallel.
  ci-workspace       Run workspace-test-rbe and clippy-rbe in parallel.
  all                Run the default scenario set.
`);
}

function now() {
  return process.hrtime.bigint();
}

function secondsSince(start) {
  return Number(process.hrtime.bigint() - start) / 1e9;
}

function run(command, args, options = {}) {
  const start = now();
  return new Promise((resolve) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => {
      output += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      output += chunk.toString();
    });
    child.on("close", (code) => {
      resolve({
        code,
        command: [command, ...args].join(" "),
        cwd: options.cwd,
        label: options.label,
        output,
        seconds: secondsSince(start),
      });
    });
  });
}

function repoCommand(cwd, env, command, paths = [], extra = []) {
  return run(
    "./scripts/buildbuddy-bazel-poc",
    [...paths, ...extra],
    {
      cwd,
      env: {
        BUILDBUDDY_BAZEL_COMMAND: command,
        ...env,
      },
      label: `${command}${paths.length ? ` ${paths.join(" ")}` : ""}`,
    },
  );
}

function printResult(result) {
  const status = result.code === 0 ? "PASS" : `FAIL(${result.code})`;
  console.log(`${status} ${result.seconds.toFixed(2)}s ${result.label}`);
  const interesting = result.output
    .split("\n")
    .filter((line) =>
      line.includes("INFO: Elapsed time:") ||
      line.includes("INFO: Build completed") ||
      line.includes("Executed ") ||
      line.includes("ERROR:") ||
      line.includes("FAILED")
    )
    .slice(-8);
  for (const line of interesting) console.log(`  ${line}`);
}

async function removeTempTree(path) {
  await run("chmod", ["-R", "u+w", path], { cwd: "/", label: `chmod ${path}` });
  rmSync(path, { force: true, maxRetries: 3, recursive: true, retryDelay: 100 });
}

async function warmNoop(root) {
  console.log("\n== warm-noop ==");
  for (const result of [
    await repoCommand(root, {}, "fast-test", [], ["--jobs=64"]),
    await repoCommand(root, {}, "clippy", [], ["--jobs=64", "--color=no", "--curses=no"]),
  ]) {
    printResult(result);
    if (result.code !== 0) return result.code;
  }
  return 0;
}

async function workspaceTest(root, { local = false } = {}) {
  console.log(`\n== workspace-test${local ? "-local" : ""} ==`);
  const command = local ? "workspace-test-local" : "workspace-test";
  const result = await repoCommand(
    root,
    { RUST_LANE_ID: command },
    command,
    [],
    local ? [] : ["--jobs=64"],
  );
  printResult(result);
  return result.code;
}

async function workspaceTestRbe(root) {
  console.log("\n== workspace-test-rbe ==");
  const result = await repoCommand(
    root,
    { RUST_LANE_ID: "workspace-test-rbe" },
    "workspace-test-rbe",
    [],
    ["--jobs=64"],
  );
  printResult(result);
  return result.code;
}

async function sameWorktree(root) {
  console.log("\n== same-worktree ==");
  const jobs = [
    repoCommand(
      root,
      { RUST_LANE_ID: "same-a" },
      "owned-build",
      ["meerkat-machine-dsl-core/src/lib.rs"],
      ["--jobs=64", "--color=no", "--curses=no"],
    ),
    repoCommand(
      root,
      { RUST_LANE_ID: "same-b" },
      "owned-fast-test",
      ["meerkat-mob/tests/member_session_bindings.rs"],
      ["--jobs=64"],
    ),
  ];
  const results = await Promise.all(jobs);
  for (const result of results) printResult(result);
  return results.some((result) => result.code !== 0) ? 1 : 0;
}

async function sameCommand(root) {
  console.log("\n== same-command ==");
  const jobs = [
    repoCommand(
      root,
      { RUST_LANE_ID: "same-command-a" },
      "owned-fast-test",
      ["meerkat-mob/tests/member_session_bindings.rs"],
      ["--jobs=64"],
    ),
    repoCommand(
      root,
      { RUST_LANE_ID: "same-command-b" },
      "owned-fast-test",
      ["meerkat-models/tests/guards.rs"],
      ["--jobs=64"],
    ),
  ];
  const results = await Promise.all(jobs);
  for (const result of results) printResult(result);
  return results.some((result) => result.code !== 0) ? 1 : 0;
}

async function supportFile(root) {
  console.log("\n== support-file ==");
  const path = "meerkat/tests/support/test_session_store.rs";
  const results = [
    await repoCommand(root, {}, "owned-fast-test", [path], ["--jobs=64"]),
    await repoCommand(root, {}, "owned-fast-test-local", [path], ["--color=no", "--curses=no"]),
  ];
  for (const result of results) printResult(result);
  return results.some((result) => result.code !== 0) ? 1 : 0;
}

function editProbeCases() {
  return [
    {
      command: "owned-build",
      env: { RUST_LANE_ID: "edit-source" },
      extra: ["--jobs=64", "--color=no", "--curses=no"],
      marker: "\n// BuildBuddy edit probe: source-owned-build.\n",
      path: "meerkat-machine-dsl-core/src/lib.rs",
    },
    {
      command: "owned-fast-test",
      env: { RUST_LANE_ID: "edit-test" },
      extra: ["--jobs=64"],
      marker: "\n// BuildBuddy edit probe: exact-test-remote.\n",
      path: "meerkat-mob/tests/member_session_bindings.rs",
    },
    {
      command: "owned-fast-test-local",
      env: { RUST_LANE_ID: "edit-support-local" },
      extra: ["--color=no", "--curses=no"],
      marker: "\n// BuildBuddy edit probe: support-local.\n",
      path: "meerkat/tests/support/test_session_store.rs",
    },
  ];
}

async function editProbes(root, { prewarm = false } = {}) {
  console.log(`\n== ${prewarm ? "edit-probes-warmed" : "edit-probes"} ==`);
  const temp = mkdtempSync(join(tmpdir(), "meerkat-bb-edit-probes-"));
  const worktree = join(temp, basename(root));
  const head = (await run("git", ["rev-parse", "HEAD"], { cwd: root, label: "rev-parse" })).output.trim();
  try {
    const add = await run("git", ["worktree", "add", "--detach", worktree, head], {
      cwd: root,
      label: "edit-probe-worktree",
    });
    printResult(add);
    if (add.code !== 0) return add.code;

    const probes = editProbeCases();

    if (prewarm) {
      console.log("prewarming edit lanes...");
      const warmResults = await Promise.all(
        probes.map((probe) => repoCommand(worktree, probe.env, probe.command, [probe.path], probe.extra)),
      );
      for (const result of warmResults) printResult(result);
      if (warmResults.some((result) => result.code !== 0)) return 1;
    }

    for (const probe of probes) {
      appendFileSync(join(worktree, probe.path), probe.marker);
      const result = await repoCommand(worktree, probe.env, probe.command, [probe.path], probe.extra);
      printResult(result);
      if (result.code !== 0) return result.code;
    }
    return 0;
  } finally {
    await run("git", ["worktree", "remove", "--force", worktree], { cwd: root, label: "remove-edit-probe-worktree" });
    await removeTempTree(temp);
  }
}

async function multiWorktree(root) {
  console.log("\n== multi-worktree ==");
  const temp = mkdtempSync(join(tmpdir(), "meerkat-bb-worktrees-"));
  const base = basename(root);
  const a = join(temp, `${base}-a`);
  const b = join(temp, `${base}-b`);
  const head = (await run("git", ["rev-parse", "HEAD"], { cwd: root, label: "rev-parse" })).output.trim();
  try {
    for (const result of [
      await run("git", ["worktree", "add", "--detach", a, head], { cwd: root, label: "worktree-a" }),
      await run("git", ["worktree", "add", "--detach", b, head], { cwd: root, label: "worktree-b" }),
    ]) {
      printResult(result);
      if (result.code !== 0) return result.code;
    }
    const results = await Promise.all([
      repoCommand(
        a,
        { RUST_LANE_ID: "wt-a" },
        "owned-build",
        ["meerkat-machine-dsl-core/src/lib.rs"],
        ["--jobs=64", "--color=no", "--curses=no"],
      ),
      repoCommand(
        b,
        { RUST_LANE_ID: "wt-b" },
        "owned-fast-test",
        ["meerkat-mob/tests/member_session_bindings.rs"],
        ["--jobs=64"],
      ),
    ]);
    for (const result of results) printResult(result);
    return results.some((result) => result.code !== 0) ? 1 : 0;
  } finally {
    await run("git", ["worktree", "remove", "--force", a], { cwd: root, label: "remove-worktree-a" });
    await run("git", ["worktree", "remove", "--force", b], { cwd: root, label: "remove-worktree-b" });
    await removeTempTree(temp);
  }
}

async function prewarmProfile(root, profile) {
  console.log(`\n== prewarm-${profile} ==`);
  const result = await run("./scripts/buildbuddy-prewarm-lanes", [profile], {
    cwd: root,
    env: { RUST_LANE_ID_PREFIX: `scenario-prewarm-${profile}` },
    label: `buildbuddy-prewarm-lanes ${profile}`,
  });
  printResult(result);
  return result.code;
}

async function ciCold(root) {
  console.log("\n== ci-cold ==");
  const temp = mkdtempSync(join(tmpdir(), "meerkat-bb-ci-"));
  try {
    const commonEnv = {
      BUILDBUDDY_MAX_IDLE_SECS: "5",
      MEERKAT_BUILDBUDDY_OUTPUT_ROOT: temp,
    };
    for (const result of [
      await repoCommand(root, { ...commonEnv, RUST_LANE_ID: "ci-fast" }, "fast-test", [], ["--jobs=64"]),
      await repoCommand(
        root,
        { ...commonEnv, RUST_LANE_ID: "ci-clippy" },
        "clippy",
        [],
        ["--jobs=64", "--color=no", "--curses=no"],
      ),
    ]) {
      printResult(result);
      if (result.code !== 0) return result.code;
    }
    return 0;
  } finally {
    await removeTempTree(temp);
  }
}

async function ciParallel(root) {
  console.log("\n== ci-parallel ==");
  const temp = mkdtempSync(join(tmpdir(), "meerkat-bb-ci-parallel-"));
  try {
    const commonEnv = {
      BUILDBUDDY_MAX_IDLE_SECS: "5",
      MEERKAT_BUILDBUDDY_OUTPUT_ROOT: temp,
    };
    const results = await Promise.all([
      repoCommand(root, { ...commonEnv, RUST_LANE_ID: "ci-parallel-fast" }, "fast-test", [], ["--jobs=64"]),
      repoCommand(
        root,
        { ...commonEnv, RUST_LANE_ID: "ci-parallel-clippy" },
        "clippy",
        [],
        ["--jobs=64", "--color=no", "--curses=no"],
      ),
    ]);
    for (const result of results) printResult(result);
    return results.some((result) => result.code !== 0) ? 1 : 0;
  } finally {
    await removeTempTree(temp);
  }
}

async function ciWorkspace(root) {
  console.log("\n== ci-workspace ==");
  const temp = mkdtempSync(join(tmpdir(), "meerkat-bb-ci-workspace-"));
  try {
    const commonEnv = {
      BUILDBUDDY_MAX_IDLE_SECS: "5",
      MEERKAT_BUILDBUDDY_OUTPUT_ROOT: temp,
    };
    const results = await Promise.all([
      repoCommand(
        root,
        { ...commonEnv, RUST_LANE_ID: "ci-workspace-test-rbe" },
        "workspace-test-rbe",
        [],
        ["--jobs=64"],
      ),
      repoCommand(
        root,
        { ...commonEnv, RUST_LANE_ID: "ci-workspace-clippy" },
        "clippy-rbe",
        [],
        ["--jobs=64", "--color=no", "--curses=no"],
      ),
    ]);
    for (const result of results) printResult(result);
    return results.some((result) => result.code !== 0) ? 1 : 0;
  } finally {
    await removeTempTree(temp);
  }
}

const rootResult = await run("git", ["rev-parse", "--show-toplevel"], {
  cwd: process.cwd(),
  label: "workspace-root",
});
if (rootResult.code !== 0) {
  printResult(rootResult);
  process.exit(rootResult.code);
}

const root = rootResult.output.trim();
const requested = process.argv.slice(2);
if (requested.includes("--help") || requested.includes("-h")) {
  usage();
  process.exit(0);
}
const scenarios = requested.length === 0 || requested.includes("all")
  ? [
      "workspace-test",
      "workspace-test-rbe",
      "warm-noop",
      "same-worktree",
      "same-command",
      "support-file",
      "edit-probes",
      "edit-probes-warmed",
      "prewarm-dev",
      "prewarm-ci",
      "multi-worktree",
      "ci-cold",
      "ci-parallel",
      "ci-workspace",
    ]
  : requested;

const runners = new Map([
  ["workspace-test", workspaceTest],
  ["workspace-test-rbe", workspaceTestRbe],
  ["workspace-test-local", (root) => workspaceTest(root, { local: true })],
  ["warm-noop", warmNoop],
  ["same-worktree", sameWorktree],
  ["same-command", sameCommand],
  ["support-file", supportFile],
  ["edit-probes", editProbes],
  ["edit-probes-warmed", (root) => editProbes(root, { prewarm: true })],
  ["prewarm-dev", (root) => prewarmProfile(root, "dev")],
  ["prewarm-ci", (root) => prewarmProfile(root, "ci")],
  ["multi-worktree", multiWorktree],
  ["ci-cold", ciCold],
  ["ci-parallel", ciParallel],
  ["ci-workspace", ciWorkspace],
]);

for (const scenario of scenarios) {
  const runner = runners.get(scenario);
  if (!runner) {
    console.error(`unknown scenario: ${scenario}`);
    usage();
    process.exit(2);
  }
  const code = await runner(root);
  if (code !== 0) process.exit(code);
}
