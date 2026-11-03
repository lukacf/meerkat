#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { loadProducts, productRootPrefixes } from "./ci-products.mjs";
import {
  crateName,
  isFastTest,
  packageDir,
  readMetadata,
  root,
  workspacePackages,
} from "./rust-test-selector.mjs";

const metadata = readMetadata();
// The Bazel graph builds Core only: no Bazel target builds a product's crates
// (ci/products.json), so their tests need no Bazel twin.
const productRoots = productRootPrefixes(loadProducts());
const isProductPackage = (pkg) => productRoots.some((prefix) => `${packageDir(pkg)}/`.startsWith(prefix));
const rootBuild = readFileSync(resolve(root, "BUILD.bazel"), "utf8");
const missing = [];
const expected = [];
const expectedUnit = [];

function hasTarget(pkg, name) {
  const buildPath = resolve(root, packageDir(pkg), "BUILD.bazel");
  if (!existsSync(buildPath)) return false;
  return readFileSync(buildPath, "utf8").includes(`name = "${name}"`);
}

function rootSuiteIncludes(label) {
  return rootBuild.includes(JSON.stringify(label));
}

for (const pkg of workspacePackages(metadata).filter((candidate) => !isProductPackage(candidate))) {
  for (const target of pkg.targets) {
    if (target.kind.includes("lib") && !target.kind.includes("proc-macro")) {
      const targetName = `${crateName(target.name)}_unit_test`;
      const label = `//${packageDir(pkg)}:${targetName}`;
      expectedUnit.push(label);

      if (!hasTarget(pkg, targetName)) {
        missing.push(`${label} missing generated unit target`);
      } else if (!rootSuiteIncludes(label)) {
        missing.push(`${label} missing from //:fast_tests`);
      }
    }

    if (!target.kind.includes("test")) continue;
    if (!isFastTest(pkg, target)) continue;

    const targetName = `${crateName(target.name)}_test`;
    const label = `//${packageDir(pkg)}:${targetName}`;
    expected.push(label);

    if (!hasTarget(pkg, targetName)) {
      missing.push(`${label} missing generated target`);
    } else if (!rootSuiteIncludes(label)) {
      missing.push(`${label} missing from //:fast_tests`);
    }
  }
}

if (missing.length) {
  console.error("BuildBuddy fast parity failed:");
  for (const item of missing) console.error(`  ${item}`);
  process.exit(1);
}

console.log(
  `BuildBuddy fast parity ok: ${expected.length} fast Cargo test target(s) and ${expectedUnit.length} lib unit target(s) mapped`,
);
