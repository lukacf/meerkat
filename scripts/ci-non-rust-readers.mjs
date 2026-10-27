// Non-Rust inputs of Rust tests: the inventory model and the pure mapping
// from a changed path list to the reader test selection.
//
// The CI planner (scripts/ci-cargo-lanes.mjs) selects a package's lanes from
// the paths that changed inside its crate directory. Some Rust code reads
// repository files outside its own crate: the rkat doc-example test walks
// docs/, xtask's static lane tests read the SDK manifests and the workflows,
// the facade embeds .claude/skills, and so on. A change to such a file alone
// sets rust_changed=false and selects no Rust lane, so before this inventory
// only the pre-push hook ran those tests.
//
// scripts/ci-non-rust-readers.json lists every such input, keyed by an exact
// repository path or by a directory prefix ending in "/", with the package,
// target and (optionally) the test names that read it.
// scripts/ci-non-rust-readers-guard.mjs keeps it complete: it scans every
// workspace Rust source and fails when a reference is missing from the
// inventory or an inventory row is no longer read.
//
// This module is pure apart from loadNonRustReaders(): no git, no cargo, and
// nothing runs at import, so the planner can import it at no cost.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const INVENTORY_PATH = "scripts/ci-non-rust-readers.json";
// The inventory, its model and its guard define the reader selection; a
// change to any of them selects every reader they name.
export const READER_DEFINITION_PATHS = [
  INVENTORY_PATH,
  "scripts/ci-non-rust-readers.mjs",
  "scripts/ci-non-rust-readers-guard.mjs",
];

// The key of a reader that walks the whole checkout (a live-workspace audit,
// a command run in the repository root): every changed path selects it,
// except paths under the roots of the products other than Core
// (ci/products.json), which those audits leave to their products.
export const WHOLE_CHECKOUT = "./";

const TARGET_RE = /^(lib|build|(bin|test|example|bench):[A-Za-z0-9_.-]+)$/;
const PATH_RE = /^[^/.][^\0]*$|^\.[A-Za-z0-9_-][^\0]*$/;

