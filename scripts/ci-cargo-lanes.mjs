#!/usr/bin/env node
// Changed-path classifier for the GitHub-hosted Cargo PR CI (ci.yml).
//
// Reads the changed paths of a push or pull request and emits the lane plan:
// which workspace packages get a clippy lane and a unit lane, how they are
// packed into parallel shards, the reverse-dependency closure for the
// closure-check lane, and the boolean flags for the conditional lanes.
//
// The plan fails closed. Any path that can change what Cargo compiles
// (Rust sources, manifests, the lock, Cargo/nextest configuration, the
// toolchain pin, the build wrapper scripts, this classifier, or ci.yml)
// yields at least one clippy and one unit lane. A Rust-relevant path that
// cannot be attributed to one workspace package, or a diff whose base cannot
// be established, escalates to the whole workspace instead of to nothing.
// The old `changed-paths` BuildBuddy mode reported unrun lanes as passed for
// three weeks; this script exits non-zero rather than emit an empty plan for
// a build-relevant change.
//
// usage:
//   ci-cargo-lanes.mjs [--base <rev> --head <rev>] [--max-shards N]
//                      [--format json|github] [--] [changed-path ...]
//   ci-cargo-lanes.mjs --paths-from-stdin [...]
//
// Without explicit paths, --base/--head, or stdin, the plan is the whole
// workspace (no diff base means no evidence of what did not change).

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import {
  normalizePath,
  packageDir,
  packageDirs,
  packageForFile,
  readMetadata,
  root,
  workspacePackages,
} from "./rust-test-selector.mjs";

const NULL_SHA = "0000000000000000000000000000000000000000";

function parseArgs(argv) {
  const args = {
    base: "",
    head: "HEAD",
    maxShards: 6,
    format: "json",
    pathsFromStdin: false,
    paths: [],
    explicitPaths: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--base") {
      args.base = argv[++i] ?? "";
    } else if (arg === "--head") {
      args.head = argv[++i] ?? "HEAD";
    } else if (arg === "--max-shards") {
      args.maxShards = Number.parseInt(argv[++i] ?? "", 10);
      if (!Number.isInteger(args.maxShards) || args.maxShards < 1) {
        throw new Error("--max-shards requires a positive integer");
      }
    } else if (arg === "--format") {
      args.format = argv[++i] ?? "";
      if (!["json", "github"].includes(args.format)) {
        throw new Error("--format must be json or github");
      }
    } else if (arg === "--paths-from-stdin") {
      args.pathsFromStdin = true;
      args.explicitPaths = true;
    } else if (arg === "--") {
      args.paths.push(...argv.slice(i + 1));
      args.explicitPaths = true;
      break;
    } else if (arg === "--help" || arg === "-h") {
      process.stdout.write(
        "usage: ci-cargo-lanes.mjs [--base <rev> --head <rev>] [--max-shards N] [--format json|github] [--paths-from-stdin] [--] [changed-path ...]\n",
      );
      process.exit(0);
    } else if (arg.startsWith("-")) {
      throw new Error(`unknown argument: ${arg}`);
    } else {
      args.paths.push(arg);
      args.explicitPaths = true;
    }
  }
  return args;
}

function git(argsList, options = {}) {
  return execFileSync("git", argsList, { cwd: root, encoding: "utf8", ...options });
}

function revisionExists(rev) {
  const result = spawnSync("git", ["cat-file", "-e", `${rev}^{commit}`], { cwd: root });
  return result.status === 0;
}

