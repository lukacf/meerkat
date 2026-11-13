#!/usr/bin/env node
// The reasoning effort execution receipt.
//
// A receipt says, for one commit of one product version, what was executed
// for the provider-neutral reasoning effort level and what was not: per cell
// of the matrix the provider family, the model, the provider API, the route,
// the level asked for and the level sent or the refusal reason; per leg the
// command, its exit status and its pass, fail and skip counts; for every
// entry whether it was executed (in process, through the built binary, or
// live) or is only in the source; and every gap as an entry of its own. It is
// one JSON document (scripts/reasoning-effort-receipt.schema.json) and a
// Markdown rendering of that document.
//
// Inputs:
//   - the row manifest (scripts/reasoning-effort-receipt-rows.json): the
//     cells, the legs and the rows, each row with the test that covers it;
//   - one leg result per leg of the manifest, `<leg>.json` in the results
//     directory: what the leg's run recorded, or the statement that it did
//     not run and why. `record` writes one from the test runner's output.
//   Both are validated against scripts/reasoning-effort-receipt-inputs.schema.json.
//
// Rules the generator keeps:
//   - It invents no result. An entry is `executed` only when the leg result
//     of its leg ran at the product commit and names the entry's test (or
//     cell) with its own outcome; the entry then carries that result file's
//     name and digest. A leg without a result file, a result file of no leg,
//     and a file that does not parse or does not match its schema are errors.
//   - Everything else is listed, not dropped: a row whose test is in the
//     source and has no result of its own is `source_only`, a cell without a
//     row is `not_run`, and each is repeated under `gaps` with its reason.
//   - It reads the version (`Cargo.toml`), the manifest and the test sources
//     from the product commit itself (`git show`), and checks that every
//     test a row names is defined in the file the row names.
//   - It writes no free text from a run: a leg result holds counts, test
//     names, typed cells and a digest of the runner's output, never the
//     output. Of what a test prints it reads only two kinds of line. A cell
//     line: a row says which model and level it picked for a cell, the
//     prefix `reasoning-effort-receipt-cell ` and one JSON object of exactly
//     test, cell, model, level and route. An observation line: a row whose
//     cells only its run knows (the models of a live account) says each cell
//     it reached, the prefix `reasoning-effort-receipt-observation ` and one
//     JSON object that is exactly an `observation` of the inputs schema; an
//     object with any other member is refused. A failed cell may carry the
//     HTTP status and an error code as typed fields (the code a word of the
//     row's own failure classes), never a message. Any string that looks like a credential, an e-mail address, a
//     URL, an absolute path, or this host's name or home directory is
//     refused, in a leg result and again in the finished receipt.
//   - The same inputs give the same bytes: no clock, no environment, fixed
//     key and entry order. The Markdown file is rendered from the parsed
//     JSON text and from nothing else.
//   - It judges nothing on its own: a receipt with failed entries and gaps
//     is written like any other. `--check` judges it: it fails on any failed
//     entry and on any gap of the in-process and built-binary legs, which
//     need no credential and can always run. Gaps of the live legs are
//     listed and never fail it.
//
// Usage:
//   reasoning-effort-receipt.mjs record --leg ID --commit SHA --command TEXT
//       --exit-status N (--nextest-json FILE | --nextest-log FILE)
//       [--observations FILE] --output FILE
//     FILE is the run's output: `cargo nextest run --message-format
//     libtest-json-plus` (stdout; every test is named), or the human log (the
//     summary line's counts, and the tests whose status lines it shows). The
//     rows' cell lines and observation lines are in the JSON output of a run
//     with `--no-capture`, and in the log of a run with `--success-output
//     final`. --observations FILE adds a JSON list of observations to those
//     the output holds.
//   reasoning-effort-receipt.mjs record --leg ID --not-run REASON --output FILE
//   reasoning-effort-receipt.mjs generate --results DIR --output-dir DIR
//       [--commit REV] [--manifest FILE] [--repo DIR] [--check]
//     writes reasoning-effort-receipt.json and reasoning-effort-receipt.md.
//   reasoning-effort-receipt.mjs render RECEIPT.json
//   reasoning-effort-receipt.mjs verify RECEIPT.json [--markdown FILE]
//       [--results DIR] [--check]
//   Exit 0 done, 1 refused (the reason is printed), 2 usage, 3 the check
//   holds something against the receipt (which is written all the same).

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { homedir, hostname } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const MANIFEST_PATH = "scripts/reasoning-effort-receipt-rows.json";
export const RECEIPT_JSON_NAME = "reasoning-effort-receipt.json";
export const RECEIPT_MARKDOWN_NAME = "reasoning-effort-receipt.md";
export const MODES = ["in_process", "built_binary", "live"];
// What a row prints before the JSON object that says which model and level
// it picked for a cell (the `cell_line` of the inputs schema).
export const CELL_LINE_PREFIX = "reasoning-effort-receipt-cell ";
// What a row with observed cells prints before the JSON object of one cell
// it saw (the `observation` of the inputs schema).
export const OBSERVATION_LINE_PREFIX = "reasoning-effort-receipt-observation ";
// The legs that need no credential and no network: they can always run, so
// `--check` takes a gap there for a defect of the run.
export const OFFLINE_MODES = ["in_process", "built_binary"];
// The levels, lowest first: the order the entries of one cell are listed in
// where a row has an entry for each level it asks for.
export const LEVELS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// A refusal: an input the generator will not read a result from.
export class ReceiptError extends Error {}

// --- Schemas ---------------------------------------------------------------

const ANNOTATIONS = new Set(["$schema", "$id", "$defs", "$comment", "title", "description"]);
const KEYWORDS = new Set([
  "$ref",
  "additionalProperties",
  "const",
  "enum",
  "items",
  "maxItems",
  "maxLength",
  "maximum",
  "minItems",
  "minLength",
  "minimum",
  "oneOf",
  "pattern",
  "properties",
  "required",
  "type",
]);

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function typeOf(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (Number.isInteger(value)) return "integer";
  return typeof value;
}

