#!/usr/bin/env node
// Release-version pin ratchet for a release projection.
//
// A release-projection plan reuses the parent's test results, which ran
// with the old version compiled in. A test, snapshot, fixture, SDK source or
// Rust code line that spells out the release's old or new version (instead of
// reading CARGO_PKG_VERSION or a generated constant) can pass on the parent
// and fail after the bump, and no reused result would show it. This ratchet
// fails on any such literal outside the files the release hook projects, so
// the guard costs one `git grep` instead of the SDK and example suites.
//
// Scanned at --revision (default HEAD), as whole tokens (not inside a longer
// number or a prerelease):
//   - Rust sources (*.rs): every line that is not a comment;
//   - sdks/ and examples/: every file;
//   - anywhere: tests/, test/, __tests__/, snapshots/, __snapshots__/ and
//     fixtures/ trees, *.test.* and *.spec.* files, and *.snap files.
// Never scanned: the projection surfaces themselves
// (scripts/release-projection-only.mjs), CHANGELOG.md, MODULE.bazel.lock,
// Markdown and MDX prose, scripts/ (its release-tooling tests run in every
// plan's format + governance lane), .claude/ and .github/, and the roots of
// the repository's other products (ci/products.json at --revision: meta/,
// connect/), which release on version lines of their own and may spell
// their own or Core's versions.
//
// usage: release-version-pins.mjs --from X.Y.Z --to X.Y.Z [--revision REV]
// Exit 0 when clean, 1 on any pin, 2 on an error.

import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { isProjectedPath } from "./release-projection-only.mjs";

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function versionToken(version) {
  return new RegExp(`(?<![0-9.])${escapeRegExp(version)}(?![0-9]|\\.[0-9]|[-+][0-9A-Za-z])`);
}

// "rust" (non-comment lines only), "file" (every line) or null (not scanned).
// `productRoots` are the other products' root directories.
export function pinScope(path, productRoots = []) {
  if (productRoots.some((productRoot) => path === productRoot || path.startsWith(`${productRoot}/`))) return null;
  if (isProjectedPath(path) || path === "CHANGELOG.md" || path === "MODULE.bazel.lock") return null;
  if (/^(scripts|\.claude|\.github)\//.test(path) || /\.mdx?$/.test(path)) return null;
  if (path.endsWith(".rs")) return "rust";
  if (/^(sdks|examples)\//.test(path)) return "file";
  if (
    /(^|\/)(tests?|__tests__|snapshots|__snapshots__|fixtures)\//.test(path) ||
    /\.(test|spec)\.[^/]+$/.test(path) ||
    /\.snap$/.test(path)
  ) {
    return "file";
  }
  return null;
}

function isRustComment(line) {
  return /^\s*(\/\/|\/\*|\*)/.test(line);
}

// The root directories of the products other than Core at `revision`
// (ci/products.json); none when the revision has no registry.
export function productRootsAt(revision, cwd) {
  const shown = spawnSync("git", ["show", `${revision}:ci/products.json`], { cwd, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  if (shown.status !== 0) return [];
  const products = JSON.parse(shown.stdout).products;
  if (!Array.isArray(products)) throw new Error(`ci/products.json at ${revision} lists no products`);
  return products.map((product) => product.root).filter((root) => typeof root === "string" && root !== "");
}

// Every pinned line at `revision`: { path, line, text, version }.
export function findReleaseVersionPins({ from, to, revision = "HEAD", cwd = process.cwd() }) {
  const productRoots = productRootsAt(revision, cwd);
  const result = spawnSync(
    "git",
    ["grep", "-z", "-n", "-I", "--full-name", "-F", "-e", from, "-e", to, revision, "--"],
    { cwd, encoding: "utf8", maxBuffer: 1024 * 1024 * 1024 },
  );
  // git grep exits 1 when nothing matches.
  if (result.status === 1 && !result.stderr) return [];
  if (result.status !== 0) throw new Error(`git grep failed: ${(result.stderr ?? "").trim() || `status ${result.status}`}`);
  const tokens = [from, to].map((version) => [version, versionToken(version)]);
  const pins = [];
  for (const record of result.stdout.split("\n")) {
    if (!record) continue;
    const [location, lineNumber, ...rest] = record.split("\0");
    const text = rest.join("\0");
    const path = location.startsWith(`${revision}:`) ? location.slice(revision.length + 1) : location;
    const scope = pinScope(path, productRoots);
    if (scope === null || (scope === "rust" && isRustComment(text))) continue;
    for (const [version, token] of tokens) {
      if (token.test(text)) pins.push({ path, line: Number(lineNumber), text: text.trim(), version });
    }
  }
  return pins;
}

function main(argv) {
  let from = "";
  let to = "";
  let revision = "HEAD";
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--from") from = argv[++i] ?? "";
    else if (argv[i] === "--to") to = argv[++i] ?? "";
    else if (argv[i] === "--revision") revision = argv[++i] ?? "";
    else {
      process.stderr.write(`unknown argument: ${argv[i]}\n`);
      return 2;
    }
  }
  const semver = /^\d+\.\d+\.\d+$/;
  if (!semver.test(from) || !semver.test(to) || !revision) {
    process.stderr.write("usage: release-version-pins.mjs --from X.Y.Z --to X.Y.Z [--revision REV]\n");
    return 2;
  }
  let pins;
  try {
    pins = findReleaseVersionPins({ from, to, revision });
  } catch (error) {
    process.stderr.write(`release-version-pins: ${error.message}\n`);
    return 2;
  }
  if (pins.length > 0) {
    process.stderr.write(
      `release-version-pins: ${pins.length} line(s) outside the release projection pin ${from} or ${to}; ` +
        "read the version from CARGO_PKG_VERSION or a generated constant instead:\n",
    );
    for (const pin of pins) process.stderr.write(`  ${pin.path}:${pin.line}: ${pin.text}\n`);
    return 1;
  }
  process.stdout.write(`release-version-pins: no test, snapshot, fixture, SDK or Rust code line pins ${from} or ${to}\n`);
  return 0;
}

const invokedPath = process.argv[1] ? realpathSync(process.argv[1]) : "";
if (invokedPath === realpathSync(fileURLToPath(import.meta.url))) {
  process.exit(main(process.argv.slice(2)));
}
