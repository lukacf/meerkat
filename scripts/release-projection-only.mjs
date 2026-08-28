#!/usr/bin/env node

import { spawnSync } from "node:child_process";

const args = process.argv.slice(2);
let base = "";
let head = "";
let verbose = false;

for (let i = 0; i < args.length; i += 1) {
  switch (args[i]) {
    case "--base":
      base = args[++i] ?? "";
      break;
    case "--head":
      head = args[++i] ?? "";
      break;
    case "--verbose":
      verbose = true;
      break;
    case "-h":
    case "--help":
      console.log("usage: release-projection-only.mjs --base <rev> --head <rev> [--verbose]");
      process.exit(0);
    default:
      console.error(`unknown argument: ${args[i]}`);
      process.exit(2);
  }
}

if (!base || !head) {
  console.error("error: --base and --head are required");
  process.exit(2);
}

function gitText(gitArgs, { allowFailure = false } = {}) {
  const result = spawnSync("git", gitArgs, { encoding: "utf8" });
  if (result.status !== 0) {
    if (allowFailure) return null;
    const detail = result.stderr.trim();
    console.error(`git ${gitArgs.join(" ")} failed${detail ? `: ${detail}` : ""}`);
    process.exit(2);
  }
  return result.stdout;
}

function notProjection(reason) {
  if (verbose) console.error(`not a release projection: ${reason}`);
  process.exit(1);
}

function workspaceVersion(revision) {
  const manifest = gitText(["show", `${revision}:Cargo.toml`], { allowFailure: true });
  if (manifest === null) return null;
  const section = manifest.match(/(?:^|\n)\[workspace\.package\]\s*\n([\s\S]*?)(?=\n\[|$)/);
  if (!section) return null;
  const version = section[1].match(/^version\s*=\s*"([^"]+)"\s*$/m);
  return version?.[1] ?? null;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function versionParts(version) {
  const match = version.match(/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/);
  if (!match) return null;
  return {
    major: match[1],
    minor: match[2],
    patch: match[3],
    prerelease: match[4] ?? "",
  };
}

function normalizeStructuredContractVersion(text, parts) {
  const pattern = new RegExp(
    `(\\"contract_version\\"\\s*:\\s*\\{\\s*\\"major\\"\\s*:\\s*)${escapeRegExp(parts.major)}` +
      `(\\s*,\\s*\\"minor\\"\\s*:\\s*)${escapeRegExp(parts.minor)}` +
      `(\\s*,\\s*\\"patch\\"\\s*:\\s*)${escapeRegExp(parts.patch)}`,
    "g",
  );
  return text.replace(pattern, "$1@@MAJOR@@$2@@MINOR@@$3@@PATCH@@");
}

function normalizeContractSource(text, parts) {
  const current = /(pub const CURRENT:[\s\S]*?=\s*Self\s*\{)([\s\S]*?)(\n\s*\};)/;
  text = text.replace(current, (_all, prefix, body, suffix) => {
    const normalized = body
      .replace(new RegExp(`(major:\\s*)${escapeRegExp(parts.major)}\\b`), "$1@@MAJOR@@")
      .replace(new RegExp(`(minor:\\s*)${escapeRegExp(parts.minor)}\\b`), "$1@@MINOR@@")
      .replace(new RegExp(`(patch:\\s*)${escapeRegExp(parts.patch)}\\b`), "$1@@PATCH@@");
    return `${prefix}${normalized}${suffix}`;
  });
  const prerelease = parts.prerelease
    ? `Some("${parts.prerelease}")`
    : "None";
  return text.replace(
    new RegExp(`(pub const PRERELEASE: Option<&'static str> = )${escapeRegExp(prerelease)};`),
    "$1@@PRERELEASE@@;",
  );
}

function normalizeCargoManifest(text, version) {
  let section = "";
  return text
    .split("\n")
    .map((line) => {
      const header = line.match(/^\s*\[([^\]]+)\]\s*$/);
      if (header) section = header[1];
      if (section === "workspace.package") {
        return line.replace(
          new RegExp(`^(\\s*version\\s*=\\s*\")${escapeRegExp(version)}(\"\\s*)$`),
          "$1@@MEERKAT_RELEASE_VERSION@@$2",
        );
      }
      if (section === "workspace.dependencies" && line.includes("path =")) {
        return line.replace(
          new RegExp(`(\\bversion\\s*=\\s*\")${escapeRegExp(version)}(\")`),
          "$1@@MEERKAT_RELEASE_VERSION@@$2",
        );
      }
      return line;
    })
    .join("\n");
}