/// The violations of `schema` by `value`, for the JSON Schema keywords the
/// committed schemas use. A keyword outside that set is an error of the
/// schema, so a schema can never ask for a check this function skips.
export function schemaErrors(root, schema, value, path = "$") {
  for (const keyword of Object.keys(schema)) {
    if (!KEYWORDS.has(keyword) && !ANNOTATIONS.has(keyword)) {
      throw new ReceiptError(`schema keyword ${keyword} at ${path} is not supported`);
    }
  }
  const errors = [];
  if (schema.$ref !== undefined) {
    const name = /^#\/\$defs\/([A-Za-z0-9_]+)$/.exec(schema.$ref)?.[1];
    const target = name === undefined ? undefined : root.$defs?.[name];
    if (!isObject(target)) throw new ReceiptError(`schema reference ${schema.$ref} at ${path} names no definition`);
    errors.push(...schemaErrors(root, target, value, path));
  }
  if (schema.const !== undefined && value !== schema.const) {
    errors.push(`${path} is not ${JSON.stringify(schema.const)}`);
  }
  if (schema.enum !== undefined && !schema.enum.includes(value)) {
    errors.push(`${path} is none of ${schema.enum.map((item) => JSON.stringify(item)).join(", ")}`);
  }
  const actual = typeOf(value);
  if (schema.type !== undefined) {
    const allowed = [schema.type].flat();
    if (!allowed.includes(actual) && !(actual === "integer" && allowed.includes("number"))) {
      errors.push(`${path} is ${actual}, not ${allowed.join(" or ")}`);
      return errors;
    }
  }
  if (actual === "string") {
    const length = [...value].length;
    if (schema.pattern !== undefined && !new RegExp(schema.pattern, "u").test(value)) {
      errors.push(`${path} does not match ${schema.pattern}`);
    }
    if (schema.minLength !== undefined && length < schema.minLength) errors.push(`${path} is shorter than ${schema.minLength}`);
    if (schema.maxLength !== undefined && length > schema.maxLength) errors.push(`${path} is longer than ${schema.maxLength}`);
  }
  if (actual === "integer" || actual === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) errors.push(`${path} is below ${schema.minimum}`);
    if (schema.maximum !== undefined && value > schema.maximum) errors.push(`${path} is above ${schema.maximum}`);
  }
  if (actual === "array") {
    if (schema.minItems !== undefined && value.length < schema.minItems) errors.push(`${path} has fewer than ${schema.minItems} items`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) errors.push(`${path} has more than ${schema.maxItems} items`);
    if (schema.items !== undefined) {
      value.forEach((item, index) => errors.push(...schemaErrors(root, schema.items, item, `${path}[${index}]`)));
    }
  }
  if (actual === "object") {
    for (const key of schema.required ?? []) {
      if (!Object.hasOwn(value, key)) errors.push(`${path} has no ${key}`);
    }
    for (const [key, item] of Object.entries(value)) {
      const property = schema.properties !== undefined && Object.hasOwn(schema.properties, key) ? schema.properties[key] : undefined;
      if (property !== undefined) {
        errors.push(...schemaErrors(root, property, item, `${path}.${key}`));
      } else if (schema.additionalProperties === false) {
        errors.push(`${path} has a member ${JSON.stringify(key)} its schema does not know`);
      } else if (isObject(schema.additionalProperties)) {
        errors.push(...schemaErrors(root, schema.additionalProperties, item, `${path}.${key}`));
      }
    }
  }
  if (schema.oneOf !== undefined) {
    const branches = schema.oneOf.map((branch) => schemaErrors(root, branch, value, path));
    const matching = branches.filter((branch) => branch.length === 0).length;
    if (matching === 0) {
      errors.push(`${path} has none of its ${branches.length} allowed forms (${branches.map((branch) => branch[0]).join("; ")})`);
    } else if (matching > 1) {
      errors.push(`${path} has ${matching} of its allowed forms at once`);
    }
  }
  return errors;
}

let loadedSchemas;

/// The two committed schema files, read from beside this script.
export function schemas() {
  loadedSchemas ??= {
    receipt: JSON.parse(readFileSync(join(here, "reasoning-effort-receipt.schema.json"), "utf8")),
    inputs: JSON.parse(readFileSync(join(here, "reasoning-effort-receipt-inputs.schema.json"), "utf8")),
  };
  return loadedSchemas;
}

function assertValid(root, definition, value, what) {
  const schema = definition === null ? root : { $ref: `#/$defs/${definition}` };
  const errors = schemaErrors(root, schema, value);
  if (errors.length > 0) {
    const shown = errors.slice(0, 8).join("; ");
    throw new ReceiptError(`${what} does not match its schema: ${shown}${errors.length > 8 ? `; and ${errors.length - 8} more` : ""}`);
  }
}

function parseJson(text, what) {
  try {
    return JSON.parse(text);
  } catch {
    throw new ReceiptError(`${what} is not JSON`);
  }
}

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/// The bytes a document is written as: two-space JSON and one newline.
export function serialize(document) {
  return `${JSON.stringify(document, null, 2)}\n`;
}

// --- What may be published -------------------------------------------------

