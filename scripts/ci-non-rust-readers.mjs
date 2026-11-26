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

// What a lane does with a reader row's tests.
//
// The inventory says which tests READ an input. It does not say that a lane
// EXECUTES them: a test marked `#[ignore]` is built with its target and
// skipped by every runner that is not asked for ignored tests. So a reader
// row is reported in states, each read from the files and none assumed:
//   planned              the inventory names the row for the changed paths;
//   built                a lane of the plan compiles its target (the planner
//                        says which, or that the push does);
//   executed             a test the row names carries no ignore attribute;
//   ignored, not executed
//                        every test the row names is ignored, and no file
//                        gives evidence that something runs it;
//   separately executed  every test the row names is ignored, and for each
//                        one a tracked file both carries an ignored-run flag
//                        and names the test: the evidence is that file and
//                        its two lines, never a lane's name. It shows that
//                        a runner selects the test, not that a CI lane of
//                        the plan runs that runner;
//   unproven             every test the row names is ignored and a runner is
//                        claimed but not shown for each of them: the ignore
//                        reason names a lane (`lane:<name>`), or only some of
//                        the tests have evidence, or the evidence could not
//                        be read. Such a row is reported neither as run nor
//                        as not run.
// The functions are pure: the planner (scripts/ci-cargo-lanes.mjs) reads the
// files and passes their text.

// A flag that makes a test runner run ignored tests: libtest's --ignored and
// --include-ignored, nextest's --run-ignored (not its `default` mode) and
// the nextest archive action's run_ignored input. Git has an --ignored of
// its own (status, ls-files, clean), which a line that calls git is taken
// to mean.
const IGNORED_RUN_FLAGS = [
  /(?:^|[^A-Za-z0-9_-])--ignored(?![A-Za-z0-9_-])/,
  /(?:^|[^A-Za-z0-9_-])--include-ignored(?![A-Za-z0-9_-])/,
  /(?:^|[^A-Za-z0-9_-])--run-ignored(?![A-Za-z0-9_-])(?![ =]+default\b)/,
  /\brun_ignored:\s*(?:all|only|ignored-only)\b/,
];
const GIT_LINE = /(?:^|[^A-Za-z0-9_-])git(?![A-Za-z0-9_-])|--porcelain|--untracked-files|\bls-files\b/;

export function hasIgnoredRunFlag(line) {
  return !GIT_LINE.test(line) && IGNORED_RUN_FLAGS.some((flag) => flag.test(line));
}

// The attributes of one piece of text: each `#[...]` with what follows its
// closing bracket up to the next attribute (a trailing comment).
function attributesOf(text) {
  const found = [];
  let at = text.indexOf("#[");
  while (at >= 0) {
    let depth = 0;
    let end = -1;
    let quote = false;
    for (let i = at + 1; i < text.length; i += 1) {
      const ch = text[i];
      if (quote) {
        if (ch === "\\") i += 1;
        else if (ch === '"') quote = false;
      } else if (ch === '"') quote = true;
      else if (ch === "[") depth += 1;
      else if (ch === "]") {
        depth -= 1;
        if (depth === 0) {
          end = i;
          break;
        }
      }
    }
    if (end < 0) return null;
    const next = text.indexOf("#[", end + 1);
    found.push({ inner: text.slice(at + 2, end).trim(), trailing: text.slice(end + 1, next < 0 ? text.length : next).trim() });
    at = next;
  }
  return found;
}

const RUST_FN = /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:const|async|unsafe)\s+)*(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/;

function isTestAttribute(inner) {
  const path = /^[A-Za-z_][A-Za-z0-9_:]*/.exec(inner)?.[0] ?? "";
  const last = path.split("::").pop();
  return last === "test" || last.endsWith("_test") || last === "rstest" || last === "test_case";
}

// The ignore attribute of a test's attribute list: "always" for `#[ignore]`
// in any form, "conditional" for an ignore inside `cfg_attr` (it holds on
// some builds only, so it is never counted as ignored), else "no"; with the
// reason the attribute gives, or the comment behind it.
function ignoreOf(attributes) {
  for (const { inner, trailing } of attributes) {
    if (/^ignore(?![A-Za-z0-9_])/.test(inner)) {
      const literal = /^ignore\s*=\s*"((?:[^"\\]|\\.)*)"/.exec(inner)?.[1];
      const comment = /^\/\/+\s*(.*)$/.exec(trailing)?.[1];
      return { ignored: "always", reason: literal ?? (comment || null) };
    }
  }
  if (attributes.some(({ inner }) => /^cfg_attr\s*\(/.test(inner) && /(?:^|[^A-Za-z0-9_])ignore(?![A-Za-z0-9_])/.test(inner))) {
    return { ignored: "conditional", reason: null };
  }
  return { ignored: "no", reason: null };
}

// Every test function of one Rust source, in order: its name, its line, and
// its ignore attribute. A test is a function whose attributes (the `#[...]`
// lines above it, comments and blank lines between them allowed) hold a
// test attribute: `test`, a path ending in `test` or `_test`, `rstest`,
// `test_case`. Tests a macro generates are not seen.
export function rustTestAttributes(source) {
  const tests = [];
  let pending = [];
  let open = null;
  const lines = String(source).split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();
    if (open !== null) {
      open += ` ${trimmed}`;
      const attributes = attributesOf(open);
      if (attributes !== null) {
        pending.push(...attributes);
        open = null;
      }
      continue;
    }
    if (trimmed.startsWith("#[")) {
      const attributes = attributesOf(trimmed);
      if (attributes === null) open = trimmed;
      else pending.push(...attributes);
      continue;
    }
    if (trimmed === "" || trimmed.startsWith("//")) continue;
    const name = RUST_FN.exec(lines[index])?.[1];
    if (name && pending.some(({ inner }) => isTestAttribute(inner))) {
      tests.push({ name, line: index + 1, ...ignoreOf(pending) });
    }
    pending = [];
  }
  return tests;
}

