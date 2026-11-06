#!/usr/bin/env node
// Fail-closed release-projection classifier: does <head> differ from <base>
// only by the version projection scripts/release-hook.sh writes for a
// forward patch release (x.y.z to x.y.(z+1))?
//
// Exit status: 0 projection only, 1 not a projection (or not provably one),
// 2 classification error. One implementation serves the pre-push hooks
// (scripts/pre-push-unit.sh and pre-push-machines.sh) and
// CI (scripts/ci-cargo-lanes.mjs and scripts/ci-release-evidence.mjs, which
// the CI gate and the release workflow's require_ci_green run from the
// parent commit's tree), so the hook and CI accept exactly the same diffs.
//
// Every changed path must be a modification of a regular, non-executable
// file (mode 100644 on both sides) on a projection surface, every blob must
// be valid UTF-8, and every changed byte must be the projection:
//   - Cargo.toml: [workspace.package] version and the versions of
//     [workspace.dependencies] path entries;
//   - Cargo.lock and standalone example locks (examples/<name>/Cargo.lock,
//     refreshed by scripts/example-locks.py): versions of source-less (path)
//     packages only;
//   - the documents the docs stamper owns (Markdown and MDX under docs/
//     outside docs/meta and docs/internal, README.md, the platform skill):
//     exactly the output of the base's scripts/stamp-docs-contract-version.sh
//     for the new version, so only its marker lines change;
//   - CHANGELOG.md: exactly the output of the base's
//     scripts/stamp-changelog-release.py for the new version and the date in
//     the stamped heading;
//   - every other projection surface (SDK manifests and generated types,
//     schemas, BUILD.bazel, other docs, ContractVersion::CURRENT): every
//     occurrence of the old version becomes the new one;
//   - MODULE.bazel.lock is proven by its own freshness gates.

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// CHANGELOG.md alone is over 1.6 MiB; spawnSync's 1 MiB default buffer made
// `git show` fail on it.
const GIT_MAX_BUFFER = 1024 * 1024 * 1024;
const REGULAR_FILE_MODE = "100644";
const DOCS_STAMPER = "scripts/stamp-docs-contract-version.sh";
const CHANGELOG_STAMPER = "scripts/stamp-changelog-release.py";

export class ReleaseProjectionError extends Error {}

class NotProjection extends Error {}