function normalizeCargoLock(text, version) {
  return text
    .split(/(?=\[\[package\]\]\n)/)
    .map((block) => {
      if (!block.startsWith("[[package]]\n") || /^source\s*=/m.test(block)) {
        return block;
      }
      return block.replace(
        new RegExp(`^(version\\s*=\\s*\")${escapeRegExp(version)}(\"\\s*)$`, "m"),
        "$1@@MEERKAT_RELEASE_VERSION@@$2",
      );
    })
    .join("");
}

function normalizeProjection(path, text, version, parts) {
  let normalized;
  if (path === "Cargo.toml") {
    normalized = normalizeCargoManifest(text, version);
  } else if (path === "Cargo.lock") {
    normalized = normalizeCargoLock(text, version);
  } else {
    normalized = text.split(version).join("@@MEERKAT_RELEASE_VERSION@@");
  }
  normalized = normalizeStructuredContractVersion(normalized, parts);
  if (path === "meerkat-contracts/src/version.rs") {
    normalized = normalizeContractSource(normalized, parts);
  }
  return normalized;
}

function isProjectedPath(path) {
  return (
    path === "Cargo.toml" ||
    path === "Cargo.lock" ||
    path === "README.md" ||
    path === ".claude/skills/meerkat-platform/SKILL.md" ||
    path === "meerkat-contracts/src/version.rs" ||
    path === "sdks/python/pyproject.toml" ||
    path === "sdks/typescript/package.json" ||
    path === "sdks/web/package.json" ||
    path === "sdks/web/src/runtime.ts" ||
    path.startsWith("sdks/web/src/generated/") ||
    path.startsWith("sdks/python/meerkat/generated/") ||
    path.startsWith("sdks/typescript/src/generated/") ||
    path.startsWith("artifacts/schemas/") ||
    (path.startsWith("docs/") && !path.startsWith("docs/mobkit/")) ||
    path.endsWith("/BUILD.bazel") ||
    path === "BUILD.bazel"
  );
}

if (
  spawnSync("git", ["merge-base", "--is-ancestor", base, head], {
    stdio: "ignore",
  }).status !== 0
) {
  notProjection("base is not an ancestor of head");
}

const oldVersion = workspaceVersion(base);
const newVersion = workspaceVersion(head);
const oldParts = oldVersion ? versionParts(oldVersion) : null;
const newParts = newVersion ? versionParts(newVersion) : null;
if (!oldVersion || !newVersion || !oldParts || !newParts) {
  notProjection("workspace package version is missing or not semantic");
}
if (oldVersion === newVersion) {
  notProjection("workspace package version did not change");
}

const statusText = gitText(["diff", "--name-status", "--diff-filter=ACDMRT", base, head, "--"]);
const changes = statusText
  .trim()
  .split("\n")
  .filter(Boolean)
  .map((line) => {
    const fields = line.split("\t");
    return { status: fields[0], path: fields.at(-1) };
  });

if (changes.length === 0) notProjection("no tracked files changed");
if (!changes.some(({ path }) => path === "Cargo.toml")) {
  notProjection("Cargo.toml did not change");
}
if (!changes.some(({ path }) => path === "CHANGELOG.md")) {
  notProjection("CHANGELOG.md did not change");
}

for (const { status, path } of changes) {
  if (status !== "M") notProjection(`${path} has non-modification status ${status}`);
  if (path === "CHANGELOG.md") continue;
  // MODULE.bazel.lock embeds content hashes for Cargo.toml, Cargo.lock, and
  // generated repository data. Its dedicated freshness gate proves the whole
  // lock against the candidate tree, so it is intentionally not normalized
  // here.
  if (path === "MODULE.bazel.lock") continue;
  if (!isProjectedPath(path)) notProjection(`${path} is not a release projection surface`);

  const before = gitText(["show", `${base}:${path}`], { allowFailure: true });
  const after = gitText(["show", `${head}:${path}`], { allowFailure: true });
  if (before === null || after === null) notProjection(`${path} is not present at both revisions`);
  const normalizedBefore = normalizeProjection(path, before, oldVersion, oldParts);
  const normalizedAfter = normalizeProjection(path, after, newVersion, newParts);
  if (normalizedBefore !== normalizedAfter) {
    notProjection(`${path} contains changes beyond the version projection`);
  }
}

const changelog = gitText(["show", `${head}:CHANGELOG.md`]);
if (!changelog.includes(`## [${newVersion}]`)) {
  notProjection(`CHANGELOG.md does not contain a ${newVersion} release heading`);
}

if (verbose) {
  console.log(`release projection only: ${oldVersion} -> ${newVersion}`);
}
