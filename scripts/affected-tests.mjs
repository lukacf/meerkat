#!/usr/bin/env node
// Affected-test selection for local runs: the commands an author runs before
// pushing a change, printed one per line on stdout, or run in order with
// --run, which stops at the first failure. A short summary of the plan and
// what it leaves to CI goes to stderr.
//
// usage:
//   scripts/affected-tests.mjs [<base>..<head> | <base>...<head>] [--no-uncommitted] [--run]
//   scripts/affected-tests.mjs [--run] -- <changed-path>...
//
// The change. Without a range: every path that differs between
// merge-base(origin/main, HEAD) and HEAD, plus the uncommitted ones (staged,
// unstaged and untracked). `A..B` diffs A with B, `A...B` their merge base
// with B. B must be the checked-out HEAD, because the plan reads the working
// tree (cargo metadata, the inventories, the specs); the uncommitted paths
// join unless --no-uncommitted. After `--` the listed paths are the change.
//
// The selection. This script classifies no path itself. The change goes to
// the CI planner (scripts/ci-cargo-lanes.mjs, which `make ci-lanes-selftest`
// pins) as an explicit path list, planned as a push (and, without the narrowed
// test targets below, as a pull request for its wasm and Web SDK flags), and
// each command is CI's own form for what the plan selects:
//   regenerate  `make machine-codegen` and `make protocol-codegen` when
//               machine authority changed, the machine poster generator when
//               it or a poster input changed, `make regen-schemas` when a
//               generated contract input changed;
//   clippy      the push plan's Clippy rows (every target of every changed
//               package, all features), CI's closure check (`check
//               --all-features`, one invocation per product) over the
//               reverse-dependency closure beyond them, and `make wasm-check`
//               when the web runtime's build changed;
//   tests       one nextest run per product over the changed packages and
//               their workspace reverse dependencies (cargo metadata), on the
//               ci-integration profile (the fast selection of CI's
//               closure-tests rows), without the tests CI leaves to other
//               hosts (HOSTED_RUNNER_EXCLUDED_*); the feature suites of those
//               packages (FEATURE_UNIT_SUITES); the selected integration
//               suites those runs do not cover (a suite with features, or
//               of a package outside them); the plan's HeadCanonical and
//               e2e-fast rows; and the readers of changed non-Rust files
//               (scripts/ci-non-rust-readers.json, the "./" whole-checkout
//               readers included) in CI's reader-row form: the inventory's
//               features and a filter on the reading tests;
//   TLC         see below;
//   SDK suites  the Python and TypeScript suites when a generated contract
//               input or an SDK host input changed, the Web suite when a
//               generated contract input or a Web SDK input changed (the pull
//               request's example_web), after building meerkat-rpc as CI's
//               SDK lane does.
// Each nextest row exports what CI's row exports: the product's test-thread
// stack (ci/products.json), the mcp-test-server fixture for packages that
// dev-depend on it, and the Python that meerkat-connect's process controls
// run.
//
// Narrowing. A changed package whose only changed paths are Rust sources of
// its tests/*.rs targets (the targets cargo metadata lists and the modules
// they declare, resolved as scripts/rust-test-selector.mjs does, and sources
// of no other target) runs just those targets, and adds no reverse
// dependencies: no other crate compiles a test target. Unit-test modules under
// src/ are not narrowed (a cfg(test) module cannot be told from library code
// without compiling it), nor is a non-Rust file under tests/ (library code can
// include it), nor any package of a whole-workspace, lock-scoped or
// product-wide plan.
//
// Generated files and TLC. The generators run first, and TLC follows the
// specs: the machines and compositions whose specs/ files changed, the
// machines a changed DSL file declares with the compositions that hold them
// (`xtask machine-verify`, with the flags of the canonical lane's own
// machine-verify part, and the lane's audit parts that run their audits:
// crates/xtask/tests/machine_verify_all_tlc_test.sh, read for the mapping),
// or the whole canonical lane (`make machine-verify`) when the lane, its
// xtask driver, or a spec or DSL path no registered machine owns changed. A
// change elsewhere in machine authority (owner code, generated code, CI
// wiring) changes no TLC input until regeneration changes specs/. Under
// --run, a regeneration that changes the working tree stops the run (exit 3)
// so the next run plans the regenerated tree.
//
// Exit status: 0 when every listed command passed (or nothing was listed),
// the failing command's status under --run, 1 when the change cannot be
// planned, 2 for a usage error, 3 when regeneration changed the tree.

import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmdirSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  FEATURE_UNIT_SUITES,
  HOOK_TEST_STACK,
  HOSTED_RUNNER_EXCLUDED_BINARIES,
  HOSTED_RUNNER_EXCLUDED_TESTS,
  INTEGRATION_SUITES,
  LIVE_ONLY_FEATURES,
  featureSuiteFilters,
  featureSuiteFlags,
} from "./ci-cargo-lanes.mjs";
import { excludedTestsFilter, parseFilter, profileFilter } from "./ci-hook-coverage.mjs";
import { loadNonRustReaders, readerSelectionsFor } from "./ci-non-rust-readers.mjs";
import { CORE, loadProducts, productOfPath, productRootPrefixes } from "./ci-products.mjs";
import {
  normalizePath,
  packageDir,
  packageDirs,
  packagesForFile,
  readMetadata,
  root,
  testSourcePaths,
  workspacePackages,
} from "./rust-test-selector.mjs";