// Returns { paths, reason }. `paths` is null when the diff cannot be
// established, which the caller treats as "everything changed".
function collectChangedPaths(args) {
  if (args.pathsFromStdin) {
    const text = readFileSync(0, "utf8");
    const paths = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    return { paths: [...paths, ...args.paths], reason: "explicit path list" };
  }
  if (args.explicitPaths) {
    return { paths: args.paths, reason: "explicit path list" };
  }
  if (!args.base || args.base === NULL_SHA) {
    return { paths: null, reason: "no diff base supplied" };
  }
  if (!revisionExists(args.base)) {
    const fetched = spawnSync("git", ["fetch", "--no-tags", "--depth=1", "origin", args.base], {
      cwd: root,
      stdio: "inherit",
    });
    if (fetched.status !== 0 || !revisionExists(args.base)) {
      return { paths: null, reason: `diff base ${args.base} is not available` };
    }
  }
  if (!revisionExists(args.head)) {
    return { paths: null, reason: `diff head ${args.head} is not available` };
  }
  const output = git(["diff", "--name-only", "--diff-filter=ACMRD", args.base, args.head, "--"]);
  return {
    paths: output.split(/\r?\n/).map((line) => line.trim()).filter(Boolean),
    reason: `git diff ${args.base}..${args.head}`,
  };
}

// Paths whose change alters the compile graph of every package.
function isGlobalPath(path) {
  if (
    path === "Cargo.toml" ||
    path === "Cargo.lock" ||
    path === "rust-toolchain.toml" ||
    path === "rust-toolchain" ||
    path === "Makefile" ||
    path === ".config/nextest.toml" ||
    path === "scripts/repo-cargo" ||
    path === "scripts/agent-gate" ||
    path === "scripts/rust-lane-doctor" ||
    path === "scripts/ci-cargo-lanes.mjs" ||
    path === "scripts/ci-cargo-lanes-selftest.mjs" ||
    path === "scripts/rust-test-selector.mjs" ||
    path === "scripts/ci-pin-rust-toolchain-env" ||
    path === ".github/workflows/ci.yml"
  ) {
    return true;
  }
  return (
    path.startsWith(".cargo/") ||
    path.startsWith("scripts/cargo-") ||
    path.startsWith(".github/actions/setup-rust-ci/")
  );
}

function isRustSourcePath(path) {
  return path.endsWith(".rs") || path.endsWith("/Cargo.toml") || path === "Cargo.toml";
}

function runClassifier(script, paths) {
  if (paths.length === 0) return false;
  const result = spawnSync(join(root, script), ["--", ...paths], {
    cwd: root,
    encoding: "utf8",
  });
  if (result.status === 0) return true;
  if (result.status === 1) return false;
  throw new Error(
    `${script} failed (status ${result.status}): ${result.stderr || result.stdout || "no output"}`,
  );
}

function rustLineCount(dir) {
  let lines = 0;
  const stack = [dir];
  while (stack.length) {
    const current = stack.pop();
    let entries;
    try {
      entries = readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (entry.name === "target" || entry.name === "node_modules" || entry.name.startsWith(".")) continue;
      const full = join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(full);
      } else if (entry.isFile() && entry.name.endsWith(".rs")) {
        try {
          const text = readFileSync(full, "utf8");
          lines += text.split("\n").length;
        } catch {
          // Unreadable sources are reported by rustc, not by the planner.
        }
      }
    }
  }
  return lines;
}

function shortName(name) {
  return name.startsWith("meerkat-") ? name.slice("meerkat-".length) : name;
}

// Longest-processing-time first bin packing of packages into at most
// `maxShards` lanes by Rust line count. Deterministic: ties break by name.
function packShards(pkgs, weights, maxShards) {
  const sorted = [...pkgs].sort((a, b) => weights.get(b) - weights.get(a) || a.localeCompare(b));
  const shardCount = Math.min(maxShards, sorted.length);
  const bins = Array.from({ length: shardCount }, () => ({ packages: [], weight: 0 }));
  for (const pkg of sorted) {
    bins.sort((a, b) => a.weight - b.weight || a.packages.length - b.packages.length);
    bins[0].packages.push(pkg);
    bins[0].weight += weights.get(pkg);
  }
  return bins
    .filter((bin) => bin.packages.length > 0)
    .map((bin) => {
      const names = [...bin.packages].sort();
      // Label by the heaviest member so a lane reads as what dominates it.
      const heaviest = [...names].sort((a, b) => weights.get(b) - weights.get(a) || a.localeCompare(b))[0];
      const label = names.length === 1
        ? shortName(names[0])
        : `${shortName(heaviest)}+${names.length - 1}`;
      return {
        name: label,
        packages: names,
        package_flags: names.map((name) => `-p ${name}`).join(" "),
        rust_lines: bin.weight,
      };
    })
    .sort((a, b) => b.rust_lines - a.rust_lines || a.name.localeCompare(b.name));
}

