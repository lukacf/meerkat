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
//                        says which, or that the push does). It is the
//                        plan's own view: another workflow may compile the
//                        same target, and the report does not look there;
//   executed             a test the row names carries no ignore attribute;
//   ignored, not executed
//                        every test the row names is ignored, and no runner
//                        that asks for ignored tests was FOUND for it by
//                        the two rules below. That is a statement about the
//                        search, not that no runner exists;
//   separately executed  every test the row names is ignored, and each one
//                        has evidence in a tracked file: a file that both
//                        carries an ignored-run flag and names the test
//                        (ignoredRunEvidence), or a cargo test command that
//                        asks for ignored tests of the row's whole target
//                        or package (targetRunEvidence). The evidence is
//                        the file and its lines, never a lane's name. It
//                        shows that a runner selects the test, not that a
//                        CI lane of the plan runs that runner;
//   unproven             every test the row names is ignored and a runner is
//                        claimed or possible but not shown for each of
//                        them: the ignore reason names a lane
//                        (`lane:<name>`), or only some of the tests have
//                        evidence, or a command asks for ignored tests and
//                        the text does not say whether it reaches the row
//                        (no package, a filter, a value held elsewhere), or
//                        the evidence could not be read. Such a row is
//                        reported neither as run nor as not run.
// Limits of a textual rule. A test is paired with a flag per file, not per
// command: a file that asks for ignored tests of something else and names
// the test on another line is credited. An ignore inside `cfg_attr` holds
// on some builds only and is read as not ignored. A test that a macro
// generates, or that lives in a file the inventory row does not list, is
// not found.
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
    if (!String(text).includes(name)) continue;
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
      covers: "test",
      flag_line: best[0] + 1,
      flag: lines[best[0]].trim().slice(0, 160),
      selector_line: best[1] + 1,
      selector: lines[best[1]].trim().slice(0, 160),
    });
  }
  return evidence.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

// The commands of a file that hold an ignored-run flag: each flag line with
// the lines a trailing backslash joins to it, as one text.
function flagCommands(text) {
  const lines = String(text).split(/\r?\n/);
  const commands = [];
  let last = -1;
  lines.forEach((line, index) => {
    if (index <= last || !line.includes("ignored") || COMMENT_LINE.test(line) || !hasIgnoredRunFlag(line)) return;
    let first = index;
    while (first > 0 && /\\\s*$/.test(lines[first - 1]) && !COMMENT_LINE.test(lines[first - 1])) first -= 1;
    last = index;
    while (last < lines.length - 1 && /\\\s*$/.test(lines[last])) last += 1;
    commands.push({ line: first + 1, flag_line: index + 1, flag: line.trim(), text: lines.slice(first, last + 1).map((part) => part.replace(/\\\s*$/, " ").trim()).join(" ") });
  });
  return commands;
}

// A file's commands are read once, however many rows ask.
const FLAG_COMMANDS = new WeakMap();
function flagCommandsOf(file) {
  let commands = FLAG_COMMANDS.get(file);
  if (!commands) {
    commands = flagCommands(file.text);
    FLAG_COMMANDS.set(file, commands);
  }
  return commands;
}