function normalize(path) {
  return String(path).replaceAll("\\", "/").replace(/^\.\//, "");
}

// Validate the parsed JSON document and return the inventory model:
//   { entries: [{ path, package, target, scope, tests, features, sources }],
//     notReaders: [{ source, literal, why }] }
// One entry per (path, reader). Throws on any malformed row, so a planner
// that imports the inventory fails closed instead of selecting nothing.
export function parseNonRustReaders(document) {
  if (document === null || typeof document !== "object") throw new Error("inventory: not a JSON object");
  if (document.schema_version !== 1) throw new Error(`inventory: unsupported schema_version ${document.schema_version}`);
  const paths = document.paths;
  if (paths === null || typeof paths !== "object" || Array.isArray(paths)) {
    throw new Error("inventory: `paths` must map repository paths to reader lists");
  }
  const entries = [];
  const seen = new Set();
  const keys = Object.keys(paths);
  const sorted = [...keys].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  if (keys.join("\n") !== sorted.join("\n")) throw new Error("inventory: `paths` keys must be sorted");
  for (const path of keys) {
    if (path !== WHOLE_CHECKOUT && (normalize(path) !== path || !PATH_RE.test(path) || path.includes("//") || path.split("/").includes(".."))) {
      throw new Error(`inventory: ${JSON.stringify(path)} is not a normalized repository path`);
    }
    const readers = paths[path];
    if (!Array.isArray(readers) || readers.length === 0) throw new Error(`inventory: ${path} has no readers`);
    for (const reader of readers) {
      const where = `inventory: ${path} reader ${JSON.stringify(reader?.package)} ${JSON.stringify(reader?.target)}`;
      if (typeof reader?.package !== "string" || !/^[A-Za-z0-9_-]+$/.test(reader.package)) throw new Error(`${where}: bad package`);
      if (typeof reader.target !== "string" || !TARGET_RE.test(reader.target)) {
        throw new Error(`${where}: target must be lib, build, or bin:/test:/example:/bench:<name>`);
      }
      for (const field of ["tests", "features"]) {
        if (reader[field] !== undefined && (!Array.isArray(reader[field]) || reader[field].some((item) => typeof item !== "string" || !item))) {
          throw new Error(`${where}: ${field} must be a list of names`);
        }
      }
      if (!Array.isArray(reader.sources) || reader.sources.length === 0 || reader.sources.some((source) => typeof source !== "string" || !source.endsWith(".rs"))) {
        throw new Error(`${where}: sources must list the Rust files that hold the reference`);
      }
      if (reader.scope !== "target" && reader.scope !== "package") throw new Error(`${where}: scope must be target or package`);
      const key = `${path}\0${reader.package}\0${reader.target}`;
      if (seen.has(key)) throw new Error(`${where}: duplicate reader row`);
      seen.add(key);
      entries.push({
        path,
        package: reader.package,
        target: reader.target,
        scope: reader.scope,
        tests: reader.tests ? [...reader.tests] : null,
        features: reader.features ? [...reader.features] : [],
        sources: [...reader.sources],
      });
    }
  }
  const notReaders = document.not_readers ?? [];
  if (!Array.isArray(notReaders)) throw new Error("inventory: `not_readers` must be a list");
  for (const row of notReaders) {
    if (typeof row?.source !== "string" || typeof row.literal !== "string" || typeof row.why !== "string" || row.why.trim().length < 8) {
      throw new Error(`inventory: not_readers row ${JSON.stringify(row)} needs source, literal and why`);
    }
  }
  return { entries, notReaders: notReaders.map((row) => ({ ...row })) };
}

export function loadNonRustReaders(file = resolve(dirname(fileURLToPath(import.meta.url)), "..", INVENTORY_PATH)) {
  return parseNonRustReaders(JSON.parse(readFileSync(file, "utf8")));
}

// Does an inventory key (exact file, or directory prefix ending in "/")
// cover a changed path? A deleted or renamed-away input is still a changed
// path, so a deletion selects its readers like an edit does.
export function inventoryPathCovers(key, changedPath, excludedRoots = []) {
  if (key === WHOLE_CHECKOUT) return !excludedRoots.some((root) => changedPath.startsWith(root));
  return key.endsWith("/") ? changedPath.startsWith(key) : changedPath === key;
}

// The reader test selection for a changed path list: one row per reader
// package and target, with the changed paths and inventory keys that
// selected it. `tests` is null when the whole target runs, else the test
// names that read the inputs (a filter for nextest). `paths` may mix
// additions, edits, deletions and both sides of renames; paths no inventory
// key covers select nothing.
// `excludedRoots` (path prefixes ending in "/") are the product roots the
// whole-checkout readers do not scan.
export function readerSelectionsFor(paths, inventory, { excludedRoots = [] } = {}) {
  const changed = [...new Set((paths ?? []).map(normalize).filter(Boolean))].sort();
  const all = changed.some((path) => READER_DEFINITION_PATHS.includes(path));
  const rows = new Map();
  for (const entry of inventory.entries) {
    const hits = all ? changed.filter((path) => READER_DEFINITION_PATHS.includes(path)) : changed.filter((path) => inventoryPathCovers(entry.path, path, excludedRoots));
    if (hits.length === 0) continue;
    const key = `${entry.package}\0${entry.target}`;
    const row = rows.get(key) ?? {
      package: entry.package,
      target: entry.target,
      scope: "target",
      tests: [],
      features: [],
      paths: [],
      reads: [],
    };
    // Library, binary or build-script code reached by every test of the
    // package widens the row to the package.
    if (entry.scope === "package") row.scope = "package";
    // A reader without a test filter needs its whole target.
    if (row.tests !== null) row.tests = entry.tests === null ? null : [...new Set([...row.tests, ...entry.tests])].sort();
    row.features = [...new Set([...row.features, ...entry.features])].sort();
    row.paths = [...new Set([...row.paths, ...hits])].sort();
    row.reads = [...new Set([...row.reads, entry.path])].sort();
    rows.set(key, row);
  }
  return [...rows.values()].sort((a, b) => (a.package < b.package ? -1 : a.package > b.package ? 1 : a.target < b.target ? -1 : a.target > b.target ? 1 : 0));
}

// The reader packages for a changed path list (sorted, unique): the seeds
// the planner adds for non-Rust inputs.
export function readerPackagesFor(paths, inventory, options = {}) {
  return [...new Set(readerSelectionsFor(paths, inventory, options).map((row) => row.package))].sort();
}