function plan(args) {
  const metadata = readMetadata();
  const packages = workspacePackages(metadata);
  const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
  const byId = new Map(packages.map((pkg) => [pkg.id, pkg]));
  const dirs = packageDirs(packages);
  const allNames = packages.map((pkg) => pkg.name).sort();

  const reverse = new Map(packages.map((pkg) => [pkg.id, new Set()]));
  for (const pkg of packages) {
    for (const dep of pkg.dependencies) {
      if (dep.source !== null) continue;
      const depPkg = byName.get(dep.name);
      if (depPkg) reverse.get(depPkg.id)?.add(pkg.id);
    }
  }

  const { paths: rawPaths, reason: pathReason } = collectChangedPaths(args);
  const changed = rawPaths === null ? null : [...new Set(rawPaths.map(normalizePath))].sort();

  const result = {
    schema_version: 1,
    diff: pathReason,
    changed_paths: changed ?? [],
    rust_changed: false,
    mode: "none",
    reason: "",
    packages: [],
    closure: [],
    shards: [],
    unmapped_rust_paths: [],
    generated_contract: false,
    machine_authority: false,
    wasm: false,
    sdk_host: false,
    docs_only: false,
  };

  let workspaceReason = "";
  const seeds = new Set();

  if (changed === null) {
    workspaceReason = pathReason;
  } else {
    for (const path of changed) {
      if (isGlobalPath(path)) {
        workspaceReason ||= `global build configuration changed: ${path}`;
        continue;
      }
      if (!isRustSourcePath(path)) {
        // Non-Rust paths can still select a package: fixtures, SQL, TOML,
        // JSON and snapshot data a crate reads at test time, and the
        // embedded inputs (for example the platform skill sources the facade
        // compiles in through include macros) that the selector maps to a
        // package from outside its directory. Only documentation that lives
        // inside the crate directory is not a build input.
        const owner = packageForFile(path, dirs);
        if (owner) {
          const insideOwner = path.startsWith(`${packageDir(owner)}/`);
          const isDoc = /\.(md|mdx)$/.test(path);
          if (!insideOwner || !isDoc) seeds.add(owner.name);
        }
        continue;
      }
      const owner = packageForFile(path, dirs);
      if (owner) {
        seeds.add(owner.name);
      } else {
        result.unmapped_rust_paths.push(path);
      }
    }
    if (result.unmapped_rust_paths.length > 0) {
      workspaceReason ||= `Rust path outside any workspace package: ${result.unmapped_rust_paths[0]}`;
    }
  }

  if (changed !== null) {
    result.generated_contract = runClassifier("scripts/generated-contract-ratchet-changed", changed);
    result.machine_authority = runClassifier("scripts/machine-authority-changed", changed);
    result.sdk_host = changed.some(
      (path) =>
        path.startsWith("sdks/python/") ||
        path.startsWith("sdks/typescript/") ||
        path.startsWith("sdks/test-fixtures/") ||
        path.startsWith("tools/sdk-codegen/") ||
        path.startsWith("tools/sdk-builder/"),
    );
    result.wasm = changed.some(
      (path) =>
        path.startsWith("meerkat-web-runtime/") ||
        path.startsWith("meerkat-contracts/") ||
        path.startsWith("meerkat-cli/src/web_runtime_template/"),
    );
  } else {
    result.generated_contract = true;
    result.machine_authority = true;
    result.sdk_host = true;
    result.wasm = true;
  }

  const weights = new Map(
    packages.map((pkg) => [pkg.name, rustLineCount(resolve(root, packageDir(pkg)))]),
  );

  if (workspaceReason) {
    result.rust_changed = true;
    result.mode = "workspace";
    result.reason = workspaceReason;
    result.packages = allNames;
    result.closure = allNames;
    result.wasm = true;
  } else if (seeds.size > 0) {
    result.rust_changed = true;
    result.mode = "packages";
    result.reason = `${seeds.size} workspace package(s) own changed paths`;
    result.packages = [...seeds].sort();
    const seen = new Set(result.packages.map((name) => byName.get(name).id));
    const queue = [...seen];
    for (let i = 0; i < queue.length; i += 1) {
      for (const dependent of reverse.get(queue[i]) ?? []) {
        if (!seen.has(dependent)) {
          seen.add(dependent);
          queue.push(dependent);
        }
      }
    }
    result.closure = [...seen].map((id) => byId.get(id).name).sort();
    if (result.closure.includes("meerkat-web-runtime")) result.wasm = true;
  } else {
    result.mode = "none";
    result.reason = "no Rust build-relevant paths changed";
    result.docs_only = changed !== null && changed.every(
      (path) => /^(docs\/|docs-internal\/|CHANGELOG\.md|README\.md|AGENTS\.md|CLAUDE\.md|.*\.mdx?$)/.test(path),
    );
  }

  if (result.rust_changed) {
    result.shards = packShards(result.packages, weights, args.maxShards);
    if (result.shards.length === 0) {
      throw new Error("internal error: Rust-relevant change produced no lanes");
    }
    const covered = new Set(result.shards.flatMap((shard) => shard.packages));
    for (const name of result.packages) {
      if (!covered.has(name)) throw new Error(`internal error: package ${name} not covered by any shard`);
    }
  }

  result.closure_flags = result.closure.map((name) => `-p ${name}`).join(" ");
  result.closure_beyond_packages = result.closure.filter((name) => !result.packages.includes(name));
  return result;
}