const CARGO = "./scripts/repo-cargo";
const PLANNER = "scripts/ci-cargo-lanes.mjs";
export const TLC_LANE = "crates/xtask/tests/machine_verify_all_tlc_test.sh";
// The canonical TLC lane and the xtask module that implements its
// machine-verify part: a change to either can change every TLC result.
export const TLC_PROCEDURE_PATHS = [TLC_LANE, "crates/xtask/src/machines.rs"];
export const DSL_DIR = "crates/meerkat-machine-schema/src/catalog/dsl/";
// The fast selection of CI's closure-tests rows (the hook's own), with a
// hung-test kill: the whole-package runs and the narrowed test targets.
export const TEST_PROFILE = "ci-integration";
// CI's unit, integration-suite and reader rows.
export const ROW_PROFILE = "ci-pr";
const STATUS = ["--no-fail-fast", "--status-level", "fail", "--final-status-level", "fail"];
// ci.yml's unit lanes export it for meerkat-connect's Console process
// controls (resources::local_round), which refuse to guess an interpreter.
const CONNECT_PYTHON_PACKAGE = "meerkat-connect";
const FIXTURE_PACKAGE = "mcp-test-server";
const LIB_KINDS = ["lib", "rlib", "cdylib", "dylib", "staticlib", "proc-macro"];

export class UsageError extends Error {}

const USAGE = `usage: scripts/affected-tests.mjs [<base>..<head> | <base>...<head>] [--no-uncommitted] [--run]
       scripts/affected-tests.mjs [--run] -- <changed-path>...
Prints the commands that test a change locally; --run runs them in order and stops at the first failure.
Default change: merge-base(origin/main, HEAD)..HEAD plus uncommitted (staged, unstaged, untracked) paths.
`;

export function parseArgs(argv) {
  const args = { range: null, paths: null, run: false, uncommitted: true, help: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--run") args.run = true;
    else if (arg === "--no-uncommitted") args.uncommitted = false;
    else if (arg === "--help" || arg === "-h") args.help = true;
    else if (arg === "--") {
      args.paths = argv.slice(index + 1);
      break;
    } else if (arg.startsWith("-")) throw new UsageError(`unknown argument: ${arg}`);
    else if (args.range === null) args.range = arg;
    else throw new UsageError(`unexpected argument: ${arg}`);
  }
  if (args.paths !== null && args.range !== null) throw new UsageError("give a range or a path list after --, not both");
  if (args.paths !== null && args.paths.length === 0) throw new UsageError("-- needs at least one changed path");
  if (args.paths !== null && !args.uncommitted) throw new UsageError("--no-uncommitted applies to a range, not a path list");
  return args;
}

function git(repo, gitArgs, options = {}) {
  return execFileSync("git", gitArgs, {
    cwd: repo,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: ["pipe", "pipe", "pipe"],
    ...options,
  });
}

function commitOf(repo, rev) {
  try {
    return git(repo, ["rev-parse", "--verify", "--quiet", "--end-of-options", `${rev}^{commit}`]).trim();
  } catch {
    throw new UsageError(`${rev} does not name a commit`);
  }
}

// The diff base and head of the change. Without a range: the merge base of
// origin/main and HEAD. A range's head must be the checked-out HEAD.
export function resolveRange(repo, range) {
  const head = commitOf(repo, "HEAD");
  if (range === null) {
    let main;
    try {
      main = commitOf(repo, "origin/main");
    } catch {
      throw new UsageError("origin/main does not resolve: fetch it, or pass a range such as main..HEAD");
    }
    const base = git(repo, ["merge-base", main, head]).trim();
    return { base, head, label: `merge-base(origin/main, HEAD) ${base.slice(0, 10)}..HEAD` };
  }
  const match = /^(.+?)(\.\.\.?)(.*)$/.exec(range);
  if (!match) throw new UsageError(`${range} is not a range (<base>..<head> or <base>...<head>)`);
  const [, left, dots, right] = match;
  if (commitOf(repo, right || "HEAD") !== head) {
    throw new UsageError(`${right} is not the checked-out HEAD: the plan reads the working tree, so check out the range's head first`);
  }
  const from = commitOf(repo, left);
  const base = dots === "..." ? git(repo, ["merge-base", from, head]).trim() : from;
  return { base, head, label: range };
}

// Every path the change touches, sorted: the committed diff from base to head
// and, with `uncommitted`, the working tree's changes against HEAD (staged and
// unstaged) and its untracked files. Renames list both sides, deletions their
// path, as the planner's own diff does.
export function changedPaths(repo, { base, head, uncommitted }) {
  const names = (output) => output.split("\0").filter(Boolean);
  const diff = (revs) => names(git(repo, ["diff", "--no-renames", "--name-only", "-z", "--diff-filter=ACMRDT", ...revs, "--"]));
  const paths = new Set(diff([base, head]));
  if (uncommitted) {
    for (const path of diff(["HEAD"])) paths.add(path);
    for (const path of names(git(repo, ["ls-files", "--others", "--exclude-standard", "-z"]))) paths.add(path);
  }
  return [...paths].map(normalizePath).sort();
}

function collectChange(repo, args) {
  if (args.paths !== null) {
    const paths = [...new Set(args.paths.map(normalizePath))].sort();
    // Only a Cargo.lock change needs the base: the planner scopes it against
    // the base lock, and without one it plans the whole workspace.
    let base = null;
    if (paths.includes("Cargo.lock")) {
      try {
        base = resolveRange(repo, null).base;
      } catch (error) {
        process.stderr.write(`affected-tests: no base lock (${error.message}); the planner plans the whole workspace for Cargo.lock\n`);
      }
    }
    return { paths, base, label: "the listed paths" };
  }
  // resolveRange holds the head to the checked-out HEAD, so the uncommitted
  // paths belong to the same tree.
  const range = resolveRange(repo, args.range);
  const paths = changedPaths(repo, { ...range, uncommitted: args.uncommitted });
  return { paths, base: range.base, label: `${range.label}${args.uncommitted ? " plus uncommitted changes" : ""}` };
}