// The words of a command: quotes, brackets and the commas of an argument
// list removed, `--option=value` split.
function commandWords(text) {
  return text
    .replace(/["'`[\]()]/g, " ")
    .replace(/,(?=\s|$)/g, " ")
    .replace(/(^|\s)(--?[A-Za-z][A-Za-z0-9-]*)=/g, "$1$2 ")
    .split(/\s+/)
    .filter(Boolean);
}

// The options of cargo test, cargo nextest run and the test binary that take
// a value. A value option this list does not know leaves its value as a
// bare word, which is read as a test filter: the command then proves
// nothing (see targetRunEvidence).
const VALUE_OPTIONS = new Set([
  ..."-p --package --test --bin --example --bench --features -F --profile --cargo-profile --target --target-dir --manifest-path".split(" "),
  ..."-j --jobs --build-jobs --color --message-format --config -Z --partition -E --filter-expr --filterset --test-threads".split(" "),
  ..."--retries --status-level --final-status-level --failure-output --success-output --no-tests --run-ignored --exclude".split(" "),
  ..."--tool-config-file --config-file --archive-file --workspace-remap --format --logfile --skip --show-progress --max-fail".split(" "),
]);

// A command read as `cargo test` or `cargo nextest run`: the packages and
// targets it selects and whatever narrows it to some tests. Null when the
// text holds no such subcommand (a test binary run directly, an argument
// list built over several lines).
function cargoTestCommand(text) {
  const words = commandWords(text);
  const start = words.findIndex((word, index) => word === "test" || (word === "run" && words[index - 1] === "nextest"));
  if (start < 0) return null;
  const nextest = words[start] === "run";
  const command = { packages: [], workspace: false, excluded: [], tests: [], bins: [], examples: [], flags: new Set(), features: [], narrowed: [], unresolved: false };
  const lists = { "-p": command.packages, "--package": command.packages, "--exclude": command.excluded, "--test": command.tests, "--bin": command.bins, "--example": command.examples };
  for (let index = start + 1; index < words.length; index += 1) {
    const word = words[index];
    // Another command of the same line, or a redirection, ends this one.
    if (/^(?:&&|\|\||\||;|\d*[<>].*)$/.test(word)) break;
    if (word === "--") continue;
    // A value the text does not hold (a variable, a substitution).
    if (/[${}]/.test(word)) {
      command.unresolved = true;
      continue;
    }
    if (VALUE_OPTIONS.has(word)) {
      const value = words[index + 1];
      index += 1;
      if (value === undefined || /[${}]/.test(value)) {
        command.unresolved = true;
        continue;
      }
      if (lists[word]) lists[word].push(value);
      else if (word === "--features" || word === "-F") command.features.push(...value.split(","));
      else if (["-E", "--filter-expr", "--filterset", "--skip", "--partition"].includes(word)) command.narrowed.push(word);
      else if (word === "--profile" && nextest) command.narrowed.push("a nextest profile");
      continue;
    }
    if (word.startsWith("-")) {
      if (word === "--workspace" || word === "--all") command.workspace = true;
      else if (word === "--exact") command.narrowed.push(word);
      else command.flags.add(word.replace(/;$/, ""));
      if (word.endsWith(";")) break;
      continue;
    }
    command.narrowed.push("a test filter");
  }
  return command;
}

// Evidence that a runner asks for ignored tests of a row's WHOLE target or
// package, which names no test function: in each file (`{ path, text }`), a
// `cargo test` or `cargo nextest run` command that holds an ignored-run flag,
// names the row's package (`-p`, or `--workspace` without excluding it) and
// selects its target (`--test <name>`, `--lib`, `--bin <name>`,
// `--example <name>`, a kind-wide or all-targets selector, or no target
// selector at all), with nothing that narrows it to some tests and with the
// row's features. Such a command covers every test of the row: `covering`.
// A command the text cannot settle goes to `unclear` with the reason, and
// gives no credit: one that names the target and no package; one for the
// package with a filter, a nextest profile, a partition, features the row
// needs and the command does not name, or a value the text does not hold;
// and a flag outside any cargo test command in a file that selects the
// target by name on another line. A command for another package, another
// target, or without the flag is told apart and is neither.
export function targetRunEvidence({ package: pkg, target, features = [] }, files) {
  const colon = target.indexOf(":");
  const kind = colon < 0 ? target : target.slice(0, colon);
  const name = colon < 0 ? null : target.slice(colon + 1);
  const option = { test: "--test", bin: "--bin", example: "--example" }[kind] ?? null;
  const kindWide = { test: "--tests", bin: "--bins", example: "--examples", lib: "--lib" }[kind] ?? null;
  const covering = [];
  const unclear = [];
  for (const file of files) {
    const { path, text } = file;
    const commands = flagCommandsOf(file);
    if (commands.length === 0) continue;
    // The file selects the row's target by name on some line that is no
    // comment.
    let named = null;
    const namesTarget = () => {
      if (named === null) {
        named = false;
        if (option !== null && String(text).includes(name)) {
          const selector = new RegExp(`${option}(?:=|["',\\s]+)["']?${name.replace(/[^A-Za-z0-9_]/g, "\\$&")}(?![A-Za-z0-9_-])`, "g");
          for (let match = selector.exec(text); match !== null && !named; match = selector.exec(text)) {
            const from = text.lastIndexOf("\n", match.index) + 1;
            named = !COMMENT_LINE.test(text.slice(from, match.index + 1));
          }
        }
      }
      return named;
    };
    for (const command of commands) {
      const item = { path, line: command.line, flag_line: command.flag_line, command: command.text.slice(0, 200) };
      const parsed = cargoTestCommand(command.text);
      if (parsed === null) {
        if (namesTarget()) unclear.push({ ...item, why: `an ignored-run flag outside a cargo test command, in a file that selects ${option} ${name}` });
        continue;
      }
      const explicit = option !== null && { test: parsed.tests, bin: parsed.bins, example: parsed.examples }[kind].includes(name);
      const selectors = parsed.tests.length + parsed.bins.length + parsed.examples.length + ["--lib", "--bins", "--tests", "--examples", "--benches", "--all-targets", "--doc"].filter((flag) => parsed.flags.has(flag)).length;
      const selects =
        explicit || (kindWide !== null && parsed.flags.has(kindWide)) || parsed.flags.has("--all-targets") || (selectors === 0 && kind !== "example" && kind !== "build");
      if (!selects) continue;
      const reaches = parsed.packages.includes(pkg) || (parsed.workspace && !parsed.excluded.includes(pkg));
      if (!reaches) {
        // Another package's run is told apart; a run that names the target
        // and no package cannot be.
        if (parsed.packages.length === 0 && !parsed.workspace && explicit) unclear.push({ ...item, why: `selects ${option} ${name} and names no package` });
        continue;
      }
      const reasons = [...new Set(parsed.narrowed)];
      if (parsed.unresolved) reasons.push("a value the text does not hold");
      const missing = parsed.flags.has("--all-features") ? [] : features.filter((feature) => !parsed.features.includes(feature));
      if (missing.length > 0) reasons.push(`without the row's features ${missing.join(",")}`);
      if (reasons.length > 0) unclear.push({ ...item, why: `narrowed or unsettled: ${reasons.join("; ")}` });
      else covering.push({ ...item, covers: explicit || (kindWide !== null && parsed.flags.has(kindWide) && kind === "lib") ? "target" : "package" });
    }
  }
  const byPlace = (a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : a.line - b.line);
  return { covering: covering.sort(byPlace), unclear: unclear.sort(byPlace) };
}

const LANE_CLAIM = /(?:^|[^A-Za-z0-9_])lane:([A-Za-z0-9_-]+)/;

// The execution state of a reader row from its test state and the evidence
// of its ignored tests: `evidenceOf(name)` gives the evidence list of one
// ignored test, or null when the evidence could not be read; `whole` is the
// evidence for the row's whole target or package (targetRunEvidence). A
// covering command credits every ignored test of the row; an unclear one
// credits none and leaves a test without evidence of its own unproven.
export function readerExecution(state, evidenceOf, whole = { covering: [], unclear: [] }) {
  if (!state.judged) {
    return { execution: "not judged", tests: state.tests.length, ignored: 0, ignored_tests: [], unclear_runners: [], unknown: state.unknown };
  }
  const covering = whole.covering.map((item) => ({
    path: item.path,
    covers: item.covers,
    flag_line: item.flag_line,
    flag: item.command.slice(0, 160),
    selector_line: item.line,
    selector: item.command.slice(0, 160),
  }));
  const ignoredTests = state.tests
    .filter((test) => test.ignored === "always")
    .map((test) => {
      const own = evidenceOf(test.name);
      const evidence = [...(own ?? []), ...covering];
      const claimed = LANE_CLAIM.exec(test.reason ?? "")?.[1] ?? null;
      const execution =
        evidence.length > 0
          ? "separately executed"
          : own === null || claimed !== null || whole.unclear.length > 0
            ? "unproven"
            : "ignored, not executed";
      return { name: test.name, reason: test.reason, source: test.source, line: test.line, claimed_lane: claimed, execution, evidence };
    });
  const count = (execution) => ignoredTests.filter((test) => test.execution === execution).length;
  let execution;
  if (ignoredTests.length === 0) execution = "executed";
  else if (ignoredTests.length < state.tests.length) execution = "executed in part";
  else if (count("separately executed") === ignoredTests.length) execution = "separately executed";
  else if (count("ignored, not executed") === ignoredTests.length) execution = "ignored, not executed";
  else execution = "unproven";
  return {
    execution,
    tests: state.tests.length,
    ignored: ignoredTests.length,
    ignored_tests: ignoredTests,
    // The commands that ask for ignored tests and may or may not reach the
    // row: what leaves it unproven.
    unclear_runners: ignoredTests.length > 0 ? whole.unclear : [],
    unknown: [],
  };
}