// Strict UTF-8: an invalid sequence is an error, and a byte-order mark stays
// part of the text, so decoded texts compare exactly as their bytes do.
function strictUtf8(bytes) {
  try {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    return null;
  }
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

// Only a forward patch release reuses evidence: same major and minor, the
// next patch number, no prerelease on either side.
export function isForwardPatchBump(oldVersion, newVersion) {
  const oldParts = versionParts(oldVersion ?? "");
  const newParts = versionParts(newVersion ?? "");
  if (!oldParts || !newParts || oldParts.prerelease || newParts.prerelease) return false;
  return (
    oldParts.major === newParts.major &&
    oldParts.minor === newParts.minor &&
    BigInt(newParts.patch) === BigInt(oldParts.patch) + 1n &&
    newVersion === `${newParts.major}.${newParts.minor}.${BigInt(newParts.patch)}` &&
    oldVersion === `${oldParts.major}.${oldParts.minor}.${BigInt(oldParts.patch)}`
  );
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
        // Internal requirements publish exact (`=X.Y.Z`). The operator stays
        // outside the placeholder, so a bump that drops or adds it is not a
        // projection.
        return line.replace(
          new RegExp(`(\\bversion\\s*=\\s*\"=?)${escapeRegExp(version)}(\")`),
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

function isCargoLockPath(path) {
  return path === "Cargo.lock" || /^examples\/[^/]+\/Cargo\.lock$/.test(path);
}

// The files scripts/stamp-docs-contract-version.sh rewrites (its own walk):
// Markdown and MDX under docs/ outside the independently released Meta
// mirror and the unpublished docs/internal notes, the README and the
// platform skill. Some are compiled into binaries (the skill through the
// facade's help corpus), so only the stamper's own output is accepted.
export function isStampedDocument(path) {
  if (path === "README.md" || path === ".claude/skills/meerkat-platform/SKILL.md") return true;
  return (
    path.startsWith("docs/") &&
    !path.startsWith("docs/meta/") &&
    !path.startsWith("docs/internal/") &&
    /\.mdx?$/.test(path)
  );
}

function normalizeProjection(path, text, version, parts) {
  let normalized;
  if (path === "Cargo.toml") {
    normalized = normalizeCargoManifest(text, version);
  } else if (isCargoLockPath(path)) {
    normalized = normalizeCargoLock(text, version);
  } else {
    normalized = text.split(version).join("@@MEERKAT_RELEASE_VERSION@@");
  }
  normalized = normalizeStructuredContractVersion(normalized, parts);
  if (path === "crates/meerkat-contracts/src/version.rs") {
    normalized = normalizeContractSource(normalized, parts);
  }
  return normalized;
}

export function isProjectedPath(path) {
  return (
    path === "Cargo.toml" ||
    isCargoLockPath(path) ||
    path === "README.md" ||
    path === ".claude/skills/meerkat-platform/SKILL.md" ||
    path === "crates/meerkat-contracts/src/version.rs" ||
    path === "sdks/python/pyproject.toml" ||
    path === "sdks/typescript/package.json" ||
    path === "sdks/web/package.json" ||
    path === "sdks/web/src/runtime.ts" ||
    path.startsWith("sdks/web/src/generated/") ||
    path.startsWith("sdks/python/meerkat/generated/") ||
    path.startsWith("sdks/typescript/src/generated/") ||
    path.startsWith("artifacts/schemas/") ||
    (path.startsWith("docs/") && !path.startsWith("docs/meta/")) ||
    path.endsWith("/BUILD.bazel") ||
    path === "BUILD.bazel"
  );
}

function makeGit(cwd) {
  const bytes = (gitArgs, { allowFailure = false } = {}) => {
    const result = spawnSync("git", gitArgs, { cwd, maxBuffer: GIT_MAX_BUFFER });
    if (result.status !== 0) {
      if (allowFailure) return null;
      const detail = (result.stderr?.toString("utf8") ?? "").trim() || result.error?.message || "";
      throw new ReleaseProjectionError(`git ${gitArgs.join(" ")} failed${detail ? `: ${detail}` : ""}`);
    }
    return result.stdout;
  };
  const succeeds = (gitArgs) => spawnSync("git", gitArgs, { cwd, stdio: "ignore" }).status === 0;
  return { bytes, succeeds };
}

// Raw diff records: modes, blobs, status and path, rename detection off.
function rawChanges(git, base, head) {
  const output = git.bytes(["diff", "--raw", "-z", "--no-renames", "--no-abbrev", base, head, "--"]);
  const fields = [];
  let start = 0;
  for (let index = 0; index < output.length; index += 1) {
    if (output[index] === 0) {
      fields.push(output.subarray(start, index));
      start = index + 1;
    }
  }
  const changes = [];
  for (let index = 0; index + 1 < fields.length; index += 2) {
    const meta = strictUtf8(fields[index]);
    const path = strictUtf8(fields[index + 1]);
    const match = meta?.match(/^:([0-7]{6}) ([0-7]{6}) ([0-9a-f]{40,64}) ([0-9a-f]{40,64}) ([A-Z])[0-9]*$/);
    if (!match || path === null) throw new NotProjection("a raw diff record is not valid UTF-8 or not understood");
    changes.push({ oldMode: match[1], newMode: match[2], oldBlob: match[3], newBlob: match[4], status: match[5], path });
  }
  return changes;
}

function blobText(git, blob, label) {
  const text = strictUtf8(git.bytes(["cat-file", "blob", blob]));
  if (text === null) throw new NotProjection(`${label} is not valid UTF-8`);
  return text;
}

function revisionText(git, revision, path) {
  const raw = git.bytes(["show", `${revision}:${path}`], { allowFailure: true });
  if (raw === null) return null;
  const text = strictUtf8(raw);
  if (text === null) throw new NotProjection(`${path} at ${revision} is not valid UTF-8`);
  return text;
}

function workspaceVersion(git, revision) {
  const manifest = revisionText(git, revision, "Cargo.toml");
  if (manifest === null) return null;
  const section = manifest.match(/(?:^|\n)\[workspace\.package\]\s*\n([\s\S]*?)(?=\n\[|$)/);
  if (!section) return null;
  const version = section[1].match(/^version\s*=\s*"([^"]+)"\s*$/m);
  return version?.[1] ?? null;
}

// Run one of the base's own stampers over the base's copies of `files` in a
// scratch tree laid out like the repository, and return the stamped bytes.
function stampWithBase(git, base, { stamper, command, files, env = {} }) {
  const stamperBytes = git.bytes(["show", `${base}:${stamper}`], { allowFailure: true });
  if (stamperBytes === null) throw new NotProjection(`${stamper} is missing at the base`);
  const scratch = mkdtempSync(join(tmpdir(), "meerkat-release-projection-"));
  try {
    const place = (path, contents) => {
      mkdirSync(dirname(join(scratch, path)), { recursive: true });
      writeFileSync(join(scratch, path), contents);
    };
    place(stamper, stamperBytes);
    for (const [path, contents] of Object.entries(files)) place(path, contents);
    const result = spawnSync(command[0], command.slice(1).map((arg) => arg.replaceAll("@@ROOT@@", scratch)), {
      cwd: scratch,
      env: { ...process.env, ...env },
      encoding: "utf8",
      maxBuffer: GIT_MAX_BUFFER,
    });
    if (result.status !== 0) {
      throw new NotProjection(`${stamper} failed on the base files: ${(result.stderr || result.stdout || "").trim().split("\n").at(-1) ?? ""}`);
    }
    return Object.fromEntries(Object.keys(files).map((path) => [path, readFileSync(join(scratch, path))]));
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// Classify base..head. Returns { projection: true, oldVersion, newVersion,
// paths } for a release projection, { projection: false, reason } otherwise;
// throws ReleaseProjectionError when git cannot answer.
export function classifyReleaseProjection({ base, head, cwd = process.cwd() }) {
  if (!base || !head) throw new ReleaseProjectionError("base and head revisions are required");
  const git = makeGit(cwd);
  const notProjection = (reason) => {
    throw new NotProjection(reason);
  };
  try {
    if (!git.succeeds(["merge-base", "--is-ancestor", base, head])) {
      notProjection("base is not an ancestor of head");
    }

    const oldVersion = workspaceVersion(git, base);
    const newVersion = workspaceVersion(git, head);
    const oldParts = oldVersion ? versionParts(oldVersion) : null;
    const newParts = newVersion ? versionParts(newVersion) : null;
    if (!oldVersion || !newVersion || !oldParts || !newParts) {
      notProjection("workspace package version is missing or not semantic");
    }
    if (oldVersion === newVersion) {
      notProjection("workspace package version did not change");
    }
    if (!isForwardPatchBump(oldVersion, newVersion)) {
      notProjection(`${oldVersion} -> ${newVersion} is not a forward patch release`);
    }

    const changes = rawChanges(git, base, head);
    if (changes.length === 0) notProjection("no tracked files changed");
    if (!changes.some(({ path }) => path === "Cargo.toml")) {
      notProjection("Cargo.toml did not change");
    }
    if (!changes.some(({ path }) => path === "CHANGELOG.md")) {
      notProjection("CHANGELOG.md did not change");
    }

    const stampedDocuments = [];
    let changelog = null;
    for (const change of changes) {
      const { status, path, oldMode, newMode } = change;
      if (status !== "M") notProjection(`${path} has non-modification status ${status}`);
      if (oldMode !== REGULAR_FILE_MODE || newMode !== REGULAR_FILE_MODE) {
        notProjection(`${path} is mode ${oldMode} -> ${newMode}, not a regular file on both sides`);
      }
      const before = blobText(git, change.oldBlob, `${path} at the base`);
      const after = blobText(git, change.newBlob, `${path} at the head`);
      // MODULE.bazel.lock embeds content hashes for Cargo.toml, Cargo.lock, and
      // generated repository data. Its dedicated freshness gates prove the
      // whole lock against the candidate tree, so it is not normalized here.
      if (path === "MODULE.bazel.lock") continue;
      if (path === "CHANGELOG.md") {
        changelog = { before, after };
        continue;
      }
      if (!isProjectedPath(path)) notProjection(`${path} is not a release projection surface`);
      if (isStampedDocument(path)) {
        stampedDocuments.push({ path, before, after });
        continue;
      }
      if (
        normalizeProjection(path, before, oldVersion, oldParts) !==
        normalizeProjection(path, after, newVersion, newParts)
      ) {
        notProjection(`${path} contains changes beyond the version projection`);
      }
    }

    if (stampedDocuments.length > 0) {
      // The stamper walks README.md and the platform skill unconditionally,
      // so the scratch tree always carries the base's copies of both.
      const files = {};
      for (const always of ["README.md", ".claude/skills/meerkat-platform/SKILL.md"]) {
        const text = revisionText(git, base, always);
        if (text === null) notProjection(`${always} is missing at the base`);
        files[always] = text;
      }
      for (const { path, before } of stampedDocuments) files[path] = before;
      const stamped = stampWithBase(git, base, {
        stamper: DOCS_STAMPER,
        command: ["bash", `@@ROOT@@/${DOCS_STAMPER}`, newVersion],
        files,
      });
      for (const { path, after } of stampedDocuments) {
        if (strictUtf8(stamped[path]) !== after) {
          notProjection(`${path} is not the docs stamper's projection of the base file`);
        }
      }
    }

    const heading = changelog?.after.match(
      new RegExp(`^## \\[${escapeRegExp(newVersion)}\\] - (\\d{4}-\\d{2}-\\d{2})$`, "m"),
    );
    if (!heading) notProjection(`CHANGELOG.md does not contain a stamped ${newVersion} release heading`);
    const stampedChangelog = stampWithBase(git, base, {
      stamper: CHANGELOG_STAMPER,
      command: ["python3", `@@ROOT@@/${CHANGELOG_STAMPER}`, "@@ROOT@@/CHANGELOG.md", newVersion],
      files: { "CHANGELOG.md": changelog.before },
      env: { MEERKAT_RELEASE_DATE: heading[1] },
    });
    if (strictUtf8(stampedChangelog["CHANGELOG.md"]) !== changelog.after) {
      notProjection("CHANGELOG.md is not the changelog stamper's projection of the base file");
    }
    return { projection: true, oldVersion, newVersion, paths: changes.map(({ path }) => path) };
  } catch (error) {
    if (error instanceof NotProjection) return { projection: false, reason: error.message };
    throw error;
  }
}

function main(argv) {
  let base = "";
  let head = "";
  let verbose = false;
  for (let i = 0; i < argv.length; i += 1) {
    switch (argv[i]) {
      case "--base":
        base = argv[++i] ?? "";
        break;
      case "--head":
        head = argv[++i] ?? "";
        break;
      case "--verbose":
        verbose = true;
        break;
      case "-h":
      case "--help":
        console.log("usage: release-projection-only.mjs --base <rev> --head <rev> [--verbose]");
        return 0;
      default:
        console.error(`unknown argument: ${argv[i]}`);
        return 2;
    }
  }
  if (!base || !head) {
    console.error("error: --base and --head are required");
    return 2;
  }
  let result;
  try {
    result = classifyReleaseProjection({ base, head });
  } catch (error) {
    console.error(error?.message ?? String(error));
    return 2;
  }
  if (!result.projection) {
    if (verbose) console.error(`not a release projection: ${result.reason}`);
    return 1;
  }
  if (verbose) console.log(`release projection only: ${result.oldVersion} -> ${result.newVersion}`);
  return 0;
}

const invokedPath = process.argv[1] ? realpathSync(process.argv[1]) : "";
if (invokedPath === realpathSync(fileURLToPath(import.meta.url))) {
  process.exit(main(process.argv.slice(2)));
}