// The tests a reader row names, with their ignore attributes, read from the
// row's sources (`{ path, text }`): the names of `tests`, or every test of
// the sources for a row without a list. A name found more than once (two
// modules) is ignored only when every occurrence is. `unknown` lists the
// names no source holds; a row with one, or with no test at all, is not
// judged.
export function readerTestState(tests, sources) {
  const found = new Map();
  for (const { path, text } of sources) {
    for (const test of rustTestAttributes(text)) {
      found.set(test.name, [...(found.get(test.name) ?? []), { ...test, source: path }]);
    }
  }
  const names = tests === null ? [...found.keys()].sort() : [...tests].sort();
  const rows = [];
  const unknown = [];
  for (const name of names) {
    const seen = found.get(name);
    if (!seen) {
      unknown.push(name);
      continue;
    }
    const ignored = seen.every((test) => test.ignored === "always")
      ? "always"
      : seen.some((test) => test.ignored === "no")
        ? "no"
        : "conditional";
    const first = seen.find((test) => test.ignored === ignored) ?? seen[0];
    rows.push({ name, ignored, reason: first.reason, source: first.source, line: first.line });
  }
  return { judged: unknown.length === 0 && rows.length > 0, tests: rows, unknown };
}

// A line that is a comment and nothing else (a Rust attribute, `#[...]`, and
// a shebang are not comments).
const COMMENT_LINE = /^\s*(?:\/\/|\/\*|\*|#(?![[!]))/;

// Evidence that something runs an ignored test: each file (`{ path, text }`)
// that holds both an ignored-run flag and the test's name as a whole word,
// with the two nearest such lines. A comment line gives neither, and a line
// that passes the name to `--skip` (the runner's exclusion) is no selector.
// The caller leaves out the test's own sources and documentation. This is
// what the file's text shows, both lines cited: it does not follow a
// variable from the name to the command, and it does not say when or where
// the file's runner runs.
export function ignoredRunEvidence(name, files) {
  const escaped = name.replace(/[^A-Za-z0-9_]/g, "\\$&");
  const word = new RegExp(`(?:^|[^A-Za-z0-9_])${escaped}(?![A-Za-z0-9_])`);
  const skipped = new RegExp(`--skip(?:[ =]+|['"]?,\\s*)['"]?(?:[A-Za-z0-9_]+::)*${escaped}(?![A-Za-z0-9_])`);
  const evidence = [];
  for (const { path, text } of files) {
    const lines = String(text).split(/\r?\n/);
    const flags = [];
    const selectors = [];
    lines.forEach((line, index) => {
      if (COMMENT_LINE.test(line)) return;
      if (hasIgnoredRunFlag(line)) flags.push(index);
      if (word.test(line) && !skipped.test(line)) selectors.push(index);
    });
    if (flags.length === 0 || selectors.length === 0) continue;
    let best = null;
    for (const flag of flags) {
      for (const selector of selectors) {
        if (best === null || Math.abs(flag - selector) < Math.abs(best[0] - best[1])) best = [flag, selector];
      }
    }
    evidence.push({
      path,
      flag_line: best[0] + 1,
      flag: lines[best[0]].trim().slice(0, 160),
      selector_line: best[1] + 1,
      selector: lines[best[1]].trim().slice(0, 160),
    });
  }
  return evidence.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

const LANE_CLAIM = /(?:^|[^A-Za-z0-9_])lane:([A-Za-z0-9_-]+)/;

// The execution state of a reader row from its test state and the evidence
// of its ignored tests: `evidenceOf(name)` gives the evidence list of one
// ignored test, or null when the evidence could not be read.
export function readerExecution(state, evidenceOf) {
  if (!state.judged) {
    return { execution: "not judged", tests: state.tests.length, ignored: 0, ignored_tests: [], unknown: state.unknown };
  }
  const ignoredTests = state.tests
    .filter((test) => test.ignored === "always")
    .map((test) => {
      const evidence = evidenceOf(test.name);
      const claimed = LANE_CLAIM.exec(test.reason ?? "")?.[1] ?? null;
      const execution =
        evidence === null
          ? "unproven"
          : evidence.length > 0
            ? "separately executed"
            : claimed !== null
              ? "unproven"
              : "ignored, not executed";
      return { name: test.name, reason: test.reason, source: test.source, line: test.line, claimed_lane: claimed, execution, evidence: evidence ?? [] };
    });
  const count = (execution) => ignoredTests.filter((test) => test.execution === execution).length;
  let execution;
  if (ignoredTests.length === 0) execution = "executed";
  else if (ignoredTests.length < state.tests.length) execution = "executed in part";
  else if (count("separately executed") === ignoredTests.length) execution = "separately executed";
  else if (count("ignored, not executed") === ignoredTests.length) execution = "ignored, not executed";
  else execution = "unproven";
  return { execution, tests: state.tests.length, ignored: ignoredTests.length, ignored_tests: ignoredTests, unknown: [] };
}