const FORBIDDEN_TEXT = [
  ["a control character", /[\u0000-\u001f\u007f]/u],
  ["a URL", /[A-Za-z][A-Za-z0-9+.-]*:\/\//u],
  ["an absolute path", /(?:^|[\s"'`=:,;])(?:\/(?!\/)[^\s/]|~\/|[A-Za-z]:\\)/u],
  ["an e-mail address", /[^\s@]+@[^\s@]+\.[A-Za-z]{2,}/u],
  [
    "a credential",
    /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{16,}|AIza[0-9A-Za-z_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})/u,
  ],
  ["a bearer credential", /\bbearer\s+[A-Za-z0-9._~+/=-]{8,}/iu],
  [
    "a value assigned to a secret's name",
    /\b(?:[A-Za-z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|AUTHORIZATION|CREDENTIAL)[A-Za-z0-9_]*|[A-Za-z0-9_]*KEY)\s*=\s*\S/iu,
  ],
];

/// The facts of this host a receipt must not carry.
export function hostFacts() {
  return { hostname: hostname(), home: homedir() };
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function hostPatterns(host) {
  const patterns = [];
  const names = new Set([host.hostname, host.hostname.split(".")[0]]);
  for (const name of names) {
    // A short name is a common word more often than a host.
    if (name.length >= 6) {
      patterns.push(["this host's name", new RegExp(`(?:^|[^A-Za-z0-9-])${escapeRegExp(name)}(?:$|[^A-Za-z0-9-])`, "iu")]);
    }
  }
  if (host.home.length >= 2) patterns.push(["this host's home directory", new RegExp(escapeRegExp(host.home), "u")]);
  return patterns;
}

/// Refuses `document` when any string in it, a member name included, is
/// something a receipt never carries. The message names the place and the
/// kind, not the text.
export function assertPublishable(document, host, what) {
  const patterns = [...FORBIDDEN_TEXT, ...hostPatterns(host)];
  const visit = (value, path) => {
    if (typeof value === "string") {
      for (const [kind, pattern] of patterns) {
        if (pattern.test(value)) throw new ReceiptError(`${what} holds ${kind} at ${path}`);
      }
    } else if (Array.isArray(value)) {
      value.forEach((item, index) => visit(item, `${path}[${index}]`));
    } else if (isObject(value)) {
      for (const [key, item] of Object.entries(value)) {
        visit(key, `${path} (a member name)`);
        visit(item, `${path}.${key}`);
      }
    }
  };
  visit(document, "$");
}

// --- The test runner's output ----------------------------------------------

function byTest(a, b) {
  return compare(a.package, b.package) || compare(a.target, b.target) || compare(a.name, b.name);
}

function compare(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function countsOf(tests, skipped) {
  const passed = tests.filter((test) => test.outcome === "pass").length;
  const failed = tests.filter((test) => test.outcome === "fail").length;
  return { run: passed + failed, passed, failed, skipped };
}

function byCellLine(a, b) {
  return compare(a.test, b.test) || compare(a.cell, b.cell) || LEVELS.indexOf(a.level) - LEVELS.indexOf(b.level);
}

/// The cell lines of a run's output, each once. A line is the prefix and one
/// JSON object of exactly the `cell_line` members; a line with the prefix
/// and anything else is refused. A row may print several lines of one cell,
/// one per level it asks for there; two lines that say different things of
/// one level of one cell of one test are refused.
function cellLines() {
  const lines = new Map();
  return {
    /// Takes `text` when it is a cell line; says whether it was one.
    take(text, place) {
      const line = text.trim();
      if (!line.startsWith(CELL_LINE_PREFIX)) return false;
      const item = parseJson(line.slice(CELL_LINE_PREFIX.length), `the cell line at ${place}`);
      assertValid(schemas().inputs, "cell_line", item, `the cell line at ${place}`);
      const picked = { test: item.test, cell: item.cell, model: item.model, level: item.level, route: item.route };
      const key = `${picked.test}\n${picked.cell}\n${picked.level}`;
      const earlier = lines.get(key);
      if (earlier !== undefined && JSON.stringify(earlier) !== JSON.stringify(picked)) {
        throw new ReceiptError(`the cell line at ${place} says another thing of ${picked.test}, cell ${picked.cell}, level ${picked.level}, than an earlier line`);
      }
      lines.set(key, picked);
      return true;
    },
    list: () => [...lines.values()].sort(byCellLine),
  };
}

// How nextest opens the line of an event in its `libtest-json` output.
const EVENT_OPENINGS = ['{"type":"suite",', '{"type":"test",'];

/// An observation with its members in the schema's order and nothing else.
/// Call it on an item that already matches the `observation` definition.
function orderedObservation(item) {
  return {
    row: item.row,
    provider_family: item.provider_family,
    backend: item.backend,
    api: item.api,
    model: item.model,
    requested_level: item.requested_level,
    outcome: item.outcome.kind === "sent" ? { kind: "sent", level: item.outcome.level } : { kind: "refused", reason: item.outcome.reason },
    result: item.result,
    ...(item.http_status === undefined ? {} : { http_status: item.http_status }),
    ...(item.error_code === undefined ? {} : { error_code: item.error_code }),
  };
}

/// The observation lines of a run's output, each once. A line is the prefix
/// and one JSON object that is exactly an `observation`: an object with any
/// other member is refused, and so are two lines that say different things
/// of one cell (a result, a status or a code that differs).
function observationLines() {
  const seen = new Map();
  return {
    /// Takes `text` when it is an observation line; says whether it was one.
    take(text, place) {
      const line = text.trim();
      if (!line.startsWith(OBSERVATION_LINE_PREFIX)) return false;
      const item = parseJson(line.slice(OBSERVATION_LINE_PREFIX.length), `the observation line at ${place}`);
      assertValid(schemas().inputs, "observation", item, `the observation line at ${place}`);
      const observed = orderedObservation(item);
      const key = observationKey(observed);
      const earlier = seen.get(key);
      if (earlier !== undefined && JSON.stringify(earlier) !== JSON.stringify(observed)) {
        throw new ReceiptError(`the observation line at ${place} says another thing of a cell of ${observed.row} than an earlier line`);
      }
      seen.set(key, observed);
      return true;
    },
    list: () => [...seen.values()],
  };
}

/// `cargo nextest run --message-format libtest-json-plus`: one JSON event a
/// line. Reads the outcome event of every test and the suite events that
/// say which target a test binary is. Of what the tests print it reads the
/// cell lines and the observation lines and nothing else: as whole lines of the output (a run with
/// `--no-capture`, where the tests' own output shares the stream with the
/// events), and inside a failed test's captured output. An event is a line
/// that opens as nextest writes one (`{"type":"suite",` or `{"type":"test",`);
/// every other line is a test's output and is not read, whatever it holds.
/// An event that does not parse, or that follows other text on its line, is
/// refused.
export function parseNextestJson(text) {
  const targets = new Map();
  const finished = [];
  const cells = cellLines();
  const observed = observationLines();
  text.split("\n").forEach((line, index) => {
    if (line.trim() === "") return;
    const place = `line ${index + 1} of the nextest JSON output`;
    if (cells.take(line, place) || observed.take(line, place)) return;
    if (!EVENT_OPENINGS.some((opening) => line.trimStart().startsWith(opening))) {
      if (EVENT_OPENINGS.some((opening) => line.includes(opening))) throw new ReceiptError(`${place} holds an event after other text`);
      return;
    }
    const event = parseJson(line, place);
    if (event.type === "suite") {
      const meta = event.nextest;
      if (!isObject(meta) || [meta.crate, meta.test_binary, meta.kind].some((item) => typeof item !== "string")) {
        throw new ReceiptError(`${place} is a suite event without nextest's crate, test_binary and kind: record the run with --message-format libtest-json-plus`);
      }
      const target = meta.kind === "lib" ? "lib" : meta.kind === "test" ? meta.test_binary : `${meta.kind}/${meta.test_binary}`;
      targets.set(`${meta.crate}::${meta.test_binary}`, { package: meta.crate, target });
      return;
    }
    if (event.event === "started") return;
    const outcome = { ok: "pass", failed: "fail", ignored: "skip" }[event.event];
    if (outcome === undefined) throw new ReceiptError(`${place} is a test event this reader does not know`);
    const name = /^([^$#]+)\$([^$#]+?)(?:#([1-9][0-9]*))?$/u.exec(typeof event.name === "string" ? event.name : "");
    if (name === null) throw new ReceiptError(`${place} names its test in a form this reader does not know`);
    if (typeof event.stdout === "string") {
      for (const printed of event.stdout.split("\n")) {
        if (!cells.take(printed, `${place} (the test's output)`)) observed.take(printed, `${place} (the test's output)`);
      }
    }
    finished.push({ binary: name[1], name: name[2], outcome, tries: name[3] === undefined ? 1 : Number(name[3]), place });
  });
  if (targets.size === 0) throw new ReceiptError("the nextest JSON output holds no suite event");
  const tests = new Map();
  for (const item of finished) {
    const target = targets.get(item.binary);
    if (target === undefined) throw new ReceiptError(`${item.place} names a test binary no suite event describes`);
    const key = `${target.package}\n${target.target}\n${item.name}`;
    if (tests.has(key)) throw new ReceiptError(`${item.place} is a second outcome of one test`);
    tests.set(key, { ...target, name: item.name, outcome: item.outcome, tries: item.tries });
  }
  const list = [...tests.values()].sort(byTest);
  return {
    format: "nextest_libtest_json_plus",
    counts: countsOf(list, list.filter((test) => test.outcome === "skip").length),
    tests_complete: true,
    tests: list,
    cell_lines: cells.list(),
    observations: observed.list(),
  };
}

const LOG_SUMMARY = /^\s*Summary \[\s*[0-9]+\.[0-9]+s\] ([0-9]+) tests? run: ([0-9]+) passed(?: \([0-9]+ slow\))?(?:, ([0-9]+) failed)?, ([0-9]+) skipped$/u;
const LOG_STATUS = /^ {8}(PASS|FAIL) \[\s*[0-9]+\.[0-9]{3}s\] \(\s*[0-9]+\/[0-9]+\) (\S+) (\S+)$/u;
const LOG_ANY_STATUS = /^\s*([A-Z][A-Z0-9 /]*?) \[[^\]]*\] \((?:\s*[0-9]+\/[0-9]+|─+)\) \S+ \S+$/u;

/// nextest's human log: the counts of its one summary line, the tests whose
/// PASS or FAIL status lines it shows (a run with `--status-level fail`
/// shows none of the passes), and the cell lines it shows (a run with
/// `--success-output final` or `--no-capture` shows a passing test's). A
/// summary or a status line of another shape (a retried, timed out or
/// leaked test) is refused: record such a run from the JSON output, which
/// names every test.
export function parseNextestLog(text) {
  let summary = null;
  const tests = new Map();
  const cells = cellLines();
  const observed = observationLines();
  text.split("\n").forEach((raw, index) => {
    const line = raw.replace(/\r$/u, "");
    const place = `line ${index + 1} of the nextest log`;
    if (cells.take(line, place) || observed.take(line, place)) return;
    if (/^\s*Summary \[/u.test(line)) {
      const match = LOG_SUMMARY.exec(line);
      if (match === null) throw new ReceiptError(`${place} is a summary this reader does not know`);
      if (summary !== null) throw new ReceiptError(`${place} is a second summary line`);
      summary = { run: Number(match[1]), passed: Number(match[2]), failed: Number(match[3] ?? 0), skipped: Number(match[4]) };
      return;
    }
    const status = LOG_ANY_STATUS.exec(line);
    if (status === null || status[1] === "SLOW") return;
    const match = LOG_STATUS.exec(line);
    if (match === null) throw new ReceiptError(`${place} is a test status this reader does not know`);
    const separator = match[2].indexOf("::");
    const test = {
      package: separator === -1 ? match[2] : match[2].slice(0, separator),
      target: separator === -1 ? "lib" : match[2].slice(separator + 2),
      name: match[3],
      outcome: match[1] === "PASS" ? "pass" : "fail",
      tries: 1,
    };
    const key = `${test.package}\n${test.target}\n${test.name}`;
    const earlier = tests.get(key);
    if (earlier !== undefined && earlier.outcome !== test.outcome) throw new ReceiptError(`${place} is a second, different outcome of one test`);
    tests.set(key, test);
  });
  if (summary === null) throw new ReceiptError("the nextest log has no summary line");
  const list = [...tests.values()].sort(byTest);
  return {
    format: "nextest_human_log",
    counts: summary,
    tests_complete: list.length === summary.run,
    tests: list,
    cell_lines: cells.list(),
    observations: observed.list(),
  };
}

/// What a leg result must agree with itself on, beyond its schema.
function checkLegResult(result, what) {
  if (result.status !== "run") return;
  const { counts, tests } = result;
  if (counts.run !== counts.passed + counts.failed) throw new ReceiptError(`${what}: ${counts.run} tests run is not its passed and failed tests together`);
  const keys = new Set(tests.map((test) => `${test.package}\n${test.target}\n${test.name}`));
  if (keys.size !== tests.length) throw new ReceiptError(`${what} names a test twice`);
  const listed = countsOf(tests, 0);
  if (listed.passed > counts.passed || listed.failed > counts.failed) throw new ReceiptError(`${what} names more passed or failed tests than its counts hold`);
  if (result.tests_complete && (listed.passed !== counts.passed || listed.failed !== counts.failed)) {
    throw new ReceiptError(`${what} says it names every test it ran, and its counts hold others`);
  }
  if (result.exit_status === 0 && counts.failed > 0) throw new ReceiptError(`${what} has exit status 0 and ${counts.failed} failed tests`);
  const cells = new Set(result.observations.map((item) => observationKey(item)));
  if (cells.size !== result.observations.length) throw new ReceiptError(`${what} holds one observation twice`);
  for (const item of result.observations) {
    if (item.result !== "fail" && (item.http_status !== undefined || item.error_code !== undefined)) {
      throw new ReceiptError(`${what} holds an HTTP status or an error code on an observation that did not fail`);
    }
  }
  const lines = new Set(result.cell_lines.map((item) => `${item.test}\n${item.cell}\n${item.level}`));
  if (lines.size !== result.cell_lines.length) throw new ReceiptError(`${what} holds two cell lines of one level of one cell of one test`);
}

function observationKey(item) {
  const outcome = item.outcome.kind === "sent" ? `sent ${item.outcome.level}` : `refused ${item.outcome.reason}`;
  return [item.row, item.provider_family, item.backend, item.api, item.model, item.requested_level, outcome].join("\n");
}

/// The leg result of a run: what `record` writes.
export function buildLegResult({ leg, commit, command, exitStatus, format, output, observations = [], host }) {
  const text = Buffer.from(output).toString("utf8");
  const parsed = format === "nextest_libtest_json_plus" ? parseNextestJson(text) : parseNextestLog(text);
  // The cells a row observed: the lines the run printed, and those handed
  // over in a file. Each is held to the schema on its own, so a member the
  // schema does not know is refused before anything is kept of the item.
  const given = observations.map((item, index) => {
    assertValid(schemas().inputs, "observation", item, `observation ${index + 1} of the observations given`);
    return orderedObservation(item);
  });
  const result = {
    schema_version: 1,
    kind: "reasoning_effort_leg_result",
    leg,
    status: "run",
    commit,
    command,
    exit_status: exitStatus,
    output: { format: parsed.format, sha256: sha256(output), bytes: Buffer.byteLength(output) },
    counts: parsed.counts,
    tests_complete: parsed.tests_complete,
    tests: parsed.tests,
    cell_lines: parsed.cell_lines,
    observations: [...given, ...parsed.observations].sort((a, b) => compare(observationKey(a), observationKey(b))),
  };
  const what = `the result of leg ${leg}`;
  assertValid(schemas().inputs, "leg_result", result, what);
  checkLegResult(result, what);
  assertPublishable(result, host, what);
  return result;
}

/// The leg result of a leg that did not run.
export function buildNotRunResult({ leg, reason }) {
  const result = { schema_version: 1, kind: "reasoning_effort_leg_result", leg, status: "not_run", reason };
  assertValid(schemas().inputs, "leg_result", result, `the result of leg ${leg}`);
  return result;
}

// --- The manifest ----------------------------------------------------------

function unique(items, what) {
  const seen = new Set();
  for (const item of items) {
    if (seen.has(item)) throw new ReceiptError(`the row manifest names ${what} ${item} twice`);
    seen.add(item);
  }
  return seen;
}

/// The manifest's references, checked: every id once, every leg, cell and
/// rule a row names defined, and a model and a level for every cell of a
/// row that has a test.
function checkManifest(manifest) {
  const rules = unique(manifest.rules.map((rule) => rule.id), "the rule");
  const cells = new Map(manifest.cells.map((cell) => [cell.id, cell]));
  unique(manifest.cells.map((cell) => cell.id), "the cell");
  const legs = new Map(manifest.legs.map((leg) => [leg.id, leg]));
  unique(manifest.legs.map((leg) => leg.id), "the leg");
  unique(manifest.rows.map((row) => row.id), "the row");
  const checkRule = (value, row) => {
    if (value?.rule !== undefined && !rules.has(value.rule)) throw new ReceiptError(`row ${row.id} of the manifest names the rule ${value.rule}, which it does not define`);
  };
  for (const row of manifest.rows) {
    if (!legs.has(row.leg)) throw new ReceiptError(`row ${row.id} of the manifest names the leg ${row.leg}, which it does not define`);
    if (row.cells === "observed") continue;
    unique(row.cells.map((item) => `${row.id}/${item.cell}`), "the entry");
    for (const item of row.cells) {
      if (!cells.has(item.cell)) throw new ReceiptError(`row ${row.id} of the manifest names the cell ${item.cell}, which it does not define`);
      const model = item.model ?? row.model;
      const level = item.requested_level ?? row.requested_level;
      checkRule(model, row);
      checkRule(level, row);
      if (row.test !== null && (model === null || level === null)) {
        throw new ReceiptError(`row ${row.id} of the manifest has a test and no model or no level for the cell ${item.cell}`);
      }
      // A row with an entry per level takes its levels from what it prints:
      // one level fixed here would be one of several.
      if (row.lines === "per_level" && level?.level !== undefined) {
        throw new ReceiptError(`row ${row.id} of the manifest has an entry for each level and fixes one level for the cell ${item.cell}`);
      }
    }
  }
  return { cells, legs };
}

/// Whether the Rust source `text` defines the test function `name` (the
/// last segment of its path).
export function definesTest(text, name) {
  const fn = name.split("::").at(-1);
  return new RegExp(`(?:^|\\n)[ \\t]*(?:pub(?:\\([a-z]+\\))?[ \\t]+)?(?:async[ \\t]+)?fn[ \\t]+${escapeRegExp(fn)}[ \\t]*\\(`, "u").test(text);
}

// --- The receipt -----------------------------------------------------------

function entryCounts(entries) {
  return {
    total: entries.length,
    executed: entries.filter((entry) => entry.status === "executed").length,
    passed: entries.filter((entry) => entry.result === "pass").length,
    failed: entries.filter((entry) => entry.result === "fail").length,
    source_only: entries.filter((entry) => entry.status === "source_only").length,
    not_run: entries.filter((entry) => entry.status === "not_run").length,
  };
}

/// The receipt document, from the manifest's text, the product version and
/// commit, the result files by name (`<leg>.json` to bytes), and a reader of
/// source files at the product commit (path to bytes, or null).
export function buildReceipt({ manifestText, manifestName = basename(MANIFEST_PATH), version, commit, results, readSource, host }) {
  const { inputs, receipt: receiptSchema } = schemas();
  const manifest = parseJson(manifestText, "the row manifest");
  assertValid(inputs, "manifest", manifest, "the row manifest");
  const { cells } = checkManifest(manifest);

  const expected = new Set(manifest.legs.map((leg) => `${leg.id}.json`));
  for (const name of [...results.keys()].sort()) {
    if (!expected.has(name)) throw new ReceiptError(`${name} is the result of no leg of the manifest`);
  }
  const rowsById = new Map(manifest.rows.map((row) => [row.id, row]));
  const states = new Map();
  for (const leg of manifest.legs) {
    const name = `${leg.id}.json`;
    const bytes = results.get(name);
    if (bytes === undefined) {
      throw new ReceiptError(`leg ${leg.id} has no result file ${name}: record its run, or record that it did not run`);
    }
    const result = parseJson(Buffer.from(bytes).toString("utf8"), `the result file ${name}`);
    assertValid(inputs, "leg_result", result, `the result file ${name}`);
    if (result.leg !== leg.id) throw new ReceiptError(`the result file ${name} is the result of leg ${result.leg}`);
    checkLegResult(result, `the result file ${name}`);
    assertPublishable(result, host, `the result file ${name}`);
    for (const item of result.observations ?? []) {
      const row = rowsById.get(item.row);
      if (row === undefined || row.leg !== leg.id || row.cells !== "observed") {
        throw new ReceiptError(`the result file ${name} holds an observation for ${item.row}, which is no row of this leg with observed cells`);
      }
      if (row.test === null) throw new ReceiptError(`the result file ${name} holds an observation for ${item.row}, and the manifest names no test for that row`);
    }
    const ran = result.status === "run";
    const atCommit = ran && result.commit === commit;
    // What the rows printed of their cells, by entry. Only a run at the
    // product commit is held to the manifest: every line must be of a cell
    // the manifest lists, or the manifest is behind the rows; and a cell has
    // one line, unless its row has an entry for each level it asks for.
    const picked = new Map();
    for (const line of atCommit ? [...result.cell_lines].sort(byCellLine) : []) {
      const owners = manifest.rows.filter((row) => row.leg === leg.id && row.cells !== "observed" && row.test?.name === line.test);
      if (owners.length !== 1 || !owners[0].cells.some((item) => item.cell === line.cell)) {
        throw new ReceiptError(`the result file ${name} holds a cell line of ${line.test}, cell ${line.cell}, which is not a cell of exactly one row of this leg in the manifest`);
      }
      const key = `${owners[0].id}/${line.cell}`;
      const lines = picked.get(key) ?? [];
      if (lines.length > 0 && owners[0].lines !== "per_level") {
        throw new ReceiptError(`the result file ${name} holds two cell lines of ${line.test}, cell ${line.cell}, and row ${owners[0].id} of the manifest has one entry for a cell`);
      }
      picked.set(key, [...lines, line]);
    }
    states.set(leg.id, { result, ran, atCommit, picked, file: { name, sha256: sha256(bytes) } });
  }

  const sources = new Map();
  const testOf = (row) => {
    if (row.test === null) return null;
    if (!sources.has(row.test.source)) {
      const bytes = readSource(row.test.source);
      if (bytes === null) throw new ReceiptError(`row ${row.id} of the manifest names ${row.test.source}, which the commit does not have`);
      sources.set(row.test.source, { text: Buffer.from(bytes).toString("utf8"), sha256: sha256(bytes) });
    }
    const source = sources.get(row.test.source);
    if (!definesTest(source.text, row.test.name)) {
      throw new ReceiptError(`row ${row.id} of the manifest names the test ${row.test.name}, which ${row.test.source} does not define at the commit`);
    }
    return { package: row.test.package, target: row.test.target, name: row.test.name, source: row.test.source, source_sha256: source.sha256 };
  };

  const entries = [];
  for (const row of manifest.rows) {
    const leg = manifest.legs.find((item) => item.id === row.leg);
    const state = states.get(row.leg);
    const test = testOf(row);
    // What the row as a whole comes to when no result of its own exists.
    const unexecuted =
      test === null
        ? { status: "not_run", gap: "no_test_row" }
        : !state.ran
          ? { status: "source_only", gap: "leg_not_run" }
          : !state.atCommit
            ? { status: "source_only", gap: "ran_at_another_commit" }
            : null;
    const entry = (fields) => ({
      id: fields.id,
      row: row.id,
      cell: fields.cell,
      title: row.title,
      provider_family: fields.provider_family,
      model: fields.model,
      api: fields.api,
      route: fields.route,
      requested_level: fields.requested_level,
      outcome: fields.outcome,
      outcome_basis: fields.outcome_basis,
      values_basis: fields.values_basis ?? null,
      status: fields.status,
      mode: leg.mode,
      result: fields.result ?? null,
      tries: fields.tries ?? null,
      failure: fields.failure ?? null,
      leg: leg.id,
      result_file: fields.status === "executed" ? state.file : null,
      test,
      gap: fields.gap,
    });

    if (row.cells === "observed") {
      const seen = unexecuted === null ? state.result.observations.filter((item) => item.row === row.id) : [];
      if (seen.length === 0) {
        entries.push(
          entry({
            id: `${row.id}/-`,
            cell: null,
            provider_family: null,
            model: null,
            api: null,
            route: null,
            requested_level: null,
            outcome: null,
            outcome_basis: null,
            ...(unexecuted ?? { status: "source_only", gap: "no_cells_observed" }),
          }),
        );
        continue;
      }
      [...seen]
        .sort((a, b) => compare(observationKey(a), observationKey(b)))
        .forEach((item, index) => {
          const executed = item.result !== "skip";
          entries.push(
            entry({
              id: `${row.id}/#${index + 1}`,
              cell: null,
              provider_family: item.provider_family,
              model: { id: item.model },
              api: item.api,
              route: `${item.backend}/${item.api}`,
              requested_level: { level: item.requested_level },
              outcome: item.outcome.kind === "sent" ? { kind: "sent", level: { level: item.outcome.level } } : { kind: "refused", reason: item.outcome.reason },
              outcome_basis: "observation",
              values_basis: "observation",
              status: executed ? "executed" : "not_run",
              result: item.result,
              tries: executed ? 1 : null,
              // Typed fields the row emitted, and only on a failure. A leg
              // result has no member a message could arrive in.
              failure:
                item.result === "fail" && (item.http_status !== undefined || item.error_code !== undefined)
                  ? { http_status: item.http_status ?? null, error_code: item.error_code ?? null }
                  : null,
              gap: executed ? null : "cell_skipped",
            }),
          );
        });
      continue;
    }

    let own = unexecuted;
    if (own === null) {
      const hit = state.result.tests.find((item) => item.package === test.package && item.target === test.target && item.name === test.name);
      if (hit === undefined) {
        own = { status: "source_only", gap: state.result.tests_complete ? "test_not_in_leg_result" : "no_per_test_result" };
      } else if (hit.outcome === "skip") {
        own = { status: "source_only", gap: "test_skipped" };
      } else {
        own = { status: "executed", result: hit.outcome, tries: hit.tries, gap: null };
      }
    }
    for (const item of row.cells) {
      const cell = cells.get(item.cell);
      const route = `${cell.backend}/${cell.api}`;
      const fixedModel = item.model ?? row.model;
      const fixedLevel = item.requested_level ?? row.requested_level;
      const declared = (fields) =>
        entry({
          cell: cell.id,
          provider_family: cell.provider_family,
          api: cell.api,
          route,
          outcome: row.outcome.kind === "sent" ? { kind: "sent", level: fields.requested_level } : { kind: "refused", reason: row.outcome.reason },
          outcome_basis: "test_assertion",
          ...fields,
          ...own,
        });
      // The row's own lines for this cell, when the row ran here: the values
      // it picked replace the manifest's rule, and must agree with whatever
      // the manifest fixes. A row with an entry for each level has one line,
      // and so one entry, per level, lowest level first.
      const lines = own.status === "executed" ? (state.picked.get(`${row.id}/${cell.id}`) ?? []) : [];
      if (lines.length === 0) {
        entries.push(declared({ id: `${row.id}/${cell.id}`, model: fixedModel, requested_level: fixedLevel, values_basis: fixedModel === null ? null : "manifest" }));
        continue;
      }
      for (const line of lines) {
        const differs = (what, said, fixed) => new ReceiptError(`row ${row.id} reports the ${what} ${said} for the cell ${cell.id}, and the manifest says ${fixed}`);
        if (line.route !== route) throw differs("route", line.route, route);
        if (fixedModel.id !== undefined && fixedModel.id !== line.model) throw differs("model", line.model, fixedModel.id);
        if (fixedLevel.level !== undefined && fixedLevel.level !== line.level) throw differs("level", line.level, fixedLevel.level);
        entries.push(
          declared({
            id: row.lines === "per_level" ? `${row.id}/${cell.id}:${line.level}` : `${row.id}/${cell.id}`,
            model: { id: line.model },
            requested_level: { level: line.level },
            values_basis: "row_output",
          }),
        );
      }
    }
  }

  const legs = manifest.legs.map((leg) => {
    const { result, atCommit, file } = states.get(leg.id);
    if (result.status === "not_run") {
      return { id: leg.id, mode: leg.mode, title: leg.title, status: "not_run", reason: result.reason, result_file: file };
    }
    return {
      id: leg.id,
      mode: leg.mode,
      title: leg.title,
      status: "run",
      commit: result.commit,
      at_product_commit: atCommit,
      command: result.command,
      exit_status: result.exit_status,
      counts: result.counts,
      per_test_results: result.tests_complete ? "complete" : result.tests.length > 0 ? "partial" : "none",
      output: result.output,
      result_file: file,
    };
  });

  const gaps = [
    ...legs
      .filter((leg) => leg.status === "not_run" || !leg.at_product_commit)
      .map((leg) => ({ scope: "leg", id: leg.id, reason: leg.status === "not_run" ? leg.reason : "ran_at_another_commit" })),
    ...entries.filter((item) => item.gap !== null).map((item) => ({ scope: "entry", id: item.id, reason: item.gap })),
  ];
  const counted = legs.filter((leg) => leg.status === "run" && leg.at_product_commit);
  const sum = (key) => counted.reduce((total, leg) => total + leg.counts[key], 0);

  const receipt = {
    schema_version: 1,
    kind: "reasoning_effort_execution_receipt",
    product: { name: "meerkat", version, commit },
    manifest: { name: manifestName, sha256: sha256(Buffer.from(manifestText)) },
    totals: {
      entries: entryCounts(entries),
      leg_tests: { run: sum("run"), passed: sum("passed"), failed: sum("failed"), skipped: sum("skipped") },
      by_mode: MODES.map((mode) => ({ mode, ...entryCounts(entries.filter((item) => item.mode === mode)) })),
      gaps: gaps.length,
    },
    legs,
    entries,
    gaps,
    rules: manifest.rules.map((rule) => ({ id: rule.id, text: rule.text })),
  };
  assertValid(receiptSchema, null, receipt, "the receipt");
  assertPublishable(receipt, host, "the receipt");
  return receipt;
}

// --- The readable rendering ------------------------------------------------

const MODE_TEXT = { in_process: "in process", built_binary: "built binary", live: "live" };
const GAP_TEXT = {
  rows_not_built: "its rows are not built",
  not_scheduled: "it was not scheduled",
  credential_unavailable: "no credential was available",
  request_budget_not_agreed: "no request budget was agreed",
  stopped_on_provider_status: "it stopped on a provider status that ends a live run",
  infrastructure_failure: "the run failed before it reported tests",
  leg_not_run: "its leg did not run",
  no_test_row: "no test covers it at this commit",
  ran_at_another_commit: "its leg ran at another commit",
  no_per_test_result: "its leg ran and reported counts only, not this test",
  test_not_in_leg_result: "its leg ran and did not run this test",
  test_skipped: "its leg skipped the test",
  no_cells_observed: "its leg ran and recorded no cell",
  cell_skipped: "its leg saw the cell and skipped it",
};

function code(text) {
  const longest = Math.max(0, ...[...text.matchAll(/`+/gu)].map((match) => match[0].length));
  const fence = "`".repeat(longest + 1);
  return longest === 0 ? `\`${text}\`` : `${fence} ${text} ${fence}`;
}

function cellText(text) {
  return String(text).replaceAll("|", "\\|");
}

function table(header, rows) {
  return [
    `| ${header.join(" | ")} |`,
    `| ${header.map(() => "---").join(" | ")} |`,
    ...rows.map((row) => `| ${row.map(cellText).join(" | ")} |`),
  ];
}

function modelText(model) {
  if (model === null) return "-";
  return model.id !== undefined ? code(model.id) : `by rule ${code(model.rule)}`;
}

function levelText(level) {
  if (level === null) return "-";
  return level.level !== undefined ? code(level.level) : `by rule ${code(level.rule)}`;
}

function outcomeText(outcome) {
  if (outcome === null) return "-";
  if (outcome.kind === "refused") return `refused: ${code(outcome.reason)}`;
  return outcome.level === null ? "sent" : `sent: ${levelText(outcome.level)}`;
}

function stateText(entry) {
  if (entry.status === "executed") return `executed, ${MODE_TEXT[entry.mode]}`;
  return `${entry.status === "source_only" ? "source only" : "not run"}: ${GAP_TEXT[entry.gap]}`;
}

function resultText(entry) {
  if (entry.result === null) return "-";
  const typed = [];
  if (entry.tries > 1) typed.push(`try ${entry.tries}`);
  if (entry.failure?.http_status != null) typed.push(`HTTP ${entry.failure.http_status}`);
  if (entry.failure?.error_code != null) typed.push(`code ${code(entry.failure.error_code)}`);
  return typed.length === 0 ? entry.result : `${entry.result} (${typed.join(", ")})`;
}

/// What `--check` holds against a receipt: every failed entry, and every
/// gap of the legs that can always run (in process and built binary: no
/// credential, no network). A gap of a live leg is listed in the receipt
/// and never fails the check.
export function checkReceipt(receipt) {
  const offlineLegs = new Set(receipt.legs.filter((leg) => OFFLINE_MODES.includes(leg.mode)).map((leg) => leg.id));
  const entries = new Map(receipt.entries.map((entry) => [entry.id, entry]));
  const problems = receipt.entries.filter((entry) => entry.result === "fail").map((entry) => `entry ${entry.id} failed`);
  for (const gap of receipt.gaps) {
    const leg = gap.scope === "leg" ? gap.id : entries.get(gap.id)?.leg;
    if (leg === undefined) problems.push(`the gap of ${gap.scope} ${gap.id} names no entry of the receipt`);
    else if (offlineLegs.has(leg)) problems.push(`${gap.scope} ${gap.id} is a gap of the offline legs: ${gap.reason}`);
  }
  return problems;
}

/// The Markdown rendering of a receipt document, from that document alone.
export function renderMarkdown(receipt) {
  const { product, totals } = receipt;
  const lines = [
    "# Reasoning effort execution receipt",
    "",
    `- Product: ${product.name} ${product.version}`,
    `- Commit: ${code(product.commit)}`,
    `- Receipt schema version: ${receipt.schema_version}`,
    `- Row manifest: ${code(receipt.manifest.name)}, sha256 ${code(receipt.manifest.sha256)}`,
    "",
    "What ran for the provider-neutral reasoning effort level at this commit, and what did not.",
    "This file is rendered from the JSON receipt beside it, which is the record: every executed",
    "entry there names the result file it was read from and that file's digest. An entry is",
    "executed only when its leg ran at this commit and reported the entry's own result; an entry",
    "whose test is in the source without such a result is source only.",
    "",
    "## Totals",
    "",
    ...table(
      ["Mode", "Entries", "Executed", "Passed", "Failed", "Source only", "Not run"],
      [
        ...totals.by_mode.map((row) => [MODE_TEXT[row.mode], row.total, row.executed, row.passed, row.failed, row.source_only, row.not_run]),
        ["all", totals.entries.total, totals.entries.executed, totals.entries.passed, totals.entries.failed, totals.entries.source_only, totals.entries.not_run],
      ],
    ),
    "",
    `Tests the legs ran at this commit: ${totals.leg_tests.run} run, ${totals.leg_tests.passed} passed, ${totals.leg_tests.failed} failed, ${totals.leg_tests.skipped} skipped. Gaps: ${totals.gaps}.`,
    "",
    "## Legs",
    "",
    ...table(
      ["Leg", "Mode", "State", "Commit", "Exit status", "Run", "Passed", "Failed", "Skipped", "Per-test results", "Result file"],
      receipt.legs.map((leg) => {
        const file = `${code(leg.result_file.name)} ${code(leg.result_file.sha256.slice(0, 12))}`;
        if (leg.status === "not_run") {
          const state = leg.reason === "ran_at_another_commit" ? "not run here: it ran at another commit" : `not run: ${GAP_TEXT[leg.reason]}`;
          return [code(leg.id), MODE_TEXT[leg.mode], state, "-", "-", "-", "-", "-", "-", "-", file];
        }
        return [
          code(leg.id),
          MODE_TEXT[leg.mode],
          leg.at_product_commit ? "ran at this commit" : "ran at another commit",
          code(leg.commit.slice(0, 10)),
          leg.exit_status,
          leg.counts.run,
          leg.counts.passed,
          leg.counts.failed,
          leg.counts.skipped,
          leg.per_test_results,
          file,
        ];
      }),
    ),
    "",
  ];
  for (const leg of receipt.legs) {
    lines.push(`- ${code(leg.id)}: ${leg.title}`);
    if (leg.status === "run") lines.push(`  - Command: ${code(leg.command)}`);
  }
  lines.push("", "## Matrix", "");
  const rows = [];
  for (const entry of receipt.entries) {
    if (!rows.some((row) => row.id === entry.row)) rows.push({ id: entry.row, title: entry.title, test: entry.test });
  }
  for (const row of rows) {
    const test = row.test === null ? "no test at this commit" : `test ${code(row.test.name)} in ${code(row.test.source)}`;
    lines.push(`- ${code(row.id)}: ${row.title} (${test})`);
  }
  lines.push(
    "",
    ...table(
      ["Entry", "Provider family", "Model", "API", "Route", "Requested level", "Outcome", "State", "Result"],
      receipt.entries.map((entry) => [
        code(entry.id),
        entry.provider_family ?? "-",
        modelText(entry.model),
        entry.api ?? "-",
        entry.route === null ? "-" : code(entry.route),
        levelText(entry.requested_level),
        outcomeText(entry.outcome),
        stateText(entry),
        resultText(entry),
      ]),
    ),
    "",
    "A model or a level shown as a value is fixed by the row's fixture or was printed by the row",
    "when it ran here; one shown as a rule is picked by the row when it runs, and no line of that",
    "pick was recorded.",
    "",
    "## Gaps",
    "",
  );
  if (receipt.gaps.length === 0) {
    lines.push("None.");
  } else {
    for (const gap of receipt.gaps.filter((item) => item.scope === "leg")) {
      lines.push(`- Leg ${code(gap.id)}: ${gap.reason === "ran_at_another_commit" ? "it ran at another commit" : `not run, ${GAP_TEXT[gap.reason]}`}.`);
    }
    const grouped = new Map();
    for (const gap of receipt.gaps.filter((item) => item.scope === "entry")) {
      const key = `${gap.id.slice(0, gap.id.indexOf("/"))}\n${gap.reason}`;
      grouped.set(key, (grouped.get(key) ?? 0) + 1);
    }
    for (const [key, count] of grouped) {
      const [row, reason] = key.split("\n");
      lines.push(`- Row ${code(row)}: ${count} ${count === 1 ? "entry" : "entries"}, ${GAP_TEXT[reason]}.`);
    }
  }
  if (receipt.rules.length > 0) {
    lines.push("", "## Selection rules", "", "Where a row picks its model or level when it runs, the matrix names the rule:", "");
    for (const rule of receipt.rules) lines.push(`- ${code(rule.id)}: ${rule.text}`);
  }
  return `${lines.join("\n")}\n`;
}

/// The two files of a receipt: the JSON text, and the Markdown rendered
/// from that text read back.
export function generate(input) {
  const json = serialize(buildReceipt(input));
  return { json, markdown: renderMarkdown(JSON.parse(json)) };
}

/// The problems of a written receipt: its schema, what it may publish, its
/// Markdown file against the rendering, and its result files' digests.
export function verifyReceipt({ jsonText, markdownText, results, host }) {
  const problems = [];
  let receipt;
  try {
    receipt = parseJson(jsonText, "the receipt");
    assertValid(schemas().receipt, null, receipt, "the receipt");
    assertPublishable(receipt, host, "the receipt");
  } catch (error) {
    if (!(error instanceof ReceiptError)) throw error;
    return [error.message];
  }
  if (serialize(receipt) !== jsonText) problems.push("the receipt is not written in the generator's form");
  if (markdownText !== undefined && markdownText !== renderMarkdown(receipt)) {
    problems.push("the Markdown file is not the rendering of the receipt");
  }
  if (results !== undefined) {
    for (const leg of receipt.legs) {
      const bytes = results.get(leg.result_file.name);
      if (bytes === undefined) problems.push(`the result file ${leg.result_file.name} is missing`);
      else if (sha256(bytes) !== leg.result_file.sha256) problems.push(`the result file ${leg.result_file.name} has another digest than the receipt names`);
    }
  }
  return problems;
}

// --- Command line ----------------------------------------------------------

/// `version` of `[workspace.package]` in a root Cargo.toml.
export function workspaceVersion(cargoToml) {
  let inside = false;
  for (const line of cargoToml.split("\n")) {
    const header = /^\s*\[([^\]]+)\]\s*(?:#.*)?$/u.exec(line);
    if (header !== null) {
      inside = header[1].trim() === "workspace.package";
      continue;
    }
    const version = inside ? /^\s*version\s*=\s*"([^"]+)"\s*(?:#.*)?$/u.exec(line) : null;
    if (version !== null) return version[1];
  }
  throw new ReceiptError("Cargo.toml at the commit has no [workspace.package] version");
}

function git(repo, args) {
  const result = spawnSync("git", ["-C", repo, ...args], { maxBuffer: 256 * 1024 * 1024 });
  return result.status === 0 ? result.stdout : null;
}

function readFile(path, what) {
  try {
    return readFileSync(path);
  } catch {
    throw new ReceiptError(`${what} cannot be read`);
  }
}

/// The result files of a directory by name. Anything in it that is not a
/// `.json` file is refused: the directory holds the leg results and nothing
/// the receipt would silently leave out.
export function readResults(directory) {
  let names;
  try {
    names = readdirSync(directory).sort();
  } catch {
    throw new ReceiptError("the results directory cannot be read");
  }
  const results = new Map();
  for (const name of names) {
    const path = join(directory, name);
    if (!name.endsWith(".json") || !statSync(path).isFile()) throw new ReceiptError(`${name} in the results directory is not a leg result file`);
    results.set(name, readFile(path, `the result file ${name}`));
  }
  return results;
}

function options(args, known, flags = []) {
  const values = new Map();
  const positional = [];
  const set = new Set();
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith("--")) {
      positional.push(arg);
    } else if (flags.includes(arg) && !set.has(arg)) {
      set.add(arg);
    } else if (!known.includes(arg) || index + 1 >= args.length || values.has(arg)) {
      return null;
    } else {
      values.set(arg, args[(index += 1)]);
    }
  }
  return { values, positional, flags: set };
}

/// Runs `--check` over a receipt and prints what it found. Exit status 3
/// when it holds something against the receipt.
function checkCommand(receipt) {
  const problems = checkReceipt(receipt);
  for (const problem of problems) console.log(`check: ${problem}`);
  if (problems.length === 0) {
    console.log("check passed: no failed entry and no gap in the offline legs");
    return 0;
  }
  console.log(`check failed: ${problems.length} failed entries or gaps of the offline legs`);
  return 3;
}

function usage() {
  console.error(
    [
      "usage: reasoning-effort-receipt.mjs record --leg ID --commit SHA --command TEXT --exit-status N",
      "           (--nextest-json FILE | --nextest-log FILE) [--observations FILE] --output FILE",
      "       reasoning-effort-receipt.mjs record --leg ID --not-run REASON --output FILE",
      "       reasoning-effort-receipt.mjs generate --results DIR --output-dir DIR [--commit REV] [--manifest FILE] [--repo DIR] [--check]",
      "       reasoning-effort-receipt.mjs render RECEIPT.json",
      "       reasoning-effort-receipt.mjs verify RECEIPT.json [--markdown FILE] [--results DIR] [--check]",
    ].join("\n"),
  );
  return 2;
}

function record(args) {
  const parsed = options(args, ["--leg", "--commit", "--command", "--exit-status", "--nextest-json", "--nextest-log", "--observations", "--not-run", "--output"]);
  if (parsed === null || parsed.positional.length > 0) return usage();
  const value = (name) => parsed.values.get(name);
  const leg = value("--leg");
  const output = value("--output");
  if (leg === undefined || output === undefined) return usage();
  let result;
  if (value("--not-run") !== undefined) {
    if (parsed.values.size !== 3) return usage();
    result = buildNotRunResult({ leg, reason: value("--not-run") });
  } else {
    const json = value("--nextest-json");
    const log = value("--nextest-log");
    const exitStatus = value("--exit-status");
    if ([value("--commit"), value("--command"), exitStatus].includes(undefined) || (json === undefined) === (log === undefined)) return usage();
    if (!/^(?:0|[1-9][0-9]{0,2})$/u.test(exitStatus)) return usage();
    const observations = value("--observations") === undefined ? [] : parseJson(readFile(value("--observations"), "the observations file").toString("utf8"), "the observations file");
    if (!Array.isArray(observations)) throw new ReceiptError("the observations file is not a list");
    result = buildLegResult({
      leg,
      commit: value("--commit"),
      command: value("--command"),
      exitStatus: Number(exitStatus),
      format: json !== undefined ? "nextest_libtest_json_plus" : "nextest_human_log",
      output: readFile(json ?? log, "the run's output"),
      observations,
      host: hostFacts(),
    });
  }
  writeFileSync(output, serialize(result));
  console.log(`recorded leg ${leg}: ${result.status === "run" ? `${result.counts.run} run, ${result.counts.passed} passed, ${result.counts.failed} failed, ${result.counts.skipped} skipped, ${result.tests.length} named, ${result.cell_lines.length} cell lines, ${result.observations.length} observations` : `not run (${result.reason})`}`);
  return 0;
}

function generateCommand(args) {
  const parsed = options(args, ["--results", "--output-dir", "--commit", "--manifest", "--repo"], ["--check"]);
  if (parsed === null || parsed.positional.length > 0) return usage();
  const resultsDirectory = parsed.values.get("--results");
  const outputDirectory = parsed.values.get("--output-dir");
  if (resultsDirectory === undefined || outputDirectory === undefined) return usage();
  const repo = resolve(parsed.values.get("--repo") ?? join(here, ".."));
  const revision = parsed.values.get("--commit") ?? "HEAD";
  const commit = git(repo, ["rev-parse", "--verify", "--quiet", `${revision}^{commit}`])?.toString("utf8").trim();
  if (commit === undefined) throw new ReceiptError(`${revision} names no commit of the repository`);
  const show = (path) => git(repo, ["show", `${commit}:${path}`]);
  const cargoToml = show("Cargo.toml");
  if (cargoToml === null) throw new ReceiptError("the commit has no Cargo.toml");
  const manifestFile = parsed.values.get("--manifest");
  const manifestBytes = manifestFile === undefined ? show(MANIFEST_PATH) : readFile(manifestFile, "the row manifest");
  if (manifestBytes === null) throw new ReceiptError(`the commit has no ${MANIFEST_PATH}`);
  const { json, markdown } = generate({
    manifestText: manifestBytes.toString("utf8"),
    manifestName: basename(manifestFile ?? MANIFEST_PATH),
    version: workspaceVersion(cargoToml.toString("utf8")),
    commit,
    results: readResults(resultsDirectory),
    readSource: show,
    host: hostFacts(),
  });
  mkdirSync(outputDirectory, { recursive: true });
  writeFileSync(join(outputDirectory, RECEIPT_JSON_NAME), json);
  writeFileSync(join(outputDirectory, RECEIPT_MARKDOWN_NAME), markdown);
  const receipt = JSON.parse(json);
  const { totals } = receipt;
  console.log(
    `wrote ${RECEIPT_JSON_NAME} and ${RECEIPT_MARKDOWN_NAME}: ${totals.entries.total} entries, ${totals.entries.executed} executed (${totals.entries.passed} passed, ${totals.entries.failed} failed), ${totals.entries.source_only} source only, ${totals.entries.not_run} not run, ${totals.gaps} gaps`,
  );
  // The receipt is written whatever the check says: it is the same truthful
  // document, and the check only judges it.
  return parsed.flags.has("--check") ? checkCommand(receipt) : 0;
}

function renderCommand(args) {
  if (args.length !== 1 || args[0].startsWith("--")) return usage();
  const receipt = parseJson(readFile(args[0], "the receipt").toString("utf8"), "the receipt");
  assertValid(schemas().receipt, null, receipt, "the receipt");
  process.stdout.write(renderMarkdown(receipt));
  return 0;
}

function verifyCommand(args) {
  const parsed = options(args, ["--markdown", "--results"], ["--check"]);
  if (parsed === null || parsed.positional.length !== 1) return usage();
  const markdown = parsed.values.get("--markdown");
  const results = parsed.values.get("--results");
  const jsonText = readFile(parsed.positional[0], "the receipt").toString("utf8");
  const problems = verifyReceipt({
    jsonText,
    markdownText: markdown === undefined ? undefined : readFile(markdown, "the Markdown file").toString("utf8"),
    results: results === undefined ? undefined : readResults(results),
    host: hostFacts(),
  });
  for (const problem of problems) console.log(`refused: ${problem}`);
  if (problems.length > 0) return 1;
  console.log("the receipt holds");
  return parsed.flags.has("--check") ? checkCommand(JSON.parse(jsonText)) : 0;
}

export function main(args) {
  const [command, ...rest] = args;
  const run = { record, generate: generateCommand, render: renderCommand, verify: verifyCommand }[command];
  if (run === undefined) return usage();
  try {
    return run(rest);
  } catch (error) {
    if (!(error instanceof ReceiptError)) throw error;
    console.log(`refused: ${error.message}`);
    return 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  process.exit(main(process.argv.slice(2)));
}
