import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

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

export function packageForFile(file, dirs) {
  const normalized = normalizePath(file);
  for (const [dir, pkg] of dirs) {
    if (dir === ".") continue;
    if (normalized === `${dir}/Cargo.toml` || normalized.startsWith(`${dir}/`)) {
      return pkg;
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
    const pathMatch = line.match(/#[ \t]*\[[ \t]*path[ \t]*=[ \t]*"([^"]+)"[ \t]*\]/);
    if (pathMatch) pendingPath = pathMatch[1];

    const modMatch = line.match(/(?:^|\s)(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;/);
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
  const packageRoot = dirname(pkg.manifest_path);
  const seen = new Set();
  const paths = new Set();

  function visit(file) {
    if (seen.has(file) || !existsSync(file)) return;
    seen.add(file);
    if (!file.startsWith(`${packageRoot}/`)) return;
    const rel = normalizePath(relative(root, file));
    if (!rel || rel.startsWith("..")) return;
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
