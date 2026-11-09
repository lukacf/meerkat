import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readlinkSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, relative, resolve } from "node:path";

export const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
  encoding: "utf8",
}).trim();

export function normalizePath(path) {
  return path.replaceAll("\\", "/").replace(/^\.\//, "");
}

export function gitLines(args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" })
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function cargoManifestFiles() {
  return gitLines(["ls-files", "Cargo.lock", ":(glob)**/Cargo.toml"]);
}

function cargoAutodiscoveryFiles() {
  return gitLines([
    "ls-files",
    ":(glob)**/tests/*.rs",
    ":(glob)**/benches/*.rs",
    ":(glob)**/examples/*.rs",
    ":(glob)**/src/bin/*.rs",
  ]);
}

function metadataFingerprint() {
  const hash = createHash("sha256");
  hash.update(root);
  for (const file of [...cargoManifestFiles(), ...cargoAutodiscoveryFiles()].sort()) {
    const path = resolve(root, file);
    hash.update("\0");
    hash.update(file);
    hash.update("\0");
    if (!existsSync(path)) {
      hash.update("deleted");
      continue;
    }
    const stat = statSync(path);
    hash.update(String(stat.size));
    hash.update("\0");
    hash.update(String(stat.mtimeMs));
  }
  return hash.digest("hex");
}

export function readMetadata() {
  const cacheRoot = resolve(
    process.env.XDG_CACHE_HOME || resolve(process.env.HOME || root, ".cache"),
    "meerkat",
    "rust-selector-metadata",
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

// The features each selected workspace member is compiled with by one cargo
// invocation over `selectionArgs` (`--workspace --exclude NAME ...` or
// `-p NAME ...`): Cargo resolves features per invocation, across the
// selected members and their dependencies (dev-dependencies included, as a
// test build has them), so the same member can get different features under
// two selections. Read from `cargo tree`'s roots, which are the selected
// members themselves (on the combined workspace, `cargo tree --workspace`'s
// roots matched cargo metadata's resolve for all 62 members). Cached per
// metadata fingerprint and selection.
export function readSelectionFeatures(selectionArgs) {
  const args = [...selectionArgs];
  const cacheRoot = resolve(
    process.env.XDG_CACHE_HOME || resolve(process.env.HOME || root, ".cache"),
    "meerkat",
    "rust-selector-features",
  );
  const key = createHash("sha256").update(metadataFingerprint()).update("\0").update(args.join("\0")).digest("hex");
  const cachePath = resolve(cacheRoot, `${key}.json`);
  try {
    const cached = JSON.parse(readFileSync(cachePath, "utf8"));
    return new Map(cached.map(([name, features]) => [name, new Set(features)]));
  } catch {
    // Compute below.
  }
  const output = execFileSync(
    "./scripts/repo-cargo",
    ["tree", ...args, "-e", "normal,build,dev", "--prefix", "depth", "--format", "{p}|{f}"],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] },
  );
  const features = new Map();
  for (const line of output.split("\n")) {
    if (!line.startsWith("0")) continue;
    const bar = line.lastIndexOf("|");
    if (bar < 0) throw new Error(`cargo tree printed a root without features: ${line}`);
    const name = line.slice(1).split(" ", 1)[0];
    const list = line.slice(bar + 1).replace(/ \(\*\)$/, "");
    features.set(name, new Set(list.split(",").filter(Boolean)));
  }
  if (features.size === 0) throw new Error(`cargo tree ${args.join(" ")} printed no root`);
  mkdirSync(cacheRoot, { recursive: true });
  const tmpPath = resolve(cacheRoot, `${key}.${process.pid}.tmp`);
  writeFileSync(tmpPath, JSON.stringify([...features].map(([name, set]) => [name, [...set].sort()])));
  renameSync(tmpPath, cachePath);
  return features;
}

export function workspacePackages(metadata) {
  const workspaceMembers = new Set(metadata.workspace_members);
  return metadata.packages.filter(
    (pkg) => pkg.source === null && workspaceMembers.has(pkg.id),
  );
}

export function packageDir(pkg) {
  return normalizePath(relative(root, dirname(pkg.manifest_path))) || ".";
}

export function packageDirs(packages) {
  return packages
    .map((pkg) => [packageDir(pkg), pkg])
    .sort((a, b) => b[0].length - a[0].length);
}

function crateDirPackageForPath(normalized, dirs) {
  for (const [dir, pkg] of dirs) {
    if (dir === ".") continue;
    if (normalized === `${dir}/Cargo.toml` || normalized.startsWith(`${dir}/`)) {
      return pkg;
    }
  }
  return null;
}

const linkInputCache = new WeakMap();

// A crate can embed repo-owned documents through tracked symlinks, for example
// `meerkat/embedded_skills/**` -> `.claude/skills/**` behind `include_str!`.
// Git records the link, not its target, so an edit to the target never shows
// up under the crate directory and a directory-prefix match alone classifies
// it as docs-only. Resolve every tracked symlink inside a crate directory and
// treat its target as an input of that crate. The mapping is derived from the
// symlinks themselves so new embedded documents need no selector change.
// The same holds for a file of another crate that a crate links into its own
// directory, such as a test fixture that two crates' tests share: the file
// lies in its own crate's directory, but the linking crate reads it too, so
// a change to it selects both crates.
// A license file name: LICENSE, LICENSE-MIT, LICENSE-APACHE, LICENSE.md, ...
const LICENSE_LINK_RE = /^LICENSE(?:[-.][A-Za-z0-9.-]+)?$/;

// Every input a tracked symlink inside a crate directory makes:
// `{ path, pkg, owner, isDirectory }`, where `pkg` is the crate holding the
// link and `owner` the crate whose directory holds the target (null outside
// every crate), in target path order. A target outside the repository, a
// link to its own crate's file and a per-crate license link make no input.
export function linkInputs(dirs) {
  let inputs = linkInputCache.get(dirs);
  if (inputs) return inputs;
  inputs = [];
  const crateDirs = dirs.map(([dir]) => dir).filter((dir) => dir !== ".");
  const entries = crateDirs.length ? gitLines(["ls-files", "-s", "--", ...crateDirs]) : [];
  for (const entry of entries) {
    const match = entry.match(/^120000 \S+ \d+\t(.+)$/);
    if (!match) continue;
    const link = normalizePath(match[1]);
    const pkg = crateDirPackageForPath(link, dirs);
    if (!pkg) continue;
    let target;
    try {
      target = readlinkSync(resolve(root, link));
    } catch {
      continue;
    }
    const path = normalizePath(relative(root, resolve(root, dirname(link), target)));
    if (!path || path === ".." || path.startsWith("../") || path.startsWith("/")) continue;
    // Per-crate license links (crates/*/LICENSE-MIT -> ../../LICENSE-MIT,
    // #1766) are package metadata that `cargo package` copies; no crate
    // compiles them, so they are not embedded inputs of any one package (the
    // first crate in path order used to claim the repo-root license files).
    if (LICENSE_LINK_RE.test(basename(link)) && !path.includes("/") && LICENSE_LINK_RE.test(path)) continue;
    const owner = crateDirPackageForPath(path, dirs);
    if (owner === pkg) continue;
    let isDirectory = false;
    try {
      isDirectory = statSync(resolve(root, path)).isDirectory();
    } catch {
      // A dangling link still names an exact input path.
    }
    inputs.push({ path, pkg, owner, isDirectory });
  }
  inputs.sort((a, b) => a.path.localeCompare(b.path));
  linkInputCache.set(dirs, inputs);
  return inputs;
}

// The link inputs outside every crate directory: the repo-owned documents a
// crate embeds. scripts/rust-embedded-inputs.mjs prints these for the agent
// gates, which map a path inside a crate directory to that crate.
export function embeddedInputs(dirs) {
  return linkInputs(dirs).filter((input) => input.owner === null);
}

// Every package a change to `file` selects: the crate whose directory holds
// it, then, in name order, each crate that reads it through a tracked symlink
// (an embedded document, or a file of another crate linked into its own
// directory). `inputs` defaults to the repository's own link inputs.
export function packagesForFile(file, dirs, inputs = linkInputs(dirs)) {
  const normalized = normalizePath(file);
  const owner = crateDirPackageForPath(normalized, dirs);
  const readers = new Map();
  for (const input of inputs) {
    if (input.pkg === owner) continue;
    if (normalized === input.path || (input.isDirectory && normalized.startsWith(`${input.path}/`))) {
      readers.set(input.pkg.name, input.pkg);
    }
  }
  const linked = [...readers.keys()].sort().map((name) => readers.get(name));
  return owner ? [owner, ...linked] : linked;
}

// One package for `file`: the crate whose directory holds it, else the first
// crate that embeds it from outside every crate directory.
export function packageForFile(file, dirs) {
  const normalized = normalizePath(file);
  const owner = crateDirPackageForPath(normalized, dirs);
  if (owner) return owner;
  for (const input of embeddedInputs(dirs)) {
    if (normalized === input.path || (input.isDirectory && normalized.startsWith(`${input.path}/`))) {
      return input.pkg;
    }
  }
  return null;
}

export function crateName(name) {
  return name.replaceAll("-", "_");
}

export function isFastTest(pkg, target) {
  if (target["required-features"]?.length) return false;
  const haystack = [
    packageDir(pkg),
    target.name,
    normalizePath(relative(root, target.src_path)),
    ...(target["required-features"] ?? []),
  ].join(" ").toLowerCase();
  return !["e2e", "system", "live", "integration", "trybuild", "snapshot", "fixture", "slow"].some(
    (tag) => haystack.includes(tag),
  );
}

export function moduleReferences(source) {
  const refs = [];
  let pendingPath = null;
  for (const line of source.split(/\r?\n/)) {
    // Only recognize attributes and module declarations at the start of a
    // Rust item. Documentation and comments may quote examples such as
    // `#[path = "..."]`; treating those as live attributes can bind the
    // quoted path to a later, real `mod` declaration.
    const pathMatch = line.match(/^[ \t]*#[ \t]*\[[ \t]*path[ \t]*=[ \t]*"([^"]+)"[ \t]*\]/);
    if (pathMatch) pendingPath = pathMatch[1];

    const itemText = pathMatch ? line.slice(pathMatch[0].length) : line;
    const modMatch = itemText.match(/^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;/);
    const modName = modMatch?.[1];
    if (!modName) continue;

    if (pendingPath) {
      refs.push({ path: pendingPath });
      pendingPath = null;
    } else {
      refs.push({ name: modName });
    }
  }
  return refs;
}

export function testSourcePaths(target, pkg) {
  const seen = new Set();
  const paths = new Set();

  function visit(file) {
    if (seen.has(file) || !existsSync(file)) return;
    seen.add(file);
    const rel = normalizePath(relative(root, file));
    if (!rel || rel === ".." || rel.startsWith("../")) return;
    paths.add(rel);

    const source = readFileSync(file, "utf8");
    for (const ref of moduleReferences(source)) {
      if (ref.path) {
        visit(resolve(dirname(file), ref.path));
        continue;
      }

      const flat = resolve(dirname(file), `${ref.name}.rs`);
      const nested = resolve(dirname(file), ref.name, "mod.rs");
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