function githubOutput(result) {
  const lines = [];
  const scalar = (key, value) => lines.push(`${key}=${value}`);
  scalar("rust_changed", String(result.rust_changed));
  scalar("mode", result.mode);
  scalar("reason", result.reason.replaceAll("\n", " "));
  scalar("generated_contract", String(result.generated_contract));
  scalar("machine_authority", String(result.machine_authority));
  scalar("wasm", String(result.wasm));
  scalar("sdk_host", String(result.sdk_host));
  scalar("docs_only", String(result.docs_only));
  scalar("package_count", String(result.packages.length));
  scalar("closure_count", String(result.closure.length));
  scalar("closure_flags", result.closure_flags);
  scalar("shard_count", String(result.shards.length));
  // A matrix must never be empty when the lanes are conditioned on
  // rust_changed; keep a placeholder so `fromJSON` stays well-formed and the
  // consumer job's `if:` decides whether it runs.
  const matrix = result.shards.length > 0
    ? result.shards.map((shard) => ({ name: shard.name, packages: shard.package_flags }))
    : [{ name: "none", packages: "" }];
  scalar("shard_matrix", JSON.stringify({ include: matrix }));
  return `${lines.join("\n")}\n`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const result = plan(args);
  if (args.format === "github") {
    process.stdout.write(githubOutput(result));
  } else {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  }
  process.stderr.write(
    `ci-cargo-lanes: mode=${result.mode} packages=${result.packages.length} closure=${result.closure.length} shards=${result.shards.length} (${result.reason})\n`,
  );
}

try {
  main();
} catch (error) {
  process.stderr.write(`ci-cargo-lanes: ${error?.message ?? error}\n`);
  process.exit(1);
}
