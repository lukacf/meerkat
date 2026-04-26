#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
}).trim();

const paths = process.argv.slice(2).map((path) =>
  path.replaceAll("\\", "/").replace(/^\.\//, ""),
);

if (paths.length === 0) process.exit(1);

const metadata = JSON.parse(
  execFileSync("./scripts/repo-cargo", ["metadata", "--format-version=1", "--no-deps"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  }),
);

const workspaceMembers = new Set(metadata.workspace_members);
const packages = metadata.packages.filter(
  (pkg) => pkg.source === null && workspaceMembers.has(pkg.id),
);
const packageDir = (pkg) => relative(root, dirname(pkg.manifest_path)).replaceAll("\\", "/") || ".";
const packageDirs = packages
  .map((pkg) => [packageDir(pkg), pkg])
  .sort((a, b) => b[0].length - a[0].length);

function packageForFile(file) {
  for (const [dir, pkg] of packageDirs) {
    if (dir === ".") continue;
    if (file === `${dir}/Cargo.toml` || file.startsWith(`${dir}/`)) return pkg;
  }
  return null;
}

function isFastTest(pkg, target) {
  const haystack = `${packageDir(pkg)} ${target.name} ${relative(root, target.src_path)}`.toLowerCase();
  return !["e2e", "system", "live", "integration", "trybuild", "snapshot", "fixture", "slow"].some(
    (tag) => haystack.includes(tag),
  );
}

function testSourcePaths(target, pkg) {
  const packageRoot = dirname(pkg.manifest_path);
  const seen = new Set();
  const sources = new Set();

  function visit(file) {
    if (seen.has(file) || !existsSync(file)) return;
    seen.add(file);
    if (!file.startsWith(`${packageRoot}/`)) return;
    const rel = relative(root, file).replaceAll("\\", "/");
    if (!rel || rel.startsWith("..")) return;
    sources.add(rel);

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
  return sources;
}

function exactTestsForFile(file, pkg) {
  const tests = [];
  for (const target of pkg.targets) {
    if (!target.kind.includes("test")) continue;
    if (target["required-features"]?.length) continue;
    if (!isFastTest(pkg, target)) continue;
    if (testSourcePaths(target, pkg).has(file)) tests.push(target.name);
  }
  return tests;
}

let selectedPackage = null;
const selectedTests = new Set();

for (const file of paths) {
  const pkg = packageForFile(file);
  if (!pkg) process.exit(1);
  if (selectedPackage && selectedPackage.name !== pkg.name) process.exit(1);
  selectedPackage = pkg;

  const tests = exactTestsForFile(file, pkg);
  if (tests.length === 0) process.exit(1);
  for (const test of tests) selectedTests.add(test);
}

if (!selectedPackage || selectedTests.size === 0) process.exit(1);

console.log(`package=${selectedPackage.name}`);
for (const test of [...selectedTests].sort()) {
  console.log(`test=${test}`);
}
