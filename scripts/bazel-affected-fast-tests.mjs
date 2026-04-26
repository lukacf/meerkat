#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
}).trim();

function gitLines(args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" })
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function parseArgs() {
  const paths = [];
  let emptyIfNoLabels = false;
  let mode = "affected";
  let kind = "test";
  for (const arg of process.argv.slice(2)) {
    if (arg === "--empty-if-no-labels") {
      emptyIfNoLabels = true;
    } else if (arg === "--owned") {
      mode = "owned";
    } else if (arg === "--affected") {
      mode = "affected";
    } else if (arg === "--build") {
      kind = "build";
    } else if (arg === "--clippy") {
      kind = "clippy";
    } else if (arg === "--test") {
      kind = "test";
    } else if (arg === "--help" || arg === "-h") {
      console.log("usage: bazel-affected-fast-tests.mjs [--owned|--affected] [--test|--build|--clippy] [--empty-if-no-labels] [changed-path ...]");
      process.exit(0);
    } else {
      paths.push(arg);
    }
  }
  return { emptyIfNoLabels, kind, mode, paths };
}

const args = parseArgs();

function cargoManifestFiles() {
  return gitLines(["ls-files", "Cargo.lock", ":(glob)**/Cargo.toml"]);
}

function metadataFingerprint() {
  const hash = createHash("sha256");
  hash.update(root);
  for (const file of cargoManifestFiles().sort()) {
    const stat = statSync(resolve(root, file));
    hash.update("\0");
    hash.update(file);
    hash.update("\0");
    hash.update(String(stat.size));
    hash.update("\0");
    hash.update(String(stat.mtimeMs));
  }
  return hash.digest("hex");
}

function readMetadata() {
  const cacheRoot = resolve(
    process.env.XDG_CACHE_HOME || resolve(process.env.HOME || root, ".cache"),
    "meerkat",
    "bazel-selector-metadata",
  );
  const fingerprint = metadataFingerprint();
  const cachePath = resolve(cacheRoot, `${fingerprint}.json`);
  try {
    return JSON.parse(readFileSync(cachePath, "utf8"));
  } catch {
    // Refresh below.
  }

  mkdirSync(cacheRoot, { recursive: true });
  const lockPath = resolve(cacheRoot, `${fingerprint}.lock`);
  try {
    mkdirSync(lockPath);
    try {
      const metadata = JSON.parse(
        execFileSync("./scripts/repo-cargo", ["metadata", "--format-version=1"], {
          cwd: root,
          encoding: "utf8",
          maxBuffer: 64 * 1024 * 1024,
        }),
      );
      const tmpPath = resolve(cacheRoot, `${fingerprint}.${process.pid}.tmp`);
      writeFileSync(tmpPath, JSON.stringify(metadata));
      renameSync(tmpPath, cachePath);
      return metadata;
    } finally {
      rmSync(lockPath, { force: true, recursive: true });
    }
  } catch {
    const sleepBuffer = new SharedArrayBuffer(4);
    const sleepArray = new Int32Array(sleepBuffer);
    for (let attempt = 0; attempt < 100; attempt += 1) {
      try {
        return JSON.parse(readFileSync(cachePath, "utf8"));
      } catch {
        Atomics.wait(sleepArray, 0, 0, 100);
      }
    }
    return JSON.parse(
      execFileSync("./scripts/repo-cargo", ["metadata", "--format-version=1"], {
        cwd: root,
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
      }),
    );
  }
}

function changedFiles() {
  if (args.paths.length) return args.paths;

  const files = new Set([
    ...gitLines(["diff", "--name-only", "HEAD", "--"]),
    ...gitLines(["diff", "--name-only", "--cached", "--"]),
  ]);
  return [...files].sort();
}

const metadata = readMetadata();

const workspaceMembers = new Set(metadata.workspace_members);
const packages = metadata.packages.filter(
  (pkg) => pkg.source === null && workspaceMembers.has(pkg.id),
);
const byId = new Map(packages.map((pkg) => [pkg.id, pkg]));
const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
const packageDir = (pkg) => relative(root, dirname(pkg.manifest_path)) || ".";
const packageDirs = packages
  .map((pkg) => [packageDir(pkg), pkg])
  .sort((a, b) => b[0].length - a[0].length);

const reverseDeps = new Map(packages.map((pkg) => [pkg.id, new Set()]));
for (const pkg of packages) {
  for (const dep of pkg.dependencies) {
    if (dep.source !== null) continue;
    const depPkg = byName.get(dep.name);
    if (depPkg) reverseDeps.get(depPkg.id)?.add(pkg.id);
  }
}

function packageForFile(file) {
  const normalized = file.replaceAll("\\", "/").replace(/^\.\//, "");
  for (const [dir, pkg] of packageDirs) {
    if (dir === ".") continue;
    if (normalized === `${dir}/Cargo.toml` || normalized.startsWith(`${dir}/`)) {
      return pkg;
    }
  }
  return null;
}

function hasFastSuite(pkg) {
  const buildFile = resolve(root, packageDir(pkg), "BUILD.bazel");
  return existsSync(buildFile) && readFileSync(buildFile, "utf8").includes('name = "fast_tests"');
}

function crateName(name) {
  return name.replaceAll("-", "_");
}

function testSourcePaths(target, pkg) {
  const packageRoot = dirname(pkg.manifest_path);
  const seen = new Set();
  const paths = new Set();

  function visit(file) {
    if (seen.has(file)) return;
    seen.add(file);
    if (!file.startsWith(`${packageRoot}/`)) return;
    const rel = relative(root, file).replaceAll("\\", "/");
    if (!rel || rel.startsWith("..")) return;
    paths.add(rel);

    const source = readFileSync(file, "utf8");
    const lineRe = /#[ \t]*\[[ \t]*path[ \t]*=[ \t]*"([^"]+)"[ \t]*\][ \t]*|(?:^|\n)[ \t]*(?:pub[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;/g;
    let pendingPath = null;
    for (const match of source.matchAll(lineRe)) {
      if (match[1]) {
        pendingPath = match[1];
        continue;
      }
      const modName = match[2];
      if (!modName) continue;
      if (pendingPath) {
        visit(resolve(dirname(file), pendingPath));
        pendingPath = null;
        continue;
      }

      const flat = resolve(dirname(file), `${modName}.rs`);
      const nested = resolve(dirname(file), modName, "mod.rs");
      try {
        if (statSync(flat).isFile()) {
          visit(flat);
          continue;
        }
      } catch {
        // Try nested module layout below.
      }
      try {
        if (statSync(nested).isFile()) visit(nested);
      } catch {
        // Missing modules are reported by rustc during validation.
      }
    }
  }

  visit(target.src_path);
  return paths;
}

function isFastTest(pkg, target) {
  const haystack = `${packageDir(pkg)} ${target.name} ${relative(root, target.src_path)}`.toLowerCase();
  return !["e2e", "system", "live", "integration", "trybuild", "snapshot", "fixture", "slow"]
    .some((tag) => haystack.includes(tag));
}

function exactFastTestLabelForFile(file, pkg) {
  const normalized = file.replaceAll("\\", "/").replace(/^\.\//, "");
  const labels = [];
  for (const target of pkg.targets) {
    if (!target.kind.includes("test")) continue;
    if (!isFastTest(pkg, target)) continue;
    if (testSourcePaths(target, pkg).has(normalized)) {
      labels.push(`//${packageDir(pkg)}:${crateName(target.name)}_test`);
    }
  }
  return labels;
}

function buildLabels(pkg) {
  const dir = packageDir(pkg);
  const libOrMacro = pkg.targets.find((target) =>
    target.kind.includes("proc-macro") || target.kind.includes("lib")
  );
  const labels = [];
  for (const target of pkg.targets) {
    if (target.kind.includes("bench") || target.kind.includes("example") || target.kind.includes("test")) {
      continue;
    }
    if (target["required-features"]?.length) continue;
    if (target.kind.includes("proc-macro") || target.kind.includes("lib")) {
      labels.push(`//${dir}:${crateName(pkg.name)}`);
    } else if (target.kind.includes("bin")) {
      const name = target.name !== pkg.name || libOrMacro ? `${crateName(target.name)}_bin` : crateName(pkg.name);
      labels.push(`//${dir}:${name}`);
    }
  }
  return [...new Set(labels)].sort();
}

function testLabels(pkg) {
  const dir = packageDir(pkg);
  const labels = [];
  for (const target of pkg.targets) {
    if (!target.kind.includes("test")) continue;
    if (target["required-features"]?.length) continue;
    const source = readFileSync(target.src_path, "utf8");
    if (source.includes("trybuild::")) continue;
    labels.push(`//${dir}:${crateName(target.name)}_test`);
  }
  return [...new Set(labels)].sort();
}

function clippyLabels(pkg) {
  return [...new Set([...buildLabels(pkg), ...testLabels(pkg)])].sort();
}

function affectedClosure(seedIds) {
  const seen = new Set(seedIds);
  const queue = [...seedIds];
  for (let i = 0; i < queue.length; i += 1) {
    for (const dep of reverseDeps.get(queue[i]) ?? []) {
      if (!seen.has(dep)) {
        seen.add(dep);
        queue.push(dep);
      }
    }
  }
  return seen;
}

const files = changedFiles();
if (files.length === 0) {
  console.log(args.kind === "test" ? "//:fast_tests" : "//...");
  process.exit(0);
}

const seedIds = new Set();
const exactLabels = new Set();
const unmapped = [];
for (const file of files) {
  if (file.endsWith("BUILD.bazel") || file === "BUILD.bazel") continue;
  const pkg = packageForFile(file);
  if (pkg) {
    const exactLabel = (args.kind === "test" || args.kind === "clippy") && args.mode === "owned"
      ? exactFastTestLabelForFile(file, pkg)
      : null;
    if (exactLabel?.length) {
      for (const label of exactLabel) exactLabels.add(label);
    } else {
      seedIds.add(pkg.id);
    }
  } else {
    unmapped.push(file);
  }
}

if (unmapped.length || (seedIds.size === 0 && exactLabels.size === 0)) {
  console.log(args.kind === "test" ? "//:fast_tests" : "//...");
  process.exit(0);
}

const selectedIds = args.mode === "owned" ? seedIds : affectedClosure(seedIds);
const selectedPackages = [...selectedIds]
  .map((id) => byId.get(id))
  .filter(Boolean);
const labels = args.kind === "build"
  ? selectedPackages.flatMap(buildLabels).sort()
  : args.kind === "clippy"
  ? [
    ...exactLabels,
    ...selectedPackages.flatMap(clippyLabels),
  ].sort()
  : [
    ...exactLabels,
    ...selectedPackages
    .filter(hasFastSuite)
      .map((pkg) => `//${packageDir(pkg)}:fast_tests`),
  ].sort();

if (labels.length) {
  console.log(labels.join(" "));
} else if (args.emptyIfNoLabels) {
  console.log("");
} else {
  console.log(args.kind === "test" ? "//:fast_tests" : "//...");
}