// The CI planner's plan of an explicit path list, as `event`.
export function runPlanner(repo, { event, paths, baseLock = null }) {
  const plannerArgs = [PLANNER, "--event", event, "--format", "json", "--paths-from-stdin"];
  if (baseLock) plannerArgs.push("--base-lock", baseLock);
  const result = spawnSync(process.execPath, plannerArgs, {
    cwd: repo,
    input: `${paths.join("\n")}\n`,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`the CI planner (${PLANNER} --event ${event}) failed: ${(result.stderr || result.stdout || "").trim()}`);
  }
  return JSON.parse(result.stdout);
}

// The push and pull-request plans of the change, with the base revision's
// Cargo.lock in a temporary file when the lock changed (the planner reads the
// head lock from the working tree). The pull-request plan's own flags
// (wasm-check, the Web SDK suite) come from the paths left once the narrowed
// test targets are taken out: a test target is in no wasm or Web SDK build.
function planChange(repo, change, ctx) {
  let lockDir = null;
  let lockFile = null;
  if (change.paths.includes("Cargo.lock") && change.base) {
    const text = spawnSync("git", ["show", `${change.base}:Cargo.lock`], { cwd: repo, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
    if (text.status === 0) {
      lockDir = mkdtempSync(join(tmpdir(), "affected-tests-"));
      lockFile = join(lockDir, "base-Cargo.lock");
      writeFileSync(lockFile, text.stdout);
    }
  }
  try {
    const push = runPlanner(repo, { event: "push", paths: change.paths, baseLock: lockFile });
    const narrowed = narrowedPackages(change.paths, push, ctx);
    const rest = change.paths.filter((path) => {
      const owners = ctx.packagesForPath(path);
      return owners.length === 0 || owners.some((name) => !narrowed.has(name));
    });
    const pr = rest.length > 0 ? runPlanner(repo, { event: "pull_request", paths: rest, baseLock: lockFile }) : NOTHING_PLANNED;
    return { push, pr };
  } finally {
    if (lockFile) unlinkSync(lockFile);
    if (lockDir) rmdirSync(lockDir);
  }
}

// The pull-request flags of an empty path list.
const NOTHING_PLANNED = { wasm: false, example_web: false, examples_browser: false };

// ----------------------------------------------------------------------------
// The canonical TLC lane, read for its audit parts and its machine-verify
// flags. Every runner (`name="${workspace_root}/specs/<machines|compositions>
// /<slug>/<file>.sh"`) belongs to the spec directory it lives in; a part runs
// the runners its `if run_part <part>; then ... fi` block names, plus the
// audits a `<prefix>_shard="<part>"` assignment gives it. Throws unless every
// runner has a part and every audit part of the lane's audit_shard() list has
// a runner, so a lane that stops reading this way fails the self-test.
export function parseTlcLane(text) {
  const runners = new Map();
  for (const match of text.matchAll(/^([a-z0-9_]+)="\$\{workspace_root\}\/(specs\/(?:machines|compositions)\/[a-z0-9_]+)\/[A-Za-z0-9_.-]+\.sh"$/gm)) {
    runners.set(match[1], match[2]);
  }
  if (runners.size === 0) throw new Error(`${TLC_LANE}: no audit runners found`);
  const shardList = /^audit_shard\(\) \{\n\s*case "\$1" in\n\s*([a-z0-9|-]+)\) return 0 ;;$/m.exec(text);
  if (!shardList) throw new Error(`${TLC_LANE}: no audit_shard() part list`);
  const auditParts = shardList[1].split("|");
  const parts = new Map(auditParts.map((part) => [part, new Set()]));
  const add = (part, runner) => {
    if (!parts.has(part)) throw new Error(`${TLC_LANE}: ${part} runs ${runner} but is not an audit part`);
    parts.get(part).add(runner);
  };
  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const open = /^if run_part (audits-[a-z0-9]+); then$/.exec(lines[index]);
    if (!open) continue;
    let end = index + 1;
    for (; end < lines.length && lines[end] !== "fi"; end += 1) {
      if (/^\s*if run_part /.test(lines[end])) {
        throw new Error(`${TLC_LANE}: the run_part ${open[1]} block at line ${index + 1} has no fi before the next part`);
      }
      for (const ref of lines[end].matchAll(/"\$\{([a-z0-9_]+)\}"/g)) {
        if (runners.has(ref[1])) add(open[1], ref[1]);
      }
    }
    if (end === lines.length) throw new Error(`${TLC_LANE}: the run_part ${open[1]} block at line ${index + 1} has no fi`);
  }
  for (const match of text.matchAll(/^([a-z0-9_]+)_shard="(audits-[a-z0-9]+)"$/gm)) {
    const runner = `${match[1]}_audit`;
    if (!runners.has(runner)) throw new Error(`${TLC_LANE}: ${match[1]}_shard names no audit runner ${runner}`);
    add(match[2], runner);
  }
  const used = new Set([...parts.values()].flatMap((set) => [...set]));
  const unused = [...runners.keys()].filter((runner) => !used.has(runner));
  if (unused.length > 0) throw new Error(`${TLC_LANE}: no audit part runs ${unused.join(", ")}`);
  const empty = [...parts].filter(([, set]) => set.size === 0).map(([part]) => part);
  if (empty.length > 0) throw new Error(`${TLC_LANE}: audit parts ${empty.join(", ")} run no audit`);
  const body = /^run_machine_verify\(\) \{\n([\s\S]*?)\n\}$/m.exec(text);
  if (!body) throw new Error(`${TLC_LANE}: no run_machine_verify()`);
  const words = body[1].replaceAll("\\\n", " ").trim().split(/\s+/);
  const at = words.indexOf("machine-verify");
  if (words[0] !== '"${xtask_bin}"' || at !== 1 || words[2] !== "--all") {
    throw new Error(`${TLC_LANE}: run_machine_verify() is not "\${xtask_bin}" machine-verify --all ...`);
  }
  const flags = [];
  const skipCompositions = [];
  for (let index = 3; index < words.length; index += 1) {
    if (words[index] === "--skip-tlc-composition") skipCompositions.push(words[++index]);
    else if (/^--[a-z-]+$/.test(words[index])) flags.push(words[index]);
    else throw new Error(`${TLC_LANE}: unexpected machine-verify argument ${words[index]}`);
  }
  return {
    runners,
    parts: new Map([...parts].map(([part, set]) => [part, [...set].sort()])),
    verifyFlags: flags,
    skipCompositions,
  };
}

// The registered machines and compositions, from their generated docs: a
// machine directory's mapping.md names its machine under `### Machine`, a
// composition's contract.md its member machines under `## Machines`.
export function machineRegistry(specsRoot) {
  const dirs = (path) =>
    existsSync(path) ? readdirSync(path, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort() : [];
  const nameToSlug = new Map();
  const machines = new Set();
  for (const slug of dirs(join(specsRoot, "machines"))) {
    const mapping = join(specsRoot, "machines", slug, "mapping.md");
    if (!existsSync(mapping)) continue;
    const section = /^### Machine\n((?:- `[^`]+`\n)+)/m.exec(readFileSync(mapping, "utf8"));
    if (!section) continue;
    machines.add(slug);
    for (const name of section[1].matchAll(/`([^`]+)`/g)) nameToSlug.set(name[1], slug);
  }
  const compositions = new Map();
  for (const slug of dirs(join(specsRoot, "compositions"))) {
    const contract = join(specsRoot, "compositions", slug, "contract.md");
    if (!existsSync(contract)) continue;
    const section = /^## Machines\n((?:- `[^`]+`: `[^`]+`.*\n)+)/m.exec(readFileSync(contract, "utf8"));
    if (!section) continue;
    compositions.set(slug, new Set([...section[1].matchAll(/^- `[^`]+`: `([^`]+)`/gm)].map((name) => name[1])));
  }
  return { machines, nameToSlug, compositions };
}

// The machines a DSL catalog file declares (`machine Name {`): its own, else
// those of the module file whose directory holds it. Null when the file is
// gone.
export function dslMachineNames(path, readText) {
  const declared = (file) => {
    const text = readText(file);
    return text === null ? null : [...text.matchAll(/^\s*machine\s+([A-Z][A-Za-z0-9]*)\s*\{/gm)].map((match) => match[1]);
  };
  const own = declared(path);
  if (own === null || own.length > 0) return own;
  const nested = path.slice(DSL_DIR.length).split("/");
  return nested.length > 1 ? declared(`${DSL_DIR}${nested[0]}.rs`) ?? [] : [];
}

// What TLC the change needs: machine and composition slugs, or reasons to
// run the whole lane (`all`).
export function tlcSelection(paths, registry, readText) {
  const machines = new Set();
  const compositions = new Set();
  const all = [];
  const addMachine = (slug, name) => {
    machines.add(slug);
    for (const [composition, members] of registry.compositions) if (members.has(name)) compositions.add(composition);
  };
  for (const path of paths) {
    const spec = /^specs\/(machines|compositions)\/([^/]+)(\/.+)?$/.exec(path);
    if (spec) {
      const [, kind, slug, rest] = spec;
      if (rest === undefined) {
        // A file beside the machine directories: documentation, or a script
        // no machine owns.
        if (!/\.mdx?$/.test(slug)) all.push(`${path} belongs to no registered machine`);
      } else if (kind === "machines" && registry.machines.has(slug)) machines.add(slug);
      else if (kind === "compositions" && registry.compositions.has(slug)) compositions.add(slug);
      else all.push(`${path} belongs to no registered ${kind === "machines" ? "machine" : "composition"}`);
      continue;
    }
    if (path.startsWith(DSL_DIR) && path.endsWith(".rs")) {
      const names = dslMachineNames(path, readText);
      const unknown = names === null ? ["(removed)"] : names.filter((name) => !registry.nameToSlug.has(name));
      if (names === null || names.length === 0 || unknown.length > 0) {
        all.push(`${path} declares ${names?.length ? `no registered machine (${unknown.join(", ")})` : "no machine"}`);
        continue;
      }
      for (const name of names) addMachine(registry.nameToSlug.get(name), name);
      continue;
    }
    if (TLC_PROCEDURE_PATHS.includes(path)) all.push(`${path} changed`);
  }
  return { machines: [...machines].sort(), compositions: [...compositions].sort(), all };
}

// ----------------------------------------------------------------------------
// Commands.

export function shellQuote(word) {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(word) ? word : `'${word.replaceAll("'", `'\\''`)}'`;
}

function commandLine(argv, env = []) {
  return [...env, ...argv.map(shellQuote)].join(" ");
}

// The packages a cargo selection names: `-p NAME`, or `--workspace` less
// every `--exclude NAME`.
function selectedPackages(flags, allNames) {
  const selected = [];
  const excluded = new Set();
  let workspace = false;
  for (let index = 0; index < flags.length; index += 1) {
    if (flags[index] === "-p" || flags[index] === "--package") selected.push(flags[++index]);
    else if (flags[index] === "--exclude") excluded.add(flags[++index]);
    else if (flags[index] === "--workspace") workspace = true;
  }
  return workspace ? allNames.filter((name) => !excluded.has(name)) : selected;
}

// One nextest row as CI's row runs it: the test-thread stack, the
// mcp-test-server fixture, the Console Python, then the selection.
function nextestCommand(ctx, { flags, targets = [], profile, filter = null, partition = null, noTests, stack = null }) {
  const names = selectedPackages(flags, [...ctx.packages.keys()]);
  const env = [];
  const products = new Set(names.map((name) => ctx.packages.get(name)?.product ?? CORE));
  const productStack = [...products].map((product) => ctx.products.find((entry) => entry.name === product)?.testStack).find(Boolean);
  const rowStack = stack ?? productStack ?? null;
  if (rowStack && rowStack !== HOOK_TEST_STACK) env.push(`RUST_MIN_STACK=${rowStack}`);
  if (names.some((name) => ctx.packages.get(name)?.devDependsOnFixture)) env.push('MEERKAT_MCP_TEST_SERVER="$fixture"');
  if (names.includes(CONNECT_PYTHON_PACKAGE)) env.push('TOOLKIT_SHARED_TEST_PYTHON="$(command -v python3)"');
  const argv = [CARGO, "nextest", "run", ...flags, ...targets, "--profile", profile];
  if (filter) argv.push("-E", filter);
  if (partition) argv.push("--partition", partition);
  argv.push(`--no-tests=${noTests}`, ...STATUS);
  const command = commandLine(argv, env);
  return env.includes('MEERKAT_MCP_TEST_SERVER="$fixture"') ? `fixture="$(scripts/mcp-test-server-fixture)" && ${command}` : command;
}

function libBinFlags(pkg) {
  const kinds = new Set(pkg.targets.flatMap((target) => target.kind));
  const flags = [];
  if (LIB_KINDS.some((kind) => kinds.has(kind))) flags.push("--lib");
  if (kinds.has("bin")) flags.push("--bins");
  if (flags.length === 0) throw new Error(`${pkg.name} has no lib or bin target`);
  return flags;
}

function byProduct(names, ctx) {
  return ctx.products
    .map((product) => [product.name, names.filter((name) => (ctx.packages.get(name)?.product ?? CORE) === product.name).sort()])
    .filter(([, group]) => group.length > 0);
}

function reverseClosure(seeds, ctx) {
  const reverse = new Map();
  for (const pkg of ctx.packages.values()) {
    for (const dep of pkg.workspaceDeps) reverse.set(dep, [...(reverse.get(dep) ?? []), pkg.name]);
  }
  const seen = new Set(seeds);
  const queue = [...seeds];
  for (let index = 0; index < queue.length; index += 1) {
    for (const dependent of reverse.get(queue[index]) ?? []) {
      if (!seen.has(dependent)) {
        seen.add(dependent);
        queue.push(dependent);
      }
    }
  }
  return [...seen].sort();
}

// The changed packages that run only their changed test targets (see the
// header), with those targets.
export function narrowedPackages(paths, push, ctx) {
  const narrowed = new Map();
  if (push.mode !== "packages") return narrowed;
  const lockMembers = new Set(push.lock_scope?.members ?? []);
  const escalated = new Set(Object.keys(push.products?.escalations ?? {}));
  const owned = new Map();
  for (const path of paths) {
    for (const name of ctx.packagesForPath(path)) owned.set(name, [...(owned.get(name) ?? []), path]);
  }
  for (const name of push.packages) {
    const mine = owned.get(name) ?? [];
    if (mine.length === 0 || lockMembers.has(name) || escalated.has(ctx.packages.get(name)?.product)) continue;
    const targets = new Map();
    let testOnly = true;
    for (const path of mine) {
      const hits = ctx.testTargetsFor(name, path);
      if (!hits || hits.length === 0) {
        testOnly = false;
        break;
      }
      for (const target of hits) targets.set(target.name, target);
    }
    if (testOnly) narrowed.set(name, [...targets.values()].sort((a, b) => a.name.localeCompare(b.name)));
  }
  return narrowed;
}

function shortName(name) {
  return name.startsWith("meerkat-") ? name.slice("meerkat-".length) : name;
}

// Exclusions CI applies to the test binaries of `names`: binaries and tests
// no hosted runner can run. A local row leaves them out as CI does, and the
// notes name them.
function hostedExclusions(names, notes) {
  const ofRow = (entry) => names.includes(entry.binary.split("::")[0]);
  const binaries = HOSTED_RUNNER_EXCLUDED_BINARIES.filter(ofRow);
  const tests = HOSTED_RUNNER_EXCLUDED_TESTS.filter(ofRow);
  for (const entry of binaries) notes.push(`left out as in CI: ${entry.binary} (${entry.reason.split(":")[0]})`);
  for (const entry of tests) notes.push(`left out as in CI: ${entry.tests.length} test(s) of ${entry.binary} (${entry.reason})`);
  const parts = [];
  if (binaries.length > 0) parts.push(`not (${binaries.map((entry) => `binary_id(${entry.binary})`).join(" | ")})`);
  const filter = excludedTestsFilter(tests.map((entry) => entry.binary), tests);
  if (filter) parts.push(filter);
  return parts.length > 0 ? parts.join(" and ") : null;
}

// The commands for a change, from its paths, its push plan, the pull-request
// plan of its paths outside the narrowed test targets (planChange) and the
// repository context (buildContext): { commands: [{ section, line }], notes,
// summary }. Pure apart from what ctx reads.
export function selectCommands({ paths, push, pr, ctx }) {
  const commands = [];
  const notes = [];
  const add = (section, line) => {
    if (!commands.some((command) => command.line === line)) commands.push({ section, line });
  };

  // Regenerate first: every later command reads the generated tree.
  if (push.machine_authority) {
    add("regenerate", "make machine-codegen");
    add("regenerate", "make protocol-codegen");
  }
  if (push.machine_authority || push.machine_posters) add("regenerate", "node scripts/machine-posters/generate-machine-posters.mjs");
  if (push.generated_contract) add("regenerate", "make regen-schemas");

  // The packages to test: the changed ones less the narrowed, and their
  // reverse-dependency closure; the planner's own closure when nothing
  // narrowed (checked against this one).
  const narrowed = narrowedPackages(paths, push, ctx);
  const seeds = push.packages.filter((name) => !narrowed.has(name));
  const closure = push.mode === "workspace" ? [...push.closure].sort() : reverseClosure(seeds, ctx);
  if (narrowed.size === 0 && closure.join("\n") !== [...push.closure].sort().join("\n")) {
    throw new Error("internal error: the reverse-dependency closure differs from the CI planner's");
  }
  const inClosure = new Set(closure);
  const seedSet = new Set(seeds);
  for (const [name, targets] of narrowed) {
    notes.push(`${name}: only test targets changed; runs ${targets.map((target) => target.name).join(", ")} without reverse dependencies`);
  }

  // Clippy: the push plan's rows, CI's closure check beyond them, wasm.
  for (const shard of push.shards ?? []) {
    add("clippy", commandLine([CARGO, "clippy", ...shard.package_flags.split(" "), "--no-deps", ...shard.targets.split(" "), "--all-features", "--", "-D", "warnings"]));
  }
  const linted = new Set((push.shards ?? []).flatMap((shard) => shard.packages));
  for (const [, names] of byProduct(closure, ctx)) {
    if (names.some((name) => !linted.has(name))) add("clippy", commandLine([CARGO, "check", ...names.flatMap((name) => ["-p", name]), "--all-features"]));
  }
  if (pr.wasm) add("clippy", "make wasm-check");

  // Tests. The plan's rows of the hook's other lanes (HeadCanonical, e2e-fast)
  // run the binaries they name in scope.
  const specialRows = (push.closure_test_rows ?? []).filter((row) => row.lane !== "tests" && row.lane !== "lib").filter((row) =>
    row.binaries.some((binary) => {
      const [name, target] = binary.split("::");
      return inClosure.has(name) || (narrowed.get(name) ?? []).some((entry) => entry.name === target);
    }),
  );
  const specialBinaries = new Set(specialRows.flatMap((row) => row.binaries));
  const narrowedRun = new Map();
  for (const [name, targets] of narrowed) {
    if (inClosure.has(name)) continue;
    const kept = targets.filter((target) => {
      if (specialBinaries.has(`${name}::${target.name}`)) return false;
      if (ctx.profileSelects(TEST_PROFILE, { kind: "test", binary: target.name, package: name })) return true;
      notes.push(`${name} --test ${target.name}: the ${TEST_PROFILE} profile leaves it to its own lane (see .config/nextest.toml)`);
      return false;
    });
    if (kept.length === 0) continue;
    narrowedRun.set(name, new Set(kept.map((target) => target.name)));
    const features = [...new Set(kept.flatMap((target) => target.requiredFeatures))].sort();
    add(
      "tests",
      nextestCommand(ctx, {
        flags: ["-p", name, ...(features.length > 0 ? ["--features", features.join(",")] : [])],
        targets: kept.flatMap((target) => ["--test", target.name]),
        profile: TEST_PROFILE,
        noTests: "fail",
      }),
    );
  }
  for (const [, names] of byProduct(closure, ctx)) {
    add(
      "tests",
      nextestCommand(ctx, {
        flags: names.flatMap((name) => ["-p", name]),
        profile: TEST_PROFILE,
        filter: hostedExclusions(names, notes),
        noTests: "pass",
      }),
    );
  }
  for (const suite of FEATURE_UNIT_SUITES) {
    if (!inClosure.has(suite.package)) continue;
    const filters = featureSuiteFilters(suite);
    add(
      "tests",
      nextestCommand(ctx, {
        flags: featureSuiteFlags(suite).split(" "),
        targets: libBinFlags(ctx.packages.get(suite.package)),
        profile: ROW_PROFILE,
        filter: filters.filter ?? null,
        noTests: "pass",
        stack: suite.rustMinStack ?? null,
      }),
    );
    if (filters.known_reds) {
      notes.push(`${shortName(suite.package)}[${suite.id}]: CI also checks that its ${filters.known_red_count} known reds still fail`);
    }
  }
  const suiteDefinitions = new Map(INTEGRATION_SUITES.map((suite) => [suite.name ?? shortName(suite.package), suite]));
  for (const row of push.integration_suites ?? []) {
    const suite = suiteDefinitions.get(row.name);
    if (!suite) throw new Error(`the CI plan names integration suite ${row.name}, which INTEGRATION_SUITES does not define`);
    const selected =
      push.mode === "workspace" ||
      suite.triggers.some((name) => seedSet.has(name)) ||
      (suite.paths ?? []).some((prefix) => paths.some((path) => path.startsWith(prefix)));
    if (!selected) {
      notes.push(`integration suite ${row.name}: only test targets of its trigger packages changed`);
      continue;
    }
    const hasFeatures = row.package_flags.split(" ").includes("--features");
    if (!hasFeatures && inClosure.has(row.packages[0])) continue;
    add(
      "tests",
      nextestCommand(ctx, {
        flags: row.package_flags.split(" "),
        targets: row.test_flags ? row.test_flags.split(" ") : ["--test", "*"],
        profile: ROW_PROFILE,
        noTests: "fail",
      }),
    );
  }
  for (const row of specialRows) {
    add(
      "tests",
      nextestCommand(ctx, {
        flags: row.package_flags.split(" "),
        targets: row.test_flags.split(" "),
        profile: row.profile,
        filter: row.filter || null,
        partition: row.partition || null,
        noTests: row.no_tests,
        stack: row.rust_min_stack ?? null,
      }),
    );
  }

  // Readers of the changed non-Rust inputs (and the whole-checkout readers
  // every path selects), less the ones a run above already covers.
  const readerRows = [];
  for (const selection of readerSelectionsFor(paths, ctx.inventory, { excludedRoots: productRootPrefixes(ctx.products) })) {
    const live = selection.features.filter((feature) => LIVE_ONLY_FEATURES.includes(feature));
    if (live.length > 0) {
      notes.push(`reader ${selection.package} ${selection.target} needs ${live.join(", ")} (live provider credentials): not run`);
      continue;
    }
    const colon = selection.target.indexOf(":");
    const kind = colon < 0 ? selection.target : selection.target.slice(0, colon);
    const name = colon < 0 ? null : selection.target.slice(colon + 1);
    const whole = selection.scope === "package" || kind === "build";
    let targets;
    if (whole) targets = [];
    else if (kind === "lib") targets = ["--lib"];
    else if (kind === "bin") targets = ["--bin", name];
    else if (kind === "test") targets = ["--test", name];
    else if (kind === "example") {
      // An example program reads its inputs when run, which no test does.
      if (!selection.tests?.length) continue;
      targets = ["--example", name];
    } else throw new Error(`reader ${selection.package} ${selection.target}: no test runs a ${kind} target`);
    const plain = selection.features.length === 0;
    const selectedByProfile = kind !== "test" || ctx.profileSelects(TEST_PROFILE, { kind: "test", binary: name, package: selection.package });
    if (plain && inClosure.has(selection.package) && kind !== "example" && selectedByProfile) continue;
    if (plain && kind === "test" && narrowedRun.get(selection.package)?.has(name)) continue;
    readerRows.push({
      package: selection.package,
      features: selection.features,
      whole,
      targets,
      filter: selection.tests?.length ? `test(/(^|::)(${selection.tests.join("|")})$/)` : null,
    });
  }
  for (const row of readerRows) {
    // An unfiltered run of the whole package with the same features runs it.
    const subsumed = readerRows.some(
      (wide) => wide !== row && wide.whole && !wide.filter && wide.package === row.package && wide.features.join(",") === row.features.join(","),
    );
    if (subsumed) continue;
    add(
      "tests",
      nextestCommand(ctx, {
        flags: ["-p", row.package, ...(row.features.length > 0 ? ["--features", row.features.join(",")] : [])],
        targets: row.targets,
        profile: row.whole ? TEST_PROFILE : ROW_PROFILE,
        filter: row.filter,
        noTests: "fail",
      }),
    );
  }

  // TLC for the touched machines (see the header).
  if (push.machine_authority) {
    const tlc = tlcSelection(paths, ctx.tlc.registry, ctx.readText);
    if (ctx.tlc.lane.error) tlc.all.push(`the lane's parts could not be read: ${ctx.tlc.lane.error}`);
    if (tlc.all.length > 0) {
      add("tlc", "make machine-verify");
      notes.push(`TLC: the whole canonical lane, because ${tlc.all.join("; ")}`);
    } else if (tlc.machines.length + tlc.compositions.length > 0) {
      const lane = ctx.tlc.lane;
      const skip = tlc.compositions.filter((slug) => lane.skipCompositions.includes(slug));
      add(
        "tlc",
        commandLine([
          CARGO, "xtask", "machine-verify", ...lane.verifyFlags,
          ...tlc.machines.flatMap((slug) => ["--machine", slug]),
          ...tlc.compositions.flatMap((slug) => ["--composition", slug]),
          ...skip.flatMap((slug) => ["--skip-tlc-composition", slug]),
        ]),
      );
      const dirs = new Set([
        ...tlc.machines.map((slug) => `specs/machines/${slug}`),
        ...tlc.compositions.map((slug) => `specs/compositions/${slug}`),
      ]);
      for (const [part, runners] of lane.parts) {
        if (runners.some((runner) => dirs.has(lane.runners.get(runner)))) add("tlc", commandLine(["bash", TLC_LANE, "--part", part]));
      }
      notes.push(`TLC: ${[...tlc.machines, ...tlc.compositions].join(", ")}`);
    } else {
      notes.push("TLC: no spec input changed yet; if regeneration changes specs/, run this script again");
    }
  }

  // SDK suites (CI's sdk-host lane builds meerkat-rpc for their smoke tests).
  const hostSuites = push.generated_contract || push.sdk_host;
  const webSuite = push.generated_contract || pr.example_web;
  if (hostSuites) {
    add("sdk", commandLine([CARGO, "build", "-p", "meerkat-rpc"]));
    add("sdk", "make test-sdk-python");
    add("sdk", "make test-sdk-typescript");
  }
  if (webSuite) add("sdk", "make test-sdk-web");

  // What CI runs beyond these commands, so a green local run is not read as
  // the whole pass.
  const ciOnly = [];
  if (push.governance_gates) ciOnly.push("the typed governance gates");
  if (push.rust_changed || push.governance_gates) ciOnly.push("the runtime authority bypass audit");
  if (push.generated_contract) ciOnly.push("the generated-contract freshness ratchets");
  if (push.machine_authority) ciOnly.push("the machine and protocol drift checks");
  if ((push.feature_check_jobs ?? []).length > 0) ciOnly.push("the feature-combination checks");
  if (push.rustdoc) ciOnly.push("rustdoc");
  if (push.governed_jsonl || push.mcp_confinement_positive) ciOnly.push("the exact-profile guard lanes");
  if (push.bazel_graph) ciOnly.push("the Bazel graph check");
  if (pr.examples_browser) ciOnly.push("the example browser suites");
  for (const lane of push.products?.lanes ?? []) ciOnly.push(lane.replaceAll("_", "-"));
  for (const product of push.products?.floor_checks ?? []) ciOnly.push(`the ${product} published-floor check`);
  if (ciOnly.length > 0) notes.push(`CI also runs ${ciOnly.join(", ")}`);
  const productPaths = new Map();
  for (const path of paths) {
    const product = productOfPath(path, ctx.products);
    if (product !== CORE && ctx.packagesForPath(path).length === 0) productPaths.set(product, (productPaths.get(product) ?? 0) + 1);
  }
  for (const [product, count] of productPaths) {
    notes.push(`${count} changed path(s) under ${product}/ outside its crates: that product's own CI lanes cover them`);
  }

  return {
    commands,
    notes,
    summary: {
      mode: push.mode,
      reason: push.reason,
      changed: push.packages.length,
      closure: byProduct(closure, ctx).map(([product, names]) => `${product} ${names.length}`),
      narrowed: [...narrowed.keys()],
    },
  };
}

// ----------------------------------------------------------------------------
// The repository context selectCommands reads.

export function buildContext() {
  const metadata = readMetadata();
  const packages = workspacePackages(metadata);
  const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
  const dirs = packageDirs(packages);
  const products = loadProducts();
  const nextestText = readFileSync(resolve(root, ".config/nextest.toml"), "utf8");
  const profileFilters = new Map();
  const readText = (path) => {
    const file = resolve(root, path);
    return existsSync(file) ? readFileSync(file, "utf8") : null;
  };
  let lane;
  try {
    lane = parseTlcLane(readFileSync(resolve(root, TLC_LANE), "utf8"));
  } catch (error) {
    lane = { error: error.message };
  }
  const sourceCache = new Map();
  const sourcesOf = (pkg, target) => {
    const key = `${pkg.name}\0${target.kind.join(",")}\0${target.name}`;
    if (!sourceCache.has(key)) sourceCache.set(key, testSourcePaths(target, pkg));
    return sourceCache.get(key);
  };
  return {
    products,
    inventory: loadNonRustReaders(),
    readText,
    tlc: { lane, registry: machineRegistry(resolve(root, "specs")) },
    packages: new Map(
      packages.map((pkg) => [
        pkg.name,
        {
          name: pkg.name,
          product: productOfPath(packageDir(pkg), products),
          targets: pkg.targets.map((target) => ({ name: target.name, kind: target.kind })),
          devDependsOnFixture: pkg.dependencies.some((dep) => dep.name === FIXTURE_PACKAGE && dep.kind === "dev"),
          workspaceDeps: pkg.dependencies.filter((dep) => dep.source === null && byName.has(dep.name)).map((dep) => dep.name),
        },
      ]),
    ),
    packagesForPath: (path) => packagesForFile(path, dirs).map((pkg) => pkg.name),
    // The test targets whose sources hold `path` when it is a Rust file under
    // the package's tests/ that no other target compiles; null otherwise.
    testTargetsFor: (name, path) => {
      const pkg = byName.get(name);
      const dir = packageDir(pkg);
      if (!path.endsWith(".rs") || !path.startsWith(dir === "." ? "tests/" : `${dir}/tests/`) || !existsSync(resolve(root, path))) return null;
      const holders = pkg.targets.filter((target) => sourcesOf(pkg, target).has(path));
      if (holders.length === 0 || holders.some((target) => !target.kind.includes("test"))) return null;
      return holders.map((target) => ({ name: target.name, requiredFeatures: target["required-features"] ?? [] }));
    },
    profileSelects: (profile, binary) => {
      if (!profileFilters.has(profile)) profileFilters.set(profile, parseFilter(profileFilter(nextestText, profile)));
      return profileFilters.get(profile)(binary);
    },
  };
}

// ----------------------------------------------------------------------------
// Running.

// A digest of the working tree's difference from HEAD: tracked changes and
// the untracked files' contents.
export function worktreeDigest(repo) {
  const hash = createHash("sha256");
  hash.update(git(repo, ["diff", "--binary", "--no-ext-diff", "--no-color", "HEAD", "--"]));
  const untracked = git(repo, ["ls-files", "--others", "--exclude-standard", "-z"]).split("\0").filter(Boolean);
  for (const path of untracked) {
    hash.update(`\0${path}\0`);
    hash.update(git(repo, ["hash-object", "--", path]));
  }
  return hash.digest("hex");
}

// Run each command with bash in `repo`, in order; stop at the first failure
// (its status) or, once the regeneration commands ran, when they changed the
// working tree (3). `spawn` and `digest` are injectable for the self-test.
export function runCommands(commands, { repo, spawn = spawnSync, digest = worktreeDigest, log = (text) => process.stderr.write(text) }) {
  const regenerating = commands.filter((command) => command.section === "regenerate").length;
  const before = regenerating > 0 ? digest(repo) : null;
  for (const [index, command] of commands.entries()) {
    log(`affected-tests: [${index + 1}/${commands.length}] ${command.line}\n`);
    const result = spawn("bash", ["-c", command.line], { cwd: repo, stdio: "inherit" });
    const status = result.status ?? 1;
    if (status !== 0) {
      log(`affected-tests: failed (exit ${status}): ${command.line}\n`);
      return status;
    }
    if (index + 1 === regenerating && digest(repo) !== before) {
      log("affected-tests: regeneration changed the working tree; review the regenerated files, then run this script again (the change, and so the commands, include them)\n");
      return 3;
    }
  }
  log(`affected-tests: all ${commands.length} command(s) passed\n`);
  return 0;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    process.stdout.write(USAGE);
    return 0;
  }
  const change = collectChange(root, args);
  const log = (text) => process.stderr.write(text);
  if (change.paths.length === 0) {
    log(`affected-tests: ${change.label}: no changed paths\n`);
    return 0;
  }
  const ctx = buildContext();
  const { push, pr } = planChange(root, change, ctx);
  const { commands, notes, summary } = selectCommands({ paths: change.paths, push, pr, ctx });
  log(`affected-tests: ${change.label}: ${change.paths.length} changed path(s)\n`);
  log(`  plan: ${summary.mode} (${summary.reason}); changed packages ${summary.changed}; tested packages ${summary.closure.join(", ") || "none"}\n`);
  for (const note of notes) log(`  ${note}\n`);
  if (!args.run) {
    for (const command of commands) process.stdout.write(`${command.line}\n`);
    log(`  ${commands.length} command(s); --run runs them in order and stops at the first failure\n`);
    return 0;
  }
  return runCommands(commands, { repo: root, log });
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`affected-tests: ${error?.message ?? error}\n`);
    process.exitCode = error instanceof UsageError ? 2 : 1;
  }
}
