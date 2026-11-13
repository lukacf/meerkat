#!/usr/bin/env node
// Self-test for scripts/reasoning-effort-receipt.mjs: the generator of the
// reasoning effort execution receipt. Each row below is one contract of the
// generator; all rows run, and every failed one is printed. No network; the
// command-line rows use scratch directories and a scratch git repository.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  CELL_LINE_PREFIX,
  LEVELS,
  MANIFEST_PATH,
  MODES,
  OBSERVATION_LINE_PREFIX,
  RECEIPT_JSON_NAME,
  RECEIPT_MARKDOWN_NAME,
  ReceiptError,
  WITHHELD_MODEL_ID,
  assertPublishable,
  buildLegResult,
  buildNotRunResult,
  buildReceipt,
  checkReceipt,
  definesTest,
  generate,
  liveGaps,
  parseNextestJson,
  parseNextestLog,
  renderMarkdown,
  schemaErrors,
  schemas,
  serialize,
  sha256,
  verifyReceipt,
  workspaceVersion,
} from "./reasoning-effort-receipt.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const script = join(here, "reasoning-effort-receipt.mjs");

// --- Fixtures --------------------------------------------------------------

const COMMIT = "1f".repeat(20);
const OTHER_COMMIT = "2e".repeat(20);
const HOST = { hostname: "build-host-17.example", home: "/home/operator" };
// What a failed test prints. No part of it may reach a leg result or a receipt.
const CAPTURED = "provider answered 429: quota exceeded for the account octo-org";
const ROUTES_SOURCE = "crates/demo-facade/tests/effort_routes.rs";
const TABLE_SOURCE = "crates/demo-core/src/resolve.rs";
const COMMAND = "./scripts/repo-cargo nextest run -p demo-facade --test effort_routes --message-format libtest-json-plus -E 'test(/level/) | test(=live_matrix)'";

const SOURCES = new Map([
  [
    ROUTES_SOURCE,
    [
      "#[tokio::test]",
      "async fn supported_level_is_sent() {}",
      "#[tokio::test]",
      "async fn unsupported_level_is_refused() {}",
      "#[test]",
      "fn resumed_level_is_sent() {}",
      "#[test]",
      "#[ignore]",
      "fn live_matrix() {}",
      "",
    ].join("\n"),
  ],
  [TABLE_SOURCE, ["mod tests {", "    #[test]", "    fn table_is_total() {}", "}", ""].join("\n")],
]);

function routesTest(name) {
  return { package: "demo-facade", target: "effort_routes", name, source: ROUTES_SOURCE };
}

function manifest() {
  return {
    schema_version: 1,
    kind: "reasoning_effort_receipt_manifest",
    rules: [
      { id: "catalog_model", text: "A model of the catalog that states levels." },
      { id: "accepted_level", text: "A level the route accepts." },
      { id: "refused_levels", text: "Every level the row refuses on the route." },
    ],
    cells: [
      { id: "direct", provider_family: "openai", backend: "openai_api", api: "responses" },
      { id: "copilot", provider_family: "openai", backend: "copilot", api: "responses" },
    ],
    legs: [
      { id: "routes", mode: "in_process", title: "The route rows against a scripted server." },
      { id: "binary", mode: "built_binary", title: "The built binary against a scripted server." },
      { id: "live", mode: "live", title: "The live rows of a real account." },
    ],
    rows: [
      {
        id: "routes.supported",
        leg: "routes",
        title: "An accepted level is sent.",
        test: routesTest("supported_level_is_sent"),
        outcome: { kind: "sent" },
        model: { rule: "catalog_model" },
        requested_level: { level: "high" },
        cells: [{ cell: "direct" }, { cell: "copilot", model: { id: "fixture-model-1" }, requested_level: { level: "low" } }],
      },
      {
        id: "routes.unsupported",
        leg: "routes",
        title: "Each level outside the set is refused.",
        lines: "per_level",
        test: routesTest("unsupported_level_is_refused"),
        outcome: { kind: "refused", reason: "unsupported_level" },
        model: { rule: "catalog_model" },
        requested_level: { rule: "refused_levels" },
        cells: [{ cell: "direct" }, { cell: "copilot" }],
      },
      {
        id: "routes.resumed",
        leg: "routes",
        title: "A resumed session still sends its level.",
        test: routesTest("resumed_level_is_sent"),
        outcome: { kind: "sent" },
        model: { rule: "catalog_model" },
        requested_level: { rule: "accepted_level" },
        cells: [{ cell: "direct" }],
      },
      {
        id: "routes.table",
        leg: "routes",
        title: "The translation table covers the route.",
        test: { package: "demo-core", target: "lib", name: "resolve::tests::table_is_total", source: TABLE_SOURCE },
        outcome: { kind: "sent" },
        model: { rule: "catalog_model" },
        requested_level: { level: "high" },
        cells: [{ cell: "direct" }],
      },
      {
        id: "binary.supported",
        leg: "binary",
        title: "The built binary sends an accepted level.",
        test: null,
        outcome: { kind: "sent" },
        model: null,
        requested_level: null,
        cells: [{ cell: "direct" }],
      },
      { id: "live.matrix", leg: "live", title: "Every model the account lists.", test: routesTest("live_matrix"), cells: "observed" },
      { id: "live.cli", leg: "live", title: "The built binary on the account.", test: null, cells: "observed" },
    ],
  };
}

function suite(event, crate, binary, kind) {
  return { type: "suite", event, test_count: 1, nextest: { crate, test_binary: binary, kind } };
}

function testEvent(event, name, extra = {}) {
  return { type: "test", event, name, ...extra };
}

function cellLine(test, cell, model, level, route) {
  return `${CELL_LINE_PREFIX}${JSON.stringify({ test, cell, model, level, route })}`;
}

/// A `libtest-json-plus` run of the route rows with `--no-capture`: a pass,
/// a failure with captured output, an ignored test, a pass on the second
/// try, and a library test of another crate. The rows' cell lines are whole
/// lines between the events; the failed test's line is in its output. `also`
/// is more whole lines the rows print.
function routesJson({ cellLines = true, also = [] } = {}) {
  const printed = cellLines
    ? [
        cellLine("supported_level_is_sent", "direct", "catalog-model-9", "high", "openai_api/responses"),
        cellLine("supported_level_is_sent", "copilot", "fixture-model-1", "low", "copilot/responses"),
        cellLine("resumed_level_is_sent", "direct", "catalog-model-9", "low", "openai_api/responses"),
      ]
    : [];
  // The row that refuses prints a line for each level it refuses, here the higher one first.
  const failedOutput = cellLines
    ? `${cellLine("unsupported_level_is_refused", "direct", "catalog-model-9", "max", "openai_api/responses")}\n${cellLine("unsupported_level_is_refused", "direct", "catalog-model-9", "none", "openai_api/responses")}\n${CAPTURED}\n`
    : CAPTURED;
  return [
    suite("started", "demo-facade", "effort_routes", "test"),
    testEvent("started", "demo-facade::effort_routes$supported_level_is_sent"),
    testEvent("ignored", "demo-facade::effort_routes$live_matrix"),
    ...printed,
    ...also,
    testEvent("ok", "demo-facade::effort_routes$supported_level_is_sent", { exec_time: 0.2 }),
    testEvent("failed", "demo-facade::effort_routes$unsupported_level_is_refused", { exec_time: 0.1, stdout: failedOutput }),
    testEvent("ok", "demo-facade::effort_routes$resumed_level_is_sent#2", { exec_time: 0.1 }),
    { type: "suite", event: "failed", passed: 2, failed: 1, ignored: 1, nextest: { crate: "demo-facade", test_binary: "effort_routes", kind: "test" } },
    suite("started", "demo-core", "demo_core", "lib"),
    testEvent("ok", "demo-core::demo_core$resolve::tests::table_is_total", { exec_time: 0.01 }),
    suite("ok", "demo-core", "demo_core", "lib"),
  ]
    .map((event) => (typeof event === "string" ? event : JSON.stringify(event)))
    .join("\n");
}

/// The human log of a run that shows every status line, and with
/// `--success-output final` what a passing test printed.
function routesLog({ quiet = false } = {}) {
  const shown = quiet
    ? []
    : [
        "        PASS [   0.004s] (1/3) demo-core resolve::tests::table_is_total",
        "        FAIL [   0.004s] (2/3) demo-facade::effort_routes unsupported_level_is_refused",
        "  stdout ───",
        `    ${CAPTURED}`,
        "        PASS [   0.005s] (3/3) demo-facade::effort_routes supported_level_is_sent",
        "  stdout ───",
        "",
        "    running 1 test",
        `    ${cellLine("supported_level_is_sent", "direct", "catalog-model-9", "high", "openai_api/responses")}`,
        "    test supported_level_is_sent ... ok",
      ];
  return [
    "   Compiling demo-facade v1.4.2 (/home/operator/src/demo/crates/demo-facade)",
    "────────────",
    "    Starting 3 tests across 2 binaries (2 tests skipped)",
    ...shown,
    "────────────",
    "     Summary [   0.041s] 3 tests run: 2 passed, 1 failed, 2 skipped",
    ...(quiet ? [] : ["        FAIL [   0.004s] (2/3) demo-facade::effort_routes unsupported_level_is_refused"]),
    "error: test run failed",
    "",
  ].join("\n");
}

function routesResult(overrides = {}) {
  return buildLegResult({
    leg: "routes",
    commit: COMMIT,
    command: COMMAND,
    exitStatus: 100,
    format: "nextest_libtest_json_plus",
    output: Buffer.from(routesJson()),
    host: HOST,
    ...overrides,
  });
}

function observation(overrides = {}) {
  return {
    row: "live.matrix",
    provider_family: "openai",
    backend: "copilot",
    api: "responses",
    model: "listed-model-a",
    requested_level: "high",
    outcome: { kind: "sent", level: "high" },
    result: "pass",
    ...overrides,
  };
}

/// A result of the live leg that holds `observations`. `event` is what the
/// run says of the live row's own test (`ok`, `failed`, `ignored`); `null`
/// is a run that names another test only.
function liveResult(observations, commit = COMMIT, event = "ok") {
  const output = [suite("started", "demo-facade", "effort_routes", "test"), event === null ? testEvent("ok", "demo-facade::effort_routes$another_test") : testEvent(event, "demo-facade::effort_routes$live_matrix")]
    .map((item) => JSON.stringify(item))
    .join("\n");
  return buildLegResult({
    leg: "live",
    commit,
    command: "make e2e-copilot-live",
    exitStatus: event === "failed" ? 100 : 0,
    format: "nextest_libtest_json_plus",
    output: Buffer.from(output),
    observations,
    host: HOST,
  });
}

function file(document) {
  return Buffer.from(serialize(document));
}

/// The generator's whole input: the routes leg ran at the product commit,
/// the other two legs are recorded as not run.
function input(change = {}) {
  const results = new Map([
    ["routes.json", file(routesResult())],
    ["binary.json", file(buildNotRunResult({ leg: "binary", reason: "rows_not_built" }))],
    ["live.json", file(buildNotRunResult({ leg: "live", reason: "credential_unavailable" }))],
  ]);
  const base = {
    manifestText: serialize(manifest()),
    version: "1.4.2",
    commit: COMMIT,
    results,
    readSource: (path) => (SOURCES.has(path) ? Buffer.from(SOURCES.get(path)) : null),
    host: HOST,
  };
  return { ...base, ...change };
}

function entryOf(receipt, id) {
  const found = receipt.entries.filter((entry) => entry.id === id);
  assert.equal(found.length, 1, `one entry ${id}`);
  return found[0];
}

function refuses(run, pattern, message) {
  assert.throws(run, (error) => error instanceof ReceiptError && pattern.test(error.message), message);
}

function withResult(name, document) {
  const change = input();
  change.results.set(name, Buffer.isBuffer(document) ? document : file(document));
  return change;
}

function strings(value, into = []) {
  if (typeof value === "string") into.push(value);
  else if (Array.isArray(value)) value.forEach((item) => strings(item, into));
  else if (value !== null && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      into.push(key);
      strings(item, into);
    }
  }
  return into;
}

function memberNames(value, into = new Set()) {
  if (Array.isArray(value)) value.forEach((item) => memberNames(item, into));
  else if (value !== null && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      into.add(key);
      memberNames(item, into);
    }
  }
  return into;
}

/// The member names a schema allows: the keys of every `properties`.
function propertyNames(schema, into = new Set()) {
  if (Array.isArray(schema)) schema.forEach((item) => propertyNames(item, into));
  else if (schema !== null && typeof schema === "object") {
    for (const [key, item] of Object.entries(schema)) {
      if (key === "properties") Object.keys(item).forEach((name) => into.add(name));
      propertyNames(item, into);
    }
  }
  return into;
}

// A credential-shaped string, assembled here so that no scanner of this
// repository reads one in the file.
const CREDENTIAL = ["gh", "p_", "a1B2c3D4".repeat(5)].join("");

const scratch = [];
function scratchDirectory() {
  const directory = mkdtempSync(join(tmpdir(), "reasoning-effort-receipt-"));
  scratch.push(directory);
  return directory;
}

function cli(args, options = {}) {
  return spawnSync(process.execPath, [script, ...args], { encoding: "utf8", ...options });
}

// --- Rows ------------------------------------------------------------------

const rows = [];
function row(name, body) {
  rows.push({ name, body });
}

row("the reader of nextest's JSON output names every test and reads no captured output", () => {
  const parsed = parseNextestJson(routesJson());
  assert.deepEqual(parsed.counts, { run: 4, passed: 3, failed: 1, skipped: 1 });
  assert.equal(parsed.tests_complete, true);
  assert.deepEqual(
    parsed.tests.map((test) => [test.package, test.target, test.name, test.outcome, test.tries]),
    [
      ["demo-core", "lib", "resolve::tests::table_is_total", "pass", 1],
      ["demo-facade", "effort_routes", "live_matrix", "skip", 1],
      ["demo-facade", "effort_routes", "resumed_level_is_sent", "pass", 2],
      ["demo-facade", "effort_routes", "supported_level_is_sent", "pass", 1],
      ["demo-facade", "effort_routes", "unsupported_level_is_refused", "fail", 1],
    ],
    "a library test is the target lib; a pass on the second try keeps its try",
  );
  assert.ok(!JSON.stringify(parsed).includes("quota"), "captured output is not read");

  refuses(() => parseNextestJson(""), /no suite event/, "an empty output is no run");
  refuses(() => parseNextestJson("not json"), /no suite event/, "an output that holds no event");
  refuses(
    () => parseNextestJson(JSON.stringify({ type: "suite", event: "started", test_count: 1 })),
    /libtest-json-plus/,
    "the plain libtest-json form does not say which target a binary is",
  );
  const stray = [suite("started", "demo-facade", "effort_routes", "test"), testEvent("ok", "other-crate::rows$a_test")];
  refuses(() => parseNextestJson(stray.map((event) => JSON.stringify(event)).join("\n")), /no suite event describes/, "a test of an undescribed binary");
  const twice = [suite("started", "demo-facade", "effort_routes", "test"), testEvent("ok", "demo-facade::effort_routes$a"), testEvent("failed", "demo-facade::effort_routes$a")];
  refuses(() => parseNextestJson(twice.map((event) => JSON.stringify(event)).join("\n")), /second outcome/, "two outcomes of one test");
  const odd = [suite("started", "demo-facade", "effort_routes", "test"), testEvent("timeout", "demo-facade::effort_routes$a")];
  refuses(() => parseNextestJson(odd.map((event) => JSON.stringify(event)).join("\n")), /test event this reader does not know/, "an unknown test event");
  const opening = JSON.stringify(suite("started", "demo-facade", "effort_routes", "test"));
  const printedJson = parseNextestJson(`${opening}\n${JSON.stringify({ type: "bench", event: "ok", name: "demo-facade::effort_routes$a" })}\n{"type":"response.output_text.delta","delta":"what a test printed"}`);
  assert.deepEqual([printedJson.tests, printedJson.cell_lines], [[], []], "JSON a test prints is its output, not an event of nextest's");
  refuses(() => parseNextestJson(`${opening}\n${JSON.stringify(testEvent("ok", "a_test_without_its_binary"))}`), /names its test in a form/, "a test name without its binary");
  refuses(() => parseNextestJson(`${opening}\n{"type":"test","event":"ok","name":"demo-facade::effort_rou`), /line 2 of the nextest JSON output is not JSON/, "an event that is cut short");
  refuses(
    () => parseNextestJson(`${opening}\nrunning 1 test${JSON.stringify(testEvent("ok", "demo-facade::effort_routes$a"))}`),
    /line 2 of the nextest JSON output holds an event after other text/,
    "an event that shares its line with a test's output",
  );
});

row("the reader of nextest's log takes the summary's counts and only the tests it shows", () => {
  const shown = parseNextestLog(routesLog());
  assert.deepEqual(shown.counts, { run: 3, passed: 2, failed: 1, skipped: 2 });
  assert.equal(shown.tests_complete, true);
  assert.deepEqual(
    shown.tests.map((test) => [test.package, test.target, test.name, test.outcome]),
    [
      ["demo-core", "lib", "resolve::tests::table_is_total", "pass"],
      ["demo-facade", "effort_routes", "supported_level_is_sent", "pass"],
      ["demo-facade", "effort_routes", "unsupported_level_is_refused", "fail"],
    ],
  );
  const quiet = parseNextestLog(routesLog({ quiet: true }));
  assert.deepEqual(quiet.counts, shown.counts);
  assert.deepEqual(quiet.tests, [], "a log that hides the status lines names no test");
  assert.equal(quiet.tests_complete, false);
  assert.deepEqual(parseNextestLog("     Summary [  118.690s] 160 tests run: 160 passed (1 slow), 0 skipped\n").counts, { run: 160, passed: 160, failed: 0, skipped: 0 });

  refuses(() => parseNextestLog("error: could not compile\n"), /no summary line/, "a log without a summary is no result");
  refuses(() => parseNextestLog(`${routesLog()}${routesLog()}`), /second summary/, "two runs in one log");
  refuses(
    () => parseNextestLog("     Summary [   0.055s] 2 tests run: 1 passed (1 flaky), 1 failed, 4 skipped\n"),
    /summary this reader does not know/,
    "a summary with retried tests",
  );
  refuses(
    () => parseNextestLog("  TRY 1 FAIL [   0.003s] (───) demo-facade::effort_routes a\n     Summary [   0.055s] 1 test run: 1 passed, 0 skipped\n"),
    /status this reader does not know/,
    "a retried test's status line",
  );
  refuses(
    () =>
      parseNextestLog(
        [
          "        PASS [   0.004s] (1/2) demo-facade::effort_routes a_row",
          "        FAIL [   0.004s] (2/2) demo-facade::effort_routes a_row",
          "     Summary [   0.041s] 2 tests run: 1 passed, 1 failed, 0 skipped",
          "",
        ].join("\n"),
      ),
    /second, different outcome/,
    "two outcomes of one test",
  );
});

row("a leg result is refused when it contradicts itself", () => {
  // What `record` writes is held to the leg result's schema as a whole.
  refuses(() => routesResult({ commit: COMMIT.slice(0, 10) }), /the result of leg routes does not match its schema.*commit/, "a commit that is no full id");
  refuses(() => routesResult({ command: "" }), /the result of leg routes does not match its schema.*command/, "no command");
  refuses(() => routesResult({ exitStatus: 300 }), /the result of leg routes does not match its schema.*exit_status/, "an exit status no process has");
  refuses(() => routesResult({ leg: "Routes" }), /does not match its schema.*leg/, "a leg id of another shape");
  refuses(() => routesResult({ exitStatus: 0 }), /exit status 0 and 1 failed/, "a green exit status beside a failed test");
  const result = routesResult();
  const change = (edit) => {
    const copy = structuredClone(result);
    edit(copy);
    return withResult("routes.json", copy);
  };
  refuses(() => buildReceipt(change((copy) => (copy.counts.passed = 2))), /not its passed and failed tests together/, "counts that do not add up");
  refuses(() => buildReceipt(change((copy) => copy.tests.push(copy.tests[0]))), /names a test twice/, "a test named twice");
  refuses(
    () =>
      buildReceipt(
        change((copy) => {
          copy.counts = { run: 5, passed: 4, failed: 1, skipped: 1 };
        }),
      ),
    /names every test it ran, and its counts hold others/,
    "a complete list that is not complete",
  );
  refuses(
    () =>
      buildReceipt(
        change((copy) => {
          copy.counts = { run: 2, passed: 1, failed: 1, skipped: 1 };
          copy.tests_complete = false;
        }),
      ),
    /names more passed or failed tests than its counts hold/,
    "more named passes than the count of passes",
  );
});

row("schema refusal: an input that does not match its schema is an error", () => {
  const good = routesResult();
  for (const [what, edit, pattern] of [
    ["another schema version", (copy) => (copy.schema_version = 2), /schema_version/],
    ["another kind", (copy) => (copy.kind = "leg_table_row"), /kind/],
    ["a short commit", (copy) => (copy.commit = COMMIT.slice(0, 10)), /commit/],
    ["a missing member", (copy) => delete copy.counts, /has no counts/],
    ["a negative count", (copy) => (copy.counts.skipped = -1), /skipped/],
    ["an outcome outside the set", (copy) => (copy.tests[0].outcome = "flaky"), /outcome/],
    ["a member the schema does not know", (copy) => (copy.started_at = "2026-01-01"), /started_at/],
    ["an unknown not-run reason", (copy) => Object.assign(copy, buildNotRunResult({ leg: "routes", reason: "not_scheduled" }), { reason: "because" }), /allowed forms/],
  ]) {
    const copy = structuredClone(good);
    edit(copy);
    refuses(() => buildReceipt(withResult("routes.json", copy)), new RegExp(`routes\\.json does not match its schema.*${pattern.source}`), what);
  }
  refuses(() => buildReceipt(withResult("routes.json", Buffer.from("{ not json"))), /routes\.json is not JSON/, "a result file that is not JSON");
  refuses(() => buildReceipt(withResult("routes.json", Buffer.from("[]\n"))), /does not match its schema/, "a result file that is a list");
  refuses(() => buildNotRunResult({ leg: "routes", reason: "the provider said no" }), /does not match its schema/, "a free-text reason");

  const edited = manifest();
  edited.rows[0].outcome = { kind: "sent", reason: "unsupported_level" };
  refuses(() => buildReceipt(input({ manifestText: serialize(edited) })), /manifest does not match its schema/, "a manifest row of no allowed form");
  refuses(() => buildReceipt(input({ manifestText: "{" })), /manifest is not JSON/, "a manifest that is not JSON");
  refuses(() => buildReceipt(input({ version: "next" })), /receipt does not match its schema.*version/, "a version that is no version");
  refuses(() => buildReceipt(input({ commit: "HEAD" })), /receipt does not match its schema.*commit/, "a commit that is no full id");
});

row("schema refusal: the receipt schema holds what the generator promises", () => {
  const { receipt: schema } = schemas();
  const receipt = buildReceipt(input());
  assert.deepEqual(schemaErrors(schema, schema, receipt), [], "the generator's receipt matches the committed schema");
  const violates = (edit, what) => {
    const copy = structuredClone(receipt);
    edit(copy);
    assert.notDeepEqual(schemaErrors(schema, schema, copy), [], what);
  };
  const executed = receipt.entries.findIndex((entry) => entry.status === "executed");
  assert.notEqual(executed, -1);
  violates((copy) => (copy.entries[executed].result_file = null), "an executed entry without a result file");
  violates((copy) => (copy.entries[executed].result_file.sha256 = "abc"), "an executed entry with a digest that is none");
  violates((copy) => (copy.entries[executed].result = null), "an executed entry without a result");
  violates((copy) => (copy.entries[executed].test = null), "an executed entry without its test");
  violates((copy) => (copy.entries[executed].gap = "leg_not_run"), "an executed entry that is also a gap");
  const unexecuted = receipt.entries.findIndex((entry) => entry.status === "source_only");
  violates((copy) => (copy.entries[unexecuted].result = "pass"), "a result on an entry that did not run");
  violates((copy) => (copy.entries[unexecuted].gap = null), "an entry that did not run and states no gap");
  violates((copy) => (copy.entries[unexecuted].status = "skipped"), "a state outside the three");
  violates((copy) => (copy.schema_version = 2), "another schema version");
  violates((copy) => (copy.product.account = "someone"), "a member the schema does not know");
  violates((copy) => (copy.legs[0].stderr = "text"), "a leg with captured output");
  violates((copy) => (copy.entries[executed].outcome = { kind: "refused", reason: "The provider said no." }), "a refusal reason that is a sentence");
  violates((copy) => (copy.entries[executed].route = "https://example.invalid/v1"), "a route that is a URL");
});

row("schema refusal: the committed schemas use only keywords the validator checks", () => {
  let visited = 0;
  const walk = (rootSchema, schema) => {
    visited += 1;
    // Throws on a keyword the validator does not implement.
    schemaErrors(rootSchema, schema, null);
    const nested = [
      ...Object.values(schema.$defs ?? {}),
      ...Object.values(schema.properties ?? {}),
      ...(schema.oneOf ?? []),
      ...(schema.items === undefined ? [] : [schema.items]),
      ...(typeof schema.additionalProperties === "object" ? [schema.additionalProperties] : []),
    ];
    for (const item of nested) walk(rootSchema, item);
  };
  for (const schema of Object.values(schemas())) walk(schema, schema);
  assert.ok(visited > 300, `every subschema of the two files is visited (${visited})`);
  refuses(() => schemaErrors({}, { format: "uri" }, "x"), /keyword format .* is not supported/, "an unchecked keyword is an error of the schema");
  refuses(() => schemaErrors({}, { $ref: "#/$defs/nowhere" }, "x"), /names no definition/, "a dangling reference");

  const schema = { oneOf: [{ type: "string", pattern: "^a+$", maxLength: 2 }, { type: "integer", minimum: 3 }] };
  assert.deepEqual(schemaErrors(schema, schema, "aa"), []);
  assert.deepEqual(schemaErrors(schema, schema, 3), []);
  for (const value of ["aaa", "b", 2, 3.5, null, []]) assert.notDeepEqual(schemaErrors(schema, schema, value), [], JSON.stringify(value));
  const closed = { type: "object", additionalProperties: false, required: ["a"], properties: { a: { const: 1 } } };
  assert.deepEqual(schemaErrors(closed, closed, { a: 1 }), []);
  for (const value of [{}, { a: 2 }, { a: 1, b: 1 }, { a: 1, constructor: 1 }]) assert.notDeepEqual(schemaErrors(closed, closed, value), [], JSON.stringify(value));
  const both = { oneOf: [{ type: "integer" }, { minimum: 0 }] };
  assert.notDeepEqual(schemaErrors(both, both, 1), [], "a value of two forms at once is refused");
});

row("missing input: a leg without a result, and a result of no leg, are errors", () => {
  for (const leg of ["routes", "binary", "live"]) {
    const change = input();
    change.results.delete(`${leg}.json`);
    refuses(() => buildReceipt(change), new RegExp(`leg ${leg} has no result file ${leg}\\.json`), `no result of leg ${leg}`);
  }
  refuses(() => buildReceipt(input({ results: new Map() })), /has no result file/, "no results at all");
  refuses(
    () => buildReceipt(withResult("extra.json", buildNotRunResult({ leg: "extra", reason: "not_scheduled" }))),
    /extra\.json is the result of no leg/,
    "a result of a leg the manifest does not have",
  );
  refuses(
    () => buildReceipt(withResult("binary.json", buildNotRunResult({ leg: "live", reason: "not_scheduled" }))),
    /binary\.json is the result of leg live/,
    "a result filed under another leg's name",
  );
});

row("missing input: a row's test must be in the source at the commit", () => {
  refuses(() => buildReceipt(input({ readSource: () => null })), /which the commit does not have/, "a source file the commit lacks");
  const renamed = new Map(SOURCES);
  renamed.set(ROUTES_SOURCE, SOURCES.get(ROUTES_SOURCE).replace("fn supported_level_is_sent", "fn supported_level_reaches_the_wire"));
  refuses(
    () => buildReceipt(input({ readSource: (path) => (renamed.has(path) ? Buffer.from(renamed.get(path)) : null) })),
    /names the test supported_level_is_sent, which .* does not define/,
    "a test the file no longer defines",
  );
  assert.ok(definesTest("    pub(crate) async fn a_row() {", "tests::a_row"));
  assert.ok(!definesTest("fn a_row_longer() {}", "a_row"), "a longer name is another test");
  assert.ok(!definesTest("// fn a_row() in a comment\nlet a_row = 1;", "a_row"));
});

row("a test the manifest marks as awaited may be missing at the commit: its cells are not run, and say why", () => {
  const awaitedSource = "crates/demo-cli/tests/run_rows.rs";
  const awaiting = manifest();
  const binary = awaiting.rows.find((item) => item.id === "binary.supported");
  binary.test = { package: "demo-cli", target: "run_rows", name: "cli_level_is_sent", source: awaitedSource, awaited: true };
  binary.model = { rule: "catalog_model" };
  binary.requested_level = { level: "high" };
  const change = (text) =>
    input({
      manifestText: serialize(awaiting),
      readSource: (path) => (path === awaitedSource ? (text === null ? null : Buffer.from(text)) : SOURCES.has(path) ? Buffer.from(SOURCES.get(path)) : null),
    });
  const build = (text) => buildReceipt(change(text));
  const other = "#[test]\nfn cli_another_row() {}\n";
  const defined = "#[test]\nfn cli_level_is_sent() {}\n";

  // The file is not at the commit, or it is and does not define the test: not run, for that reason. The entry names no test.
  for (const [what, text] of [
    ["no source file", null],
    ["a file without the test", other],
  ]) {
    const receipt = build(text);
    const entry = entryOf(receipt, "binary.supported/direct");
    assert.deepEqual([entry.status, entry.gap, entry.test, entry.result, entry.result_file], ["not_run", "test_not_at_commit", null, null, null], what);
    assert.deepEqual(
      [entry.model, entry.requested_level, entry.outcome, entry.values_basis],
      [{ rule: "catalog_model" }, { level: "high" }, { kind: "sent", level: { level: "high" } }, "manifest"],
      `${what}: what the manifest says of the row is listed`,
    );
    assert.ok(receipt.gaps.some((gap) => gap.id === "binary.supported/direct" && gap.reason === "test_not_at_commit"), what);
    assert.ok(checkReceipt(receipt).includes("entry binary.supported/direct is a gap of the offline legs: test_not_at_commit"), `${what}: a gap of an offline leg like any other`);
    const markdown = renderMarkdown(receipt);
    assert.ok(markdown.includes("- `binary.supported`: The built binary sends an accepted level. (no test at this commit)"), what);
    assert.ok(markdown.includes("- Row `binary.supported`: 1 entry, the test the manifest names for it is not in the source at this commit yet."), what);
  }
  // Once the commit defines the test the row is like any other: source only until its leg runs here, executed when it does.
  const landed = entryOf(build(defined), "binary.supported/direct");
  assert.deepEqual([landed.status, landed.gap, landed.test.name, landed.test.source_sha256], ["source_only", "leg_not_run", "cli_level_is_sent", sha256(Buffer.from(defined))]);
  const ran = buildLegResult({
    leg: "binary",
    commit: COMMIT,
    command: "make e2e-system",
    exitStatus: 0,
    format: "nextest_libtest_json_plus",
    output: Buffer.from([suite("started", "demo-cli", "run_rows", "test"), testEvent("ok", "demo-cli::run_rows$cli_level_is_sent")].map((event) => JSON.stringify(event)).join("\n")),
    host: HOST,
  });
  const withRun = (text) => {
    const withBinary = change(text);
    withBinary.results.set("binary.json", file(ran));
    return entryOf(buildReceipt(withBinary), "binary.supported/direct");
  };
  assert.deepEqual([withRun(defined).status, withRun(defined).result, withRun(defined).test.name], ["executed", "pass", "cli_level_is_sent"]);
  // A result that names the test executes nothing while the commit does not define it.
  assert.deepEqual([withRun(null).status, withRun(null).gap, withRun(null).result_file], ["not_run", "test_not_at_commit", null]);
  assert.deepEqual([withRun(other).status, withRun(other).gap], ["not_run", "test_not_at_commit"]);

  // Without the mark both are errors, as for every other row.
  delete binary.test.awaited;
  refuses(() => build(null), /row binary\.supported of the manifest names crates\/demo-cli\/tests\/run_rows\.rs, which the commit does not have/, "an unmarked test whose file the commit lacks");
  refuses(() => build(other), /row binary\.supported of the manifest names the test cli_level_is_sent, which .* does not define/, "an unmarked test the file does not define");
  assert.equal(entryOf(build(defined), "binary.supported/direct").status, "source_only");
  // The mark is true or absent.
  binary.test.awaited = false;
  refuses(() => build(defined), /manifest does not match its schema/, "a mark that is not true");
  // A row with observed cells may await its test the same way.
  const live = manifest();
  live.rows.find((item) => item.id === "live.cli").test = { package: "demo-cli", target: "run_rows", name: "cli_live_matrix", source: awaitedSource, awaited: true };
  const awaitedLive = entryOf(buildReceipt(input({ manifestText: serialize(live) })), "live.cli/-");
  assert.deepEqual([awaitedLive.status, awaitedLive.gap, awaitedLive.test], ["not_run", "test_not_at_commit", null]);
});

row("missing input: the manifest's references must resolve", () => {
  const broken = (edit) => {
    const copy = manifest();
    edit(copy);
    return input({ manifestText: serialize(copy) });
  };
  refuses(() => buildReceipt(broken((copy) => (copy.rows[0].leg = "nightly"))), /names the leg nightly/, "an undefined leg");
  refuses(() => buildReceipt(broken((copy) => (copy.rows[0].cells[0].cell = "bedrock"))), /names the cell bedrock/, "an undefined cell");
  refuses(() => buildReceipt(broken((copy) => (copy.rows[0].model = { rule: "any_model" }))), /names the rule any_model/, "an undefined rule");
  refuses(() => buildReceipt(broken((copy) => copy.rows[0].cells.push({ cell: "direct" }))), /names the entry routes\.supported\/direct twice/, "a cell twice in a row");
  refuses(() => buildReceipt(broken((copy) => copy.rows.push(copy.rows[0]))), /names the row routes\.supported twice/, "a row twice");
  refuses(() => buildReceipt(broken((copy) => copy.cells.push(copy.cells[0]))), /names the cell direct twice/, "a cell twice");
  refuses(() => buildReceipt(broken((copy) => copy.legs.push(copy.legs[0]))), /names the leg routes twice/, "a leg twice");
  refuses(() => buildReceipt(broken((copy) => copy.rules.push(copy.rules[0]))), /names the rule catalog_model twice/, "a rule twice");
  refuses(() => buildReceipt(broken((copy) => (copy.rows[0].model = null))), /has a test and no model or no level/, "a tested row without a model");
});

row("deterministic bytes: the same inputs give the same two files", () => {
  const first = generate(input());
  const second = generate(input());
  assert.equal(first.json, second.json);
  assert.equal(first.markdown, second.markdown);
  assert.ok(first.json.endsWith("}\n") && !first.json.endsWith("\n\n"));
  assert.ok(first.markdown.endsWith("\n") && !first.markdown.endsWith("\n\n"));

  // The order the result files are handed over in changes nothing.
  const reversed = input();
  reversed.results = new Map([...reversed.results].reverse());
  assert.equal(generate(reversed).json, first.json);

  // Nothing of the moment or of the place: another host, same bytes.
  assert.equal(generate(input({ host: { hostname: "another-host-99", home: "/Users/someone" } })).json, first.json);

  // The observations of a leg result are stored in one order.
  const a = observation();
  const b = observation({ model: "listed-model-b", outcome: { kind: "refused", reason: "unsupported_level" } });
  assert.equal(serialize(liveResult([a, b])), serialize(liveResult([b, a])));

  // Another input, other bytes.
  assert.notEqual(generate(input({ version: "1.4.3" })).json, first.json);
  assert.notEqual(generate(input({ commit: OTHER_COMMIT })).json, first.json);
});

row("deterministic bytes: the Markdown file is the rendering of the JSON file", () => {
  const { json, markdown } = generate(input());
  assert.equal(markdown, renderMarkdown(JSON.parse(json)));
  assert.deepEqual(verifyReceipt({ jsonText: json, markdownText: markdown, results: input().results, host: HOST }), []);
  assert.deepEqual(
    verifyReceipt({ jsonText: json, markdownText: markdown.replace("| 7 | 7 | 4 | 3 |", "| 7 | 7 | 7 | 0 |"), host: HOST }),
    ["the Markdown file is not the rendering of the receipt"],
    "a Markdown file edited by hand is found",
  );
  const other = withResult("binary.json", buildNotRunResult({ leg: "binary", reason: "not_scheduled" })).results;
  assert.deepEqual(verifyReceipt({ jsonText: json, results: other, host: HOST }), ["the result file binary.json has another digest than the receipt names"]);
  other.delete("live.json");
  assert.ok(verifyReceipt({ jsonText: json, results: other, host: HOST }).includes("the result file live.json is missing"));
  assert.deepEqual(verifyReceipt({ jsonText: JSON.stringify(JSON.parse(json)), host: HOST }), ["the receipt is not written in the generator's form"]);
  assert.equal(verifyReceipt({ jsonText: json.replace('"schema_version": 1', '"schema_version": 3'), host: HOST }).length, 1);

  // What a table cell or a code span cannot hold as written.
  assert.ok(markdown.includes("test(/level/) \\| test(=live_matrix)") === false, "a command is not in a table");
  assert.ok(markdown.includes(`  - Command: \`${COMMAND}\``));
  const ticked = structuredClone(JSON.parse(json));
  ticked.legs[0].command = "run `this` | that";
  ticked.entries[0].title = "a | b";
  ticked.entries[0].model = { id: "a|b" };
  const rendered = renderMarkdown(ticked);
  assert.ok(rendered.includes("  - Command: `` run `this` | that ``"));
  assert.ok(rendered.includes("- `routes.supported`: a | b ("));
  assert.ok(rendered.includes("| `routes.supported/direct` | openai | `a\\|b` | responses |"), "a pipe cannot end a table cell");
});

row("not-run cells are listed: every cell of every row is an entry, whatever ran", () => {
  const expected = [];
  for (const item of manifest().rows) {
    if (item.cells === "observed") expected.push(`${item.id}/-`);
    else for (const cell of item.cells) expected.push(`${item.id}/${cell.cell}`);
  }
  const nothingRan = input();
  nothingRan.results.set("routes.json", file(buildNotRunResult({ leg: "routes", reason: "not_scheduled" })));
  for (const [what, change] of [
    ["one leg ran", input()],
    ["no leg ran", nothingRan],
  ]) {
    const receipt = buildReceipt(change);
    // A cell has one entry, or one for each level its row printed for it.
    const listed = receipt.entries.map((entry) => `${entry.row}/${entry.cell ?? "-"}`);
    assert.deepEqual(
      listed.filter((cell, index) => cell !== listed[index - 1]),
      expected,
      `${what}: the entries are the manifest's cells, in its order`,
    );
    assert.equal(new Set(receipt.entries.map((entry) => entry.id)).size, receipt.entries.length, `${what}: every entry has an id of its own`);
    for (const entry of receipt.entries) {
      assert.equal(entry.status === "executed", entry.gap === null, `${entry.id}: an entry is executed or a stated gap`);
    }
    const stated = receipt.gaps.filter((gap) => gap.scope === "entry").map((gap) => gap.id);
    assert.deepEqual(stated, receipt.entries.filter((entry) => entry.status !== "executed").map((entry) => entry.id), `${what}: every unexecuted entry is under gaps`);
    const { entries: totals, by_mode: byMode } = receipt.totals;
    assert.equal(totals.total, receipt.entries.length);
    assert.ok(totals.total >= expected.length);
    assert.equal(totals.executed + totals.source_only + totals.not_run, totals.total);
    assert.equal(totals.passed + totals.failed, totals.executed);
    assert.deepEqual(byMode.map((mode) => mode.mode), MODES);
    assert.equal(byMode.reduce((sum, mode) => sum + mode.total, 0), totals.total);
    assert.equal(receipt.totals.gaps, receipt.gaps.length);
    const markdown = renderMarkdown(receipt);
    for (const { id } of receipt.entries) assert.ok(markdown.includes(`| \`${id}\` |`), `${what}: the Markdown matrix lists ${id}`);
  }

  const receipt = buildReceipt(nothingRan);
  assert.deepEqual(
    receipt.entries.map((entry) => entry.id),
    expected,
    "nothing ran: one entry for each cell of the manifest",
  );
  assert.equal(receipt.totals.entries.executed, 0);
  assert.deepEqual(receipt.totals.leg_tests, { run: 0, passed: 0, failed: 0, skipped: 0 });
  assert.deepEqual(
    receipt.gaps.filter((gap) => gap.scope === "leg"),
    [
      { scope: "leg", id: "routes", reason: "not_scheduled" },
      { scope: "leg", id: "binary", reason: "rows_not_built" },
      { scope: "leg", id: "live", reason: "credential_unavailable" },
    ],
  );
  // A row in the source whose leg did not run is source only; a cell no
  // test covers is not run. Neither is ever executed.
  assert.deepEqual([entryOf(receipt, "routes.supported/direct").status, entryOf(receipt, "routes.supported/direct").gap], ["source_only", "leg_not_run"]);
  assert.deepEqual([entryOf(receipt, "binary.supported/direct").status, entryOf(receipt, "binary.supported/direct").gap], ["not_run", "no_test_row"]);
  assert.deepEqual([entryOf(receipt, "live.matrix/-").status, entryOf(receipt, "live.matrix/-").gap], ["source_only", "leg_not_run"]);
  assert.deepEqual([entryOf(receipt, "live.cli/-").status, entryOf(receipt, "live.cli/-").gap], ["not_run", "no_test_row"]);
  const planned = entryOf(receipt, "binary.supported/direct");
  assert.deepEqual([planned.model, planned.requested_level, planned.outcome, planned.mode], [null, null, { kind: "sent", level: null }, "built_binary"]);
  const markdown = renderMarkdown(receipt);
  assert.ok(markdown.includes("- Leg `binary`: not run, its rows are not built."));
  assert.ok(markdown.includes("- Row `routes.supported`: 2 entries, its leg did not run."));
  assert.ok(markdown.includes("- Row `live.cli`: 1 entry, no test covers it at this commit."));
});

row("an entry is executed only on its own recorded result at the product commit", () => {
  const change = input();
  const receipt = buildReceipt(change);
  const digest = sha256(change.results.get("routes.json"));
  const direct = entryOf(receipt, "routes.supported/direct");
  assert.deepEqual(
    [direct.status, direct.mode, direct.result, direct.tries, direct.gap, direct.outcome_basis],
    ["executed", "in_process", "pass", 1, null, "test_assertion"],
  );
  assert.deepEqual(direct.result_file, { name: "routes.json", sha256: digest }, "the entry points at the result file and its digest");
  assert.deepEqual(direct.test, { ...routesTest("supported_level_is_sent"), source_sha256: sha256(Buffer.from(SOURCES.get(ROUTES_SOURCE))) });
  assert.deepEqual([direct.provider_family, direct.api, direct.route], ["openai", "responses", "openai_api/responses"]);
  assert.deepEqual(
    [direct.model, direct.requested_level, direct.outcome, direct.values_basis],
    [{ id: "catalog-model-9" }, { level: "high" }, { kind: "sent", level: { level: "high" } }, "row_output"],
    "the row's own line names the model its rule picked",
  );
  const copilot = entryOf(receipt, "routes.supported/copilot");
  assert.deepEqual([copilot.route, copilot.model, copilot.requested_level], ["copilot/responses", { id: "fixture-model-1" }, { level: "low" }], "a cell's own model and level win");

  const failed = entryOf(receipt, "routes.unsupported/direct:max");
  assert.deepEqual([failed.status, failed.result, failed.outcome], ["executed", "fail", { kind: "refused", reason: "unsupported_level" }], "a failed test is an executed entry that failed");
  assert.equal(entryOf(receipt, "routes.resumed/direct").tries, 2, "a pass on the second try says so");
  assert.equal(entryOf(receipt, "routes.table/direct").status, "executed", "a library test is found by its target");
  assert.deepEqual(receipt.totals.entries, { total: 10, executed: 7, passed: 4, failed: 3, source_only: 1, not_run: 2 });
  assert.deepEqual(receipt.totals.leg_tests, { run: 4, passed: 3, failed: 1, skipped: 1 });
  for (const entry of receipt.entries) {
    assert.equal(entry.result_file !== null, entry.status === "executed", `${entry.id}: only an executed entry names a result file`);
  }
  const leg = receipt.legs[0];
  assert.deepEqual(
    [leg.status, leg.commit, leg.at_product_commit, leg.command, leg.exit_status, leg.per_test_results],
    ["run", COMMIT, true, COMMAND, 100, "complete"],
  );
  assert.deepEqual(leg.output, { format: "nextest_libtest_json_plus", sha256: sha256(Buffer.from(routesJson())), bytes: Buffer.byteLength(routesJson()) });

  // The same result recorded at another commit executes nothing here.
  const elsewhere = buildReceipt(withResult("routes.json", routesResult({ commit: OTHER_COMMIT })));
  assert.equal(elsewhere.totals.entries.executed, 0);
  assert.deepEqual(elsewhere.totals.leg_tests, { run: 0, passed: 0, failed: 0, skipped: 0 });
  assert.deepEqual([entryOf(elsewhere, "routes.supported/direct").status, entryOf(elsewhere, "routes.supported/direct").gap], ["source_only", "ran_at_another_commit"]);
  assert.deepEqual(elsewhere.gaps[0], { scope: "leg", id: "routes", reason: "ran_at_another_commit" });
  assert.deepEqual([elsewhere.legs[0].commit, elsewhere.legs[0].at_product_commit], [OTHER_COMMIT, false]);

  // Counts without the test's own line execute nothing either.
  const quiet = buildLegResult({ leg: "routes", commit: COMMIT, command: COMMAND, exitStatus: 100, format: "nextest_human_log", output: Buffer.from(routesLog({ quiet: true })), host: HOST });
  const counted = buildReceipt(withResult("routes.json", quiet));
  assert.equal(counted.totals.entries.executed, 0);
  assert.deepEqual(counted.totals.leg_tests, { run: 3, passed: 2, failed: 1, skipped: 2 });
  assert.equal(counted.legs[0].per_test_results, "none");
  assert.deepEqual([entryOf(counted, "routes.supported/direct").status, entryOf(counted, "routes.supported/direct").gap], ["source_only", "no_per_test_result"]);

  // A log that names its tests executes those and no other.
  const loud = buildLegResult({ leg: "routes", commit: COMMIT, command: COMMAND, exitStatus: 100, format: "nextest_human_log", output: Buffer.from(routesLog()), host: HOST });
  const shown = buildReceipt(withResult("routes.json", loud));
  assert.equal(entryOf(shown, "routes.supported/direct").result, "pass");
  assert.equal(entryOf(shown, "routes.unsupported/copilot").result, "fail");
  assert.deepEqual([entryOf(shown, "routes.resumed/direct").status, entryOf(shown, "routes.resumed/direct").gap], ["source_only", "test_not_in_leg_result"]);

  // A test the leg skipped is not executed.
  const skipping = routesJson().replace('"event":"ok","name":"demo-facade::effort_routes$supported_level_is_sent"', '"event":"ignored","name":"demo-facade::effort_routes$supported_level_is_sent"');
  const skipped = buildReceipt(withResult("routes.json", routesResult({ output: Buffer.from(skipping) })));
  assert.deepEqual([entryOf(skipped, "routes.supported/copilot").status, entryOf(skipped, "routes.supported/copilot").gap], ["source_only", "test_skipped"]);

  // A test of the same name in another package is another test.
  const namesake = [
    routesJson().replace('"event":"ok","name":"demo-facade::effort_routes$supported_level_is_sent"', '"event":"ok","name":"other-facade::effort_routes$supported_level_is_sent"'),
    JSON.stringify(suite("started", "other-facade", "effort_routes", "test")),
  ].join("\n");
  const borrowed = buildReceipt(withResult("routes.json", routesResult({ output: Buffer.from(namesake) })));
  assert.deepEqual([entryOf(borrowed, "routes.supported/direct").status, entryOf(borrowed, "routes.supported/direct").gap], ["source_only", "test_not_in_leg_result"]);
});

row("observed cells come from the leg result and from nowhere else", () => {
  const seen = [
    observation({ model: "listed-model-b", requested_level: "max", outcome: { kind: "refused", reason: "unsupported_level" } }),
    observation(),
    observation({ model: "listed-model-c", result: "skip" }),
    observation({ model: "listed-model-d", provider_family: "anthropic", api: "messages", requested_level: "low", outcome: { kind: "sent", level: "low" }, result: "fail" }),
  ];
  const change = withResult("live.json", liveResult(seen));
  const receipt = buildReceipt(change);
  const live = receipt.entries.filter((entry) => entry.row === "live.matrix");
  assert.deepEqual(
    live.map((entry) => [entry.id, entry.provider_family, entry.route, entry.model.id, entry.requested_level.level, entry.outcome, entry.status, entry.result, entry.gap]),
    [
      ["live.matrix/#1", "anthropic", "copilot/messages", "listed-model-d", "low", { kind: "sent", level: { level: "low" } }, "executed", "fail", null],
      ["live.matrix/#2", "openai", "copilot/responses", "listed-model-a", "high", { kind: "sent", level: { level: "high" } }, "executed", "pass", null],
      ["live.matrix/#3", "openai", "copilot/responses", "listed-model-b", "max", { kind: "refused", reason: "unsupported_level" }, "executed", "pass", null],
      ["live.matrix/#4", "openai", "copilot/responses", "listed-model-c", "high", { kind: "sent", level: { level: "high" } }, "not_run", "skip", "cell_skipped"],
    ],
  );
  for (const entry of live) {
    assert.deepEqual([entry.mode, entry.outcome_basis, entry.cell], ["live", "observation", null]);
    assert.deepEqual(entry.result_file, entry.status === "executed" ? { name: "live.json", sha256: sha256(change.results.get("live.json")) } : null);
  }
  assert.equal(entryOf(receipt, "live.cli/-").gap, "no_test_row", "a row without a test stays one entry that did not run");
  assert.equal(entryOf(buildReceipt(withResult("live.json", liveResult([]))), "live.matrix/-").gap, "no_cells_observed");

  // Cells seen at another commit are not this commit's cells.
  const elsewhere = buildReceipt(withResult("live.json", liveResult(seen, OTHER_COMMIT)));
  assert.deepEqual(
    elsewhere.entries.filter((entry) => entry.row === "live.matrix").map((entry) => [entry.id, entry.status, entry.gap, entry.model]),
    [["live.matrix/-", "source_only", "ran_at_another_commit", null]],
  );

  // Observations make entries only beside the row's own test, passed or
  // failed, in the same leg result. A result that holds them (from the
  // run's lines, or from a list given in a file) while the test was
  // skipped, or was not run at all, proves no cell: the row is one entry,
  // source only, and nothing of it is executed.
  for (const [event, gap] of [
    ["ignored", "test_skipped"],
    [null, "test_not_in_leg_result"],
  ]) {
    const unproven = buildReceipt(withResult("live.json", liveResult(seen, COMMIT, event)));
    assert.deepEqual(
      unproven.entries.filter((entry) => entry.row === "live.matrix").map((entry) => [entry.id, entry.status, entry.gap, entry.model, entry.result, entry.result_file]),
      [["live.matrix/-", "source_only", gap, null, null, null]],
      `observations beside a test the run reports as ${event ?? "absent"}`,
    );
  }
  // A failed test still ran: its cells are read, each with its own result.
  assert.deepEqual(
    buildReceipt(withResult("live.json", liveResult(seen, COMMIT, "failed")))
      .entries.filter((entry) => entry.row === "live.matrix")
      .map((entry) => [entry.id, entry.status, entry.result]),
    [
      ["live.matrix/#1", "executed", "fail"],
      ["live.matrix/#2", "executed", "pass"],
      ["live.matrix/#3", "executed", "pass"],
      ["live.matrix/#4", "not_run", "skip"],
    ],
  );

  // A result file written in another order lists the same cells in the same order.
  const reordered = structuredClone(liveResult(seen));
  reordered.observations.reverse();
  const withoutFile = (entries) => entries.map((entry) => ({ ...entry, result_file: null }));
  assert.deepEqual(withoutFile(buildReceipt(withResult("live.json", reordered)).entries), withoutFile(receipt.entries));

  refuses(() => liveResult([observation(), observation()]), /one observation twice/, "the same cell twice");
  refuses(() => buildReceipt(withResult("live.json", liveResult([observation({ row: "routes.supported" })]))), /no row of this leg with observed cells/, "an observation for a row with declared cells");
  refuses(() => buildReceipt(withResult("live.json", liveResult([observation({ row: "live.cli" })]))), /names no test for that row/, "an observation for a row without a test");
  refuses(() => liveResult([observation({ login: "someone" })]), /does not match its schema.*login/, "an observation with a member the schema does not know");
  refuses(() => liveResult([observation({ model: "a model with spaces" })]), /does not match its schema/, "a model id that is a phrase");
  refuses(() => liveResult([observation({ outcome: { kind: "refused", reason: "429 Too Many Requests" } })]), /does not match its schema/, "a refusal reason that is provider text");
});

row("the cells a live row prints are read from the run's output, each line exactly an observation", () => {
  const line = (item) => `${OBSERVATION_LINE_PREFIX}${JSON.stringify(item)}`;
  const sent = observation();
  const refused = observation({ model: "listed-model-b", requested_level: "max", outcome: { kind: "refused", reason: "unsupported_level" } });
  const failed = observation({ model: "listed-model-d", result: "fail", http_status: 429, error_code: "rate_limited" });
  const name = "demo-facade::effort_routes$live_matrix";
  // A run with --no-capture: the row's lines, the harness's and the row's other output, between the events.
  const output = (printed, end = testEvent("ok", name)) =>
    [suite("started", "demo-facade", "effort_routes", "test"), testEvent("started", name), "", "running 1 test", ...printed, CAPTURED, "test live_matrix ... ok", end]
      .map((event) => (typeof event === "string" ? event : JSON.stringify(event)))
      .join("\n");
  const record = (text, extra = {}) =>
    buildLegResult({ leg: "live", commit: COMMIT, command: "make e2e-copilot-live", exitStatus: 0, format: "nextest_libtest_json_plus", output: Buffer.from(text), host: HOST, ...extra });

  assert.deepEqual(parseNextestJson(output([line(refused), line(sent), `  ${line(failed)}\r`])).observations, [refused, sent, failed]);
  assert.deepEqual(parseNextestJson(output([])).observations, []);
  // The members of a line in any order: the leg result has them in the schema's.
  const reversed = Object.fromEntries(Object.entries(failed).reverse());
  assert.notEqual(JSON.stringify(reversed), JSON.stringify(failed));
  assert.equal(JSON.stringify(parseNextestJson(output([line(reversed)])).observations), JSON.stringify([failed]));
  // Inside a failed test's captured output, and in a log that shows a test's output.
  const captured = output([], testEvent("failed", name, { stdout: `${line(failed)}\n${CAPTURED}\n` }));
  assert.deepEqual(parseNextestJson(captured).observations, [failed]);
  assert.deepEqual(parseNextestLog(`  stdout ───\n    ${line(sent)}\n    ${CAPTURED}\n     Summary [   0.041s] 1 test run: 1 passed, 0 skipped\n`).observations, [sent]);

  // The two lines the live rows' own test pins, byte for byte.
  const written = [
    'reasoning-effort-receipt-observation {"row":"live.copilot_matrix","provider_family":"openai","backend":"copilot","api":"responses","model":"model-a","requested_level":"low","outcome":{"kind":"sent","level":"low"},"result":"fail","http_status":429,"error_code":"rate_limited"}',
    'reasoning-effort-receipt-observation {"row":"live.direct_providers","provider_family":"openai","backend":"openai_api","api":"responses","model":"model-a","requested_level":"max","outcome":{"kind":"refused","reason":"unsupported_level"},"result":"pass"}',
  ];
  assert.deepEqual(
    parseNextestJson(output(written)).observations.map((item) => [item.row, item.outcome.kind, item.result, item.http_status ?? null, item.error_code ?? null]),
    [
      ["live.copilot_matrix", "sent", "fail", 429, "rate_limited"],
      ["live.direct_providers", "refused", "pass", null, null],
    ],
  );

  // The leg result holds them as it holds observations given in a file, and the receipt lists them.
  const result = record(output([line(refused), line(sent), line(failed)]));
  assert.deepEqual(result.observations, liveResult([sent, refused, failed]).observations);
  assert.ok(!serialize(result).includes("quota"), "nothing else the row printed is kept");
  const receipt = buildReceipt(withResult("live.json", result));
  assert.deepEqual(
    receipt.entries.filter((entry) => entry.row === "live.matrix").map((entry) => [entry.id, entry.model.id, entry.result, entry.failure]),
    [
      ["live.matrix/#1", "listed-model-a", "pass", null],
      ["live.matrix/#2", "listed-model-b", "pass", null],
      ["live.matrix/#3", "listed-model-d", "fail", { http_status: 429, error_code: "rate_limited" }],
    ],
  );
  // Lines and a file together: one list; the same cell from both is one cell twice.
  assert.equal(record(output([line(sent)]), { observations: [refused] }).observations.length, 2);
  refuses(() => record(output([line(sent)]), { observations: [sent] }), /one observation twice/, "a cell in the file and in the output");
  // The same line twice is one cell; two lines that disagree about a cell are refused.
  assert.deepEqual(record(output([line(sent), line(sent)])).observations, [sent]);
  refuses(() => record(output([line(sent), line({ ...sent, result: "fail" })])), /observation line at line 6 .* says another thing of a cell of live\.matrix/, "a cell that passed and failed");
  refuses(
    () => record(output([line(failed), line({ ...failed, error_code: "server_error" })])),
    /says another thing of a cell of live\.matrix/,
    "a failed cell with two codes",
  );

  // A line is exactly an observation: no other member, no other shape, no text.
  for (const [what, text, pattern] of [
    ["a member the schema does not know", line({ ...sent, login: "someone" }), /observation line at line 5 .* does not match its schema.*login/],
    ["the provider's message", line({ ...failed, message: "You exceeded your current quota." }), /does not match its schema.*message/],
    ["a code that is a sentence", line({ ...failed, error_code: "quota exceeded for the account" }), /does not match its schema.*error_code/],
    ["a model the row withheld a part of", line({ ...sent, model: `listed-${WITHHELD_MODEL_ID}` }), /does not match its schema.*model/],
    ["a missing member", line({ row: "live.matrix", model: "listed-model-a" }), /does not match its schema.*has no provider_family/],
    ["a level outside the set", line({ ...sent, requested_level: "x_high" }), /does not match its schema.*requested_level/],
    ["a list", `${OBSERVATION_LINE_PREFIX}[]`, /does not match its schema/],
    ["no object", `${OBSERVATION_LINE_PREFIX}PASS route=copilot/responses`, /observation line at line 5 .* is not JSON/],
  ]) {
    refuses(() => parseNextestJson(output([text])), pattern, what);
  }
  refuses(
    () => parseNextestJson(output([], testEvent("failed", name, { stdout: `${line({ ...sent, account: "someone" })}\n` }))),
    /observation line at line \d+ .*the test's output.* does not match its schema.*account/,
    "a line with another member inside a failed test's output",
  );
  refuses(() => parseNextestLog(`    ${line({ ...sent, host: "somewhere" })}\n     Summary [   0.041s] 1 test run: 1 passed, 0 skipped\n`), /does not match its schema.*host/, "a line with another member in a log");
  refuses(() => record(output([line({ ...sent, http_status: 200 })])), /on an observation that did not fail/, "a status on a cell that passed");
  refuses(() => record(output([]), { observations: [{ ...sent, login: "someone" }] }), /observation 1 of the observations given does not match its schema.*login/, "a file item with another member");
  // The prefix is the one the live rows print (the row of the committed
  // manifest reads it in their source). The two prefixes are two words: a
  // cell line is no observation, and the reverse.
  assert.equal(OBSERVATION_LINE_PREFIX, "reasoning-effort-receipt-observation ");
  assert.notEqual(OBSERVATION_LINE_PREFIX, CELL_LINE_PREFIX);
  refuses(() => parseNextestJson(output([`${CELL_LINE_PREFIX}${JSON.stringify(sent)}`])), /cell line at line 5 .* does not match its schema/, "an observation after the cell line's prefix");

  // From the command line: the run's output is all `record` needs.
  const work = scratchDirectory();
  writeFileSync(join(work, "live.nextest.json"), output([line(refused), line(sent), line(failed)]));
  const recorded = cli(["record", "--leg", "live", "--commit", COMMIT, "--command", "make e2e-copilot-live", "--exit-status", "0", "--nextest-json", join(work, "live.nextest.json"), "--output", join(work, "live.json")]);
  assert.deepEqual([recorded.status, recorded.stdout], [0, "recorded leg live: 1 run, 1 passed, 0 failed, 0 skipped, 1 named, 0 cell lines, 3 observations\n"]);
  assert.deepEqual(JSON.parse(readFileSync(join(work, "live.json"), "utf8")).observations, [sent, refused, failed]);
  writeFileSync(join(work, "bad.nextest.json"), output([line({ ...sent, login: "someone" })]));
  const refusedRecord = cli(["record", "--leg", "live", "--commit", COMMIT, "--command", "make e2e-copilot-live", "--exit-status", "0", "--nextest-json", join(work, "bad.nextest.json"), "--output", join(work, "bad.json")]);
  assert.equal(refusedRecord.status, 1);
  assert.ok(/^refused: the observation line at line 5 .* does not match its schema/u.test(refusedRecord.stdout), refusedRecord.stdout);
  assert.equal(existsSync(join(work, "bad.json")), false, "a refused record writes nothing");
});

row("a failed live cell carries the typed status and code its row emitted, never a message", () => {
  const failed = observation({ model: "listed-model-d", result: "fail", http_status: 429, error_code: "rate_limited" });
  const receipt = buildReceipt(withResult("live.json", liveResult([failed, observation({ model: "listed-model-e", result: "fail" }), observation()])));
  assert.deepEqual(
    receipt.entries.filter((entry) => entry.row === "live.matrix").map((entry) => [entry.model.id, entry.result, entry.failure]),
    [
      ["listed-model-a", "pass", null],
      ["listed-model-d", "fail", { http_status: 429, error_code: "rate_limited" }],
      ["listed-model-e", "fail", null],
    ],
  );
  assert.ok(renderMarkdown(receipt).includes("| fail (HTTP 429, code `rate_limited`) |"));
  const onlyStatus = buildReceipt(withResult("live.json", liveResult([observation({ result: "fail", http_status: 503 })])));
  assert.deepEqual(entryOf(onlyStatus, "live.matrix/#1").failure, { http_status: 503, error_code: null });
  for (const entry of receipt.entries) assert.equal(entry.failure !== null && entry.result !== "fail", false, `${entry.id}: only a failure carries one`);

  refuses(() => liveResult([observation({ http_status: 200 })]), /on an observation that did not fail/, "a status on a cell that passed");
  refuses(() => liveResult([observation({ result: "skip", error_code: "not_listed" })]), /on an observation that did not fail/, "a code on a cell that was skipped");
  for (const [what, extra] of [
    ["the provider's message", { message: "You exceeded your current quota." }],
    ["an error text member", { error: "quota exceeded" }],
    ["a code that is a sentence", { error_code: "quota exceeded for the account" }],
    ["a status that is no HTTP status", { http_status: 42 }],
    ["a status written as text", { http_status: "429 Too Many Requests" }],
  ]) {
    refuses(() => liveResult([observation({ result: "fail", ...extra })]), /does not match its schema/, what);
  }
  const { receipt: schema } = schemas();
  const copy = structuredClone(receipt);
  copy.entries.find((entry) => entry.failure !== null).failure.message = "text";
  assert.notDeepEqual(schemaErrors(schema, schema, copy), [], "the receipt schema has no place for a message either");
});

row("a model id a live row withheld is a form of its own, and never the model a row is to show", () => {
  const withheld = observation({ model: WITHHELD_MODEL_ID });
  assert.equal(WITHHELD_MODEL_ID, schemas().inputs.$defs.withheld_model_id.const, "the constant is the schema's");
  assert.equal(new RegExp(schemas().inputs.$defs.model_id.pattern, "u").test(WITHHELD_MODEL_ID), false, "no model id can be the constant");

  // Accepted where a live row says what it saw: in a line and in a file. The leg result keeps the constant.
  const opening = [suite("started", "demo-facade", "effort_routes", "test"), testEvent("ok", "demo-facade::effort_routes$live_matrix")].map((event) => JSON.stringify(event));
  assert.deepEqual(parseNextestJson([opening[0], `${OBSERVATION_LINE_PREFIX}${JSON.stringify(withheld)}`, opening[1]].join("\n")).observations, [withheld]);
  const result = liveResult([observation(), withheld]);
  assert.deepEqual(result.observations.map((item) => item.model), [WITHHELD_MODEL_ID, "listed-model-a"]);

  // In the receipt the cell is executed with its own result, and its model is the statement, not an id.
  const receipt = buildReceipt(withResult("live.json", result));
  assert.deepEqual(
    receipt.entries.filter((entry) => entry.row === "live.matrix").map((entry) => [entry.id, entry.model, entry.status, entry.result, entry.values_basis, entry.route]),
    [
      ["live.matrix/#1", { observed: "id_withheld" }, "executed", "pass", "observation", "copilot/responses"],
      ["live.matrix/#2", { id: "listed-model-a" }, "executed", "pass", "observation", "copilot/responses"],
    ],
  );
  const { json, markdown } = generate(withResult("live.json", result));
  assert.ok(!json.includes(WITHHELD_MODEL_ID) && !markdown.includes(WITHHELD_MODEL_ID), "the receipt states it in its own words");
  assert.ok(markdown.includes("| `live.matrix/#1` | openai | observed, id withheld | responses | `copilot/responses` | `high` | sent: `high` | executed, live | pass |"));
  const skipped = buildReceipt(withResult("live.json", liveResult([observation({ model: WITHHELD_MODEL_ID, result: "skip" })])));
  assert.deepEqual([entryOf(skipped, "live.matrix/#1").model, entryOf(skipped, "live.matrix/#1").gap], [{ observed: "id_withheld" }, "cell_skipped"]);

  // Only the constant: nothing near it is the constant or a model id.
  for (const model of [`${WITHHELD_MODEL_ID}-5`, `listed-${WITHHELD_MODEL_ID}`, WITHHELD_MODEL_ID.toUpperCase(), WITHHELD_MODEL_ID.slice(1), ` ${WITHHELD_MODEL_ID}`, "<withheld>", "<>"]) {
    refuses(() => liveResult([observation({ model })]), /does not match its schema.*model/, `the model ${model}`);
  }
  // A scripted row has nothing to withhold: its cell line names a model id.
  refuses(
    () => parseNextestJson(`${opening[0]}\n${cellLine("a_row", "direct", WITHHELD_MODEL_ID, "low", "openai_api/responses")}`),
    /cell line at line 2 .* does not match its schema.*model/,
    "a cell line with a withheld model",
  );
  // The receipt schema has the statement as one form of a model, and no id that is the constant.
  const { receipt: schema } = schemas();
  const withModel = (model) => {
    const copy = structuredClone(receipt);
    copy.entries.find((entry) => entry.id === "live.matrix/#1").model = model;
    return schemaErrors(schema, schema, copy);
  };
  assert.deepEqual(withModel({ observed: "id_withheld" }), []);
  assert.notDeepEqual(withModel({ observed: "id_known" }), []);
  assert.notDeepEqual(withModel({ id: WITHHELD_MODEL_ID }), [], "no model id of a receipt is the constant");
  assert.notDeepEqual(withModel({ observed: "id_withheld", id: "listed-model-a" }), []);

  // A model the manifest names for the row, on a cell of the matrix.
  const naming = manifest();
  naming.rows.find((item) => item.id === "live.matrix").named_models = [{ cell: "copilot", model: { id: "listed-model-a" } }];
  const named = (result, edit = naming) => {
    const change = withResult("live.json", result);
    change.manifestText = serialize(edit);
    return buildReceipt(change);
  };
  const live = (built) => built.entries.filter((entry) => entry.row === "live.matrix").map((entry) => [entry.id, entry.status, entry.gap]);
  // A cell that names the id shows it, whatever became of the cell: the named model has no entry of its own.
  assert.deepEqual(live(named(result)), [
    ["live.matrix/#1", "executed", null],
    ["live.matrix/#2", "executed", null],
  ]);
  assert.deepEqual(live(named(liveResult([observation({ result: "fail" })]))), [["live.matrix/#1", "executed", null]]);
  assert.deepEqual(live(named(liveResult([observation({ result: "skip" })]))), [["live.matrix/#1", "not_run", "cell_skipped"]]);
  // A cell with its id withheld never shows it: the named model is a gap, not a pass.
  const hidden = named(liveResult([withheld]));
  assert.deepEqual(live(hidden), [
    ["live.matrix/#1", "executed", null],
    ["live.matrix/copilot", "source_only", "model_not_proven"],
  ]);
  const unproven = entryOf(hidden, "live.matrix/copilot");
  assert.deepEqual(
    [unproven.cell, unproven.provider_family, unproven.model, unproven.api, unproven.route, unproven.requested_level, unproven.outcome, unproven.values_basis, unproven.result, unproven.result_file, unproven.test.name],
    ["copilot", "openai", { id: "listed-model-a" }, "responses", "copilot/responses", null, null, "manifest", null, null, "live_matrix"],
  );
  assert.deepEqual(hidden.gaps.filter((gap) => gap.id.startsWith("live.matrix/")), [{ scope: "entry", id: "live.matrix/copilot", reason: "model_not_proven" }]);
  assert.equal(entryOf(hidden, "live.matrix/#1").result, "pass", "the withheld cell itself passed");
  // The check names it among the live gaps and, as every gap of a live leg, does not hold it against the receipt.
  assert.ok(liveGaps(hidden).includes("entry live.matrix/copilot is a gap of the live legs: model_not_proven"));
  assert.ok(!checkReceipt(hidden).some((problem) => problem.includes("live.matrix")));
  const shown = renderMarkdown(hidden);
  assert.ok(shown.includes("| `live.matrix/copilot` | openai | `listed-model-a` | responses | `copilot/responses` | - | - | source only: its leg saw a cell of this route with its model id withheld: the named model is not proven | - |"));
  assert.ok(shown.includes("- Row `live.matrix`: 1 entry, its leg saw a cell of this route with its model id withheld: the named model is not proven."));
  // The totals count the named models no cell shows on their own line, apart from the rows that did not run.
  assert.deepEqual(hidden.totals.named_models, { not_proven: 1, not_observed: 0 });
  assert.ok(shown.includes("\nModels named for a live row that no observed cell shows: 1 not proven (a cell of the route had its model id withheld), 0 not observed. "));
  assert.deepEqual(named(result).totals.named_models, { not_proven: 0, not_observed: 0 }, "a named model a cell shows is counted nowhere");
  assert.deepEqual(named(liveResult([observation({ model: "listed-model-b" })])).totals.named_models, { not_proven: 0, not_observed: 1 });
  const two = manifest();
  two.rows.find((item) => item.id === "live.matrix").named_models = [
    { cell: "copilot", model: { id: "listed-model-a" } },
    { cell: "direct", model: { id: "listed-model-a" } },
  ];
  const twoNamed = named(liveResult([withheld]), two);
  assert.deepEqual(twoNamed.totals.named_models, { not_proven: 1, not_observed: 1 });
  assert.ok(renderMarkdown(twoNamed).includes("that no observed cell shows: 1 not proven (a cell of the route had its model id withheld), 1 not observed."));
  assert.deepEqual(named(buildNotRunResult({ leg: "live", reason: "credential_unavailable" }), two).totals.named_models, { not_proven: 0, not_observed: 0 }, "a named model of a row that did not run carries the row's gap and is a row not run");
  assert.ok(renderMarkdown(buildReceipt(input())).includes("that no observed cell shows: 0 not proven (a cell of the route had its model id withheld), 0 not observed."), "the line is there when no model is named");
  const uncounted = structuredClone(hidden);
  delete uncounted.totals.named_models;
  assert.notDeepEqual(schemaErrors(schema, schema, uncounted), [], "a receipt without the count does not match the schema");
  // Another model, and a cell of another route (the named id or a withheld one), show nothing of this cell.
  assert.deepEqual(live(named(liveResult([observation({ model: "listed-model-b" })]))).at(-1), ["live.matrix/copilot", "source_only", "model_not_observed"]);
  for (const elsewhere of [{ backend: "openai_api" }, { api: "chat_completions" }, { provider_family: "anthropic" }]) {
    for (const model of ["listed-model-a", WITHHELD_MODEL_ID]) {
      assert.deepEqual(
        live(named(liveResult([observation({ model, ...elsewhere })]))).at(-1),
        ["live.matrix/copilot", "source_only", "model_not_observed"],
        `${model} on ${JSON.stringify(elsewhere)}`,
      );
    }
  }
  assert.ok(renderMarkdown(named(liveResult([]))).includes("- Row `live.matrix`: 1 entry, its leg ran and recorded no cell of the named model on this route."));
  // Where the row has no result here, the named model is listed with the row's own gap.
  assert.deepEqual(live(named(liveResult([]))), [
    ["live.matrix/-", "source_only", "no_cells_observed"],
    ["live.matrix/copilot", "source_only", "model_not_observed"],
  ]);
  assert.deepEqual(live(named(buildNotRunResult({ leg: "live", reason: "credential_unavailable" }))), [
    ["live.matrix/-", "source_only", "leg_not_run"],
    ["live.matrix/copilot", "source_only", "leg_not_run"],
  ]);
  assert.deepEqual(live(named(liveResult([observation()], OTHER_COMMIT))), [
    ["live.matrix/-", "source_only", "ran_at_another_commit"],
    ["live.matrix/copilot", "source_only", "ran_at_another_commit"],
  ]);
  const untested = manifest();
  untested.rows.find((item) => item.id === "live.cli").named_models = [{ cell: "direct", model: { id: "listed-model-a" } }];
  const planned = entryOf(named(liveResult([observation()]), untested), "live.cli/direct");
  assert.deepEqual([planned.status, planned.gap, planned.model, planned.route, planned.test], ["not_run", "no_test_row", { id: "listed-model-a" }, "openai_api/responses", null]);

  // What the manifest may name: one model id for a cell it defines.
  for (const [what, list, pattern] of [
    ["a cell the manifest does not define", [{ cell: "nowhere", model: { id: "listed-model-a" } }], /row live\.matrix of the manifest names the cell nowhere, which it does not define/],
    ["two models for one cell", [{ cell: "copilot", model: { id: "listed-model-a" } }, { cell: "copilot", model: { id: "listed-model-b" } }], /names the entry live\.matrix\/copilot twice/],
    ["a model by rule", [{ cell: "copilot", model: { rule: "catalog_model" } }], /manifest does not match its schema/],
    ["the constant as the model", [{ cell: "copilot", model: { id: WITHHELD_MODEL_ID } }], /manifest does not match its schema/],
    ["no model at all", [], /manifest does not match its schema/],
  ]) {
    const copy = manifest();
    copy.rows.find((item) => item.id === "live.matrix").named_models = list;
    refuses(() => buildReceipt(input({ manifestText: serialize(copy) })), pattern, what);
  }
  const declared = manifest();
  declared.rows[0].named_models = [{ cell: "copilot", model: { id: "listed-model-a" } }];
  refuses(() => buildReceipt(input({ manifestText: serialize(declared) })), /manifest does not match its schema/, "a named model on a row whose cells the manifest lists");
});

row("the rows' cell lines name the model and level a row picked, and are the only output read", () => {
  const direct = { test: "supported_level_is_sent", cell: "direct", model: "catalog-model-9", level: "high", route: "openai_api/responses" };
  const copilot = { test: "supported_level_is_sent", cell: "copilot", model: "fixture-model-1", level: "low", route: "copilot/responses" };
  const refused = { test: "unsupported_level_is_refused", cell: "direct", model: "catalog-model-9", level: "max", route: "openai_api/responses" };
  const refusedNone = { ...refused, level: "none" };
  // Whole lines of a run with --no-capture, and the line inside a failed test's output; sorted.
  const resumed = { test: "resumed_level_is_sent", cell: "direct", model: "catalog-model-9", level: "low", route: "openai_api/responses" };
  assert.deepEqual(parseNextestJson(routesJson()).cell_lines, [resumed, copilot, direct, refusedNone, refused], "by test, cell and level, the lowest level first");
  assert.deepEqual(parseNextestJson(routesJson({ cellLines: false })).cell_lines, []);
  // The log of a run that shows a passing test's output, indented as nextest prints it.
  assert.deepEqual(parseNextestLog(routesLog()).cell_lines, [direct]);
  assert.deepEqual(parseNextestLog(routesLog({ quiet: true })).cell_lines, []);
  const result = routesResult();
  assert.deepEqual(result.cell_lines, [resumed, copilot, direct, refusedNone, refused]);
  assert.ok(!serialize(result).includes("quota"), "the rest of a failed test's output is not kept");

  // In the receipt: the picked values replace the rule, on executed entries only.
  const receipt = buildReceipt(input());
  const entry = (id) => {
    const found = entryOf(receipt, id);
    return [found.model, found.requested_level, found.outcome, found.values_basis];
  };
  assert.deepEqual(entry("routes.supported/direct"), [{ id: "catalog-model-9" }, { level: "high" }, { kind: "sent", level: { level: "high" } }, "row_output"]);
  assert.deepEqual(entry("routes.supported/copilot"), [{ id: "fixture-model-1" }, { level: "low" }, { kind: "sent", level: { level: "low" } }, "row_output"]);
  assert.deepEqual(entry("routes.unsupported/direct:max"), [{ id: "catalog-model-9" }, { level: "max" }, { kind: "refused", reason: "unsupported_level" }, "row_output"]);
  assert.deepEqual(entry("routes.unsupported/copilot"), [{ rule: "catalog_model" }, { rule: "refused_levels" }, { kind: "refused", reason: "unsupported_level" }, "manifest"], "a cell without a line keeps the manifest's rule");
  assert.deepEqual(entry("routes.resumed/direct"), [{ id: "catalog-model-9" }, { level: "low" }, { kind: "sent", level: { level: "low" } }, "row_output"], "a level the row picks by rule is the level it printed");
  assert.deepEqual(entry("binary.supported/direct"), [null, null, { kind: "sent", level: null }, null]);
  assert.deepEqual(entryOf(receipt, "live.matrix/-").values_basis, null);
  const without = buildReceipt(withResult("routes.json", routesResult({ output: Buffer.from(routesJson({ cellLines: false })) })));
  assert.deepEqual([entryOf(without, "routes.supported/direct").model, entryOf(without, "routes.supported/direct").values_basis], [{ rule: "catalog_model" }, "manifest"]);
  assert.deepEqual([entryOf(without, "routes.resumed/direct").requested_level, entryOf(without, "routes.resumed/direct").outcome], [{ rule: "accepted_level" }, { kind: "sent", level: { rule: "accepted_level" } }]);
  // Lines of a run at another commit, and of a row that has no result of its own, name nothing.
  const elsewhere = buildReceipt(withResult("routes.json", routesResult({ commit: OTHER_COMMIT })));
  assert.deepEqual([entryOf(elsewhere, "routes.supported/direct").model, entryOf(elsewhere, "routes.supported/direct").values_basis], [{ rule: "catalog_model" }, "manifest"]);
  // That run is not held to this commit's manifest either: its rows may be other rows.
  const otherRows = structuredClone(routesResult({ commit: OTHER_COMMIT }));
  otherRows.cell_lines.push({ ...direct, test: "a_row_of_that_commit" });
  assert.equal(buildReceipt(withResult("routes.json", otherRows)).totals.entries.executed, 0);
  const logOnly = buildLegResult({ leg: "routes", commit: COMMIT, command: COMMAND, exitStatus: 100, format: "nextest_human_log", output: Buffer.from(routesLog({ quiet: true }).replace("────────────\n", `────────────\n    ${cellLine("supported_level_is_sent", "direct", "catalog-model-9", "high", "openai_api/responses")}\n`)), host: HOST });
  assert.equal(logOnly.cell_lines.length, 1);
  assert.deepEqual([entryOf(buildReceipt(withResult("routes.json", logOnly)), "routes.supported/direct").status, entryOf(buildReceipt(withResult("routes.json", logOnly)), "routes.supported/direct").model], ["source_only", { rule: "catalog_model" }]);

  // A line is exactly its five members, in the shapes the schema gives them.
  const opening = JSON.stringify(suite("started", "demo-facade", "effort_routes", "test"));
  const withLine = (line) => () => parseNextestJson(`${opening}\n${line}`);
  assert.deepEqual(withLine(`  ${cellLine("a_row", "direct", "m-1", "low", "openai_api/responses")}\r`)().cell_lines.length, 1, "leading and trailing white space is not part of a line");
  assert.deepEqual(withLine(`${cellLine("a_row", "direct", "m-1", "low", "openai_api/responses")}\n${cellLine("a_row", "direct", "m-1", "low", "openai_api/responses")}`)().cell_lines.length, 1, "the same line twice is one line");
  refuses(withLine(`${cellLine("a_row", "direct", "m-1", "low", "openai_api/responses")}\n${cellLine("a_row", "direct", "m-2", "low", "openai_api/responses")}`), /says another thing of a_row, cell direct/, "two lines that disagree");
  refuses(withLine(`${CELL_LINE_PREFIX}not json`), /cell line at line 2 .* is not JSON/, "a prefix and no object");
  refuses(withLine(`${CELL_LINE_PREFIX}${JSON.stringify({ ...direct, account: "someone" })}`), /does not match its schema.*account/, "a line with another member");
  refuses(withLine(`${CELL_LINE_PREFIX}${JSON.stringify({ ...direct, level: "x_high" })}`), /does not match its schema.*level/, "a level outside the set");
  refuses(withLine(`${CELL_LINE_PREFIX}${JSON.stringify({ ...direct, model: "a model the provider refused: quota" })}`), /does not match its schema.*model/, "a model that is a phrase");
  refuses(withLine(`${CELL_LINE_PREFIX}${JSON.stringify({ ...direct, route: "https://example.invalid/v1" })}`), /does not match its schema.*route/, "a route that is a URL");
  refuses(withLine(`${CELL_LINE_PREFIX}${JSON.stringify({ test: direct.test, cell: direct.cell })}`), /has no model/, "a line without its values");
  // With --no-capture the test harness and the tests print between the
  // events. None of it is read but the cell lines.
  const uncaptured = parseNextestJson(
    [
      opening,
      JSON.stringify(testEvent("started", "demo-facade::effort_routes$a_row")),
      "",
      "running 1 test",
      CAPTURED,
      '{"model":"what a test printed for itself"}',
      cellLine("a_row", "direct", "m-1", "low", "openai_api/responses"),
      "test a_row ... ok",
      "",
      "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.05s",
      JSON.stringify(testEvent("ok", "demo-facade::effort_routes$a_row")),
    ].join("\n"),
  );
  assert.deepEqual([uncaptured.counts, uncaptured.cell_lines.length], [{ run: 1, passed: 1, failed: 0, skipped: 0 }, 1]);
  assert.ok(!JSON.stringify(uncaptured).includes("quota") && !JSON.stringify(uncaptured).includes("printed for itself"), "a test's other output is not kept");
  refuses(
    () => parseNextestJson(`${opening}\n${JSON.stringify(testEvent("failed", "demo-facade::effort_routes$a_row", { stdout: `${CELL_LINE_PREFIX}{"test":1}\n` }))}`),
    /cell line at line 2 .*the test's output.* does not match its schema/,
    "a malformed line inside a failed test's output",
  );
  refuses(() => parseNextestLog(`    ${CELL_LINE_PREFIX}{}\n     Summary [   0.041s] 1 test run: 1 passed, 0 skipped\n`), /does not match its schema/, "a malformed line in a log");

  // A line must be of a cell the manifest lists, and agree with what the manifest fixes.
  const edited = (edit) => {
    const copy = structuredClone(result);
    edit(copy.cell_lines);
    return withResult("routes.json", copy);
  };
  refuses(() => buildReceipt(edited((lines) => lines.push({ ...direct, test: "a_row_the_manifest_lacks" }))), /cell line of a_row_the_manifest_lacks, cell direct, which is not a cell of exactly one row/, "a line of a test no row names");
  refuses(() => buildReceipt(edited((lines) => lines.push({ ...direct, test: "resumed_level_is_sent", cell: "copilot" }))), /cell line of resumed_level_is_sent, cell copilot, which is not a cell/, "a line of a cell the row does not list");
  refuses(() => buildReceipt(edited((lines) => lines.push({ ...direct, test: "live_matrix" }))), /which is not a cell of exactly one row/, "a line of a row with observed cells");
  refuses(() => buildReceipt(edited((lines) => (lines[2].route = "openai_api/chat_completions"))), /row routes\.supported reports the route openai_api\/chat_completions for the cell direct, and the manifest says openai_api\/responses/, "another route than the manifest's cell");
  refuses(() => buildReceipt(edited((lines) => (lines[1].model = "fixture-model-2"))), /reports the model fixture-model-2 for the cell copilot, and the manifest says fixture-model-1/, "another model than the manifest fixes");
  refuses(() => buildReceipt(edited((lines) => (lines[2].level = "low"))), /reports the level low for the cell direct, and the manifest says high/, "another level than the manifest fixes");
  refuses(() => buildReceipt(edited((lines) => lines.push(lines[0]))), /two cell lines of one level of one cell of one test/, "one level of one cell twice in a result file");
  const twoRows = manifest();
  twoRows.rows.push({ ...twoRows.rows[0], id: "routes.supported_again" });
  refuses(() => buildReceipt(input({ manifestText: serialize(twoRows) })), /which is not a cell of exactly one row/, "a line two rows would claim");
});

row("a row that asks for several levels on a cell has an entry for each level it printed", () => {
  const change = input();
  const receipt = buildReceipt(change);
  const digest = sha256(change.results.get("routes.json"));
  const unsupported = receipt.entries.filter((entry) => entry.row === "routes.unsupported");
  assert.deepEqual(
    unsupported.map((entry) => [entry.id, entry.cell, entry.model, entry.requested_level, entry.values_basis, entry.status, entry.result]),
    [
      ["routes.unsupported/direct:none", "direct", { id: "catalog-model-9" }, { level: "none" }, "row_output", "executed", "fail"],
      ["routes.unsupported/direct:max", "direct", { id: "catalog-model-9" }, { level: "max" }, "row_output", "executed", "fail"],
      ["routes.unsupported/copilot", "copilot", { rule: "catalog_model" }, { rule: "refused_levels" }, "manifest", "executed", "fail"],
    ],
    "one entry for each level the row printed, the lowest first whatever order it printed them in; a cell without a line keeps its one entry",
  );
  for (const entry of unsupported) {
    assert.deepEqual([entry.outcome, entry.route.split("/")[1], entry.result_file, entry.test.name], [{ kind: "refused", reason: "unsupported_level" }, "responses", { name: "routes.json", sha256: digest }, "unsupported_level_is_refused"]);
  }
  const markdown = renderMarkdown(receipt);
  assert.ok(markdown.includes("| `routes.unsupported/direct:none` | openai | `catalog-model-9` | responses | `openai_api/responses` | `none` | refused: `unsupported_level` |"));
  assert.ok(markdown.includes("| `routes.unsupported/direct:max` | openai | `catalog-model-9` | responses | `openai_api/responses` | `max` | refused: `unsupported_level` |"));

  // Every level a cell can be asked for, printed in no order: listed lowest first.
  assert.deepEqual(LEVELS, schemas().receipt.$defs.level.enum, "the order of the levels is the schema's");
  const every = structuredClone(routesResult());
  every.cell_lines = every.cell_lines.filter((line) => line.test !== "unsupported_level_is_refused");
  for (const level of ["max", "xhigh", "none", "medium", "minimal", "low", "high"]) {
    every.cell_lines.push({ test: "unsupported_level_is_refused", cell: "copilot", model: "listed-model-a", level, route: "copilot/responses" });
  }
  const ladder = buildReceipt(withResult("routes.json", every)).entries.filter((entry) => entry.row === "routes.unsupported");
  assert.deepEqual(
    ladder.map((entry) => entry.id),
    ["routes.unsupported/direct", ...LEVELS.map((level) => `routes.unsupported/copilot:${level}`)],
  );
  assert.deepEqual(ladder.slice(1).map((entry) => entry.requested_level.level), LEVELS);

  // A row that did not run here has one entry for a cell, whatever lines its result holds.
  for (const [what, result, gap] of [
    ["a leg that did not run", buildNotRunResult({ leg: "routes", reason: "not_scheduled" }), "leg_not_run"],
    ["a run at another commit", routesResult({ commit: OTHER_COMMIT }), "ran_at_another_commit"],
  ]) {
    assert.deepEqual(
      buildReceipt(withResult("routes.json", result))
        .entries.filter((entry) => entry.row === "routes.unsupported")
        .map((entry) => [entry.id, entry.requested_level, entry.status, entry.gap]),
      [
        ["routes.unsupported/direct", { rule: "refused_levels" }, "source_only", gap],
        ["routes.unsupported/copilot", { rule: "refused_levels" }, "source_only", gap],
      ],
      what,
    );
  }

  // The reader keeps a line for each level of a cell, and refuses two that disagree on one level.
  const opening = JSON.stringify(suite("started", "demo-facade", "effort_routes", "test"));
  const lines = (...items) => parseNextestJson([opening, ...items].join("\n")).cell_lines;
  assert.deepEqual(
    lines(cellLine("a_row", "direct", "m-1", "max", "openai_api/responses"), cellLine("a_row", "direct", "m-1", "low", "openai_api/responses"), cellLine("a_row", "direct", "m-1", "max", "openai_api/responses")).map((line) => line.level),
    ["low", "max"],
    "two levels of one cell are two lines; the same line twice is one",
  );
  refuses(
    () => lines(cellLine("a_row", "direct", "m-1", "low", "openai_api/responses"), cellLine("a_row", "direct", "m-2", "low", "openai_api/responses")),
    /says another thing of a_row, cell direct, level low/,
    "two lines of one level that disagree",
  );

  // Only a row the manifest gives an entry per level may print several levels for a cell.
  const result = routesResult();
  const edited = (edit) => {
    const copy = structuredClone(result);
    edit(copy.cell_lines);
    return withResult("routes.json", copy);
  };
  const supported = result.cell_lines.find((line) => line.test === "supported_level_is_sent" && line.cell === "direct");
  refuses(
    () => buildReceipt(edited((items) => items.push({ ...supported, level: "low" }))),
    /two cell lines of supported_level_is_sent, cell direct, and row routes\.supported of the manifest has one entry for a cell/,
    "a second level for a cell of a row with one entry for a cell",
  );
  // Each line of such a row is held to the manifest's cell and to the model it fixes.
  const refused = result.cell_lines.filter((line) => line.test === "unsupported_level_is_refused");
  assert.equal(refused.length, 2);
  refuses(() => buildReceipt(edited((items) => (items.at(-1).route = "openai_api/chat_completions"))), /row routes\.unsupported reports the route openai_api\/chat_completions for the cell direct/, "a second line on another route");
  const fixing = manifest();
  fixing.rows[1].cells[0].model = { id: "catalog-model-9" };
  assert.equal(buildReceipt(input({ manifestText: serialize(fixing) })).totals.entries.executed, 7);
  refuses(
    () => buildReceipt({ ...edited((items) => (items.at(-1).model = "catalog-model-8")), manifestText: serialize(fixing) }),
    /row routes\.unsupported reports the model catalog-model-8 for the cell direct, and the manifest says catalog-model-9/,
    "a second line with another model than the manifest fixes",
  );
  // The manifest names the levels of such a row by rule: one fixed level would be one of several.
  for (const [what, edit] of [
    ["a level fixed for the row", (rows) => (rows[1].requested_level = { level: "max" })],
    ["a level fixed for a cell", (rows) => (rows[1].cells[1].requested_level = { level: "high" })],
  ]) {
    const copy = manifest();
    edit(copy.rows);
    refuses(() => buildReceipt(input({ manifestText: serialize(copy) })), /row routes\.unsupported of the manifest has an entry for each level and fixes one level for the cell/, what);
  }
  const misnamed = manifest();
  misnamed.rows[1].lines = "per_test";
  refuses(() => buildReceipt(input({ manifestText: serialize(misnamed) })), /manifest does not match its schema/, "another word for how a row prints");
  // Without the mark the same lines are one too many.
  const unmarked = manifest();
  delete unmarked.rows[1].lines;
  refuses(() => buildReceipt(input({ manifestText: serialize(unmarked) })), /two cell lines of unsupported_level_is_refused, cell direct, and row routes\.unsupported of the manifest has one entry for a cell/, "a row without the mark");

  // The receipt schema takes the level in an executed entry's id and nowhere else.
  const { receipt: schema } = schemas();
  const withId = (id, status) => {
    const copy = structuredClone(receipt);
    copy.entries.find((entry) => entry.status === status).id = id;
    return schemaErrors(schema, schema, copy);
  };
  assert.deepEqual(withId("routes.unsupported/direct:xhigh", "executed"), []);
  assert.notDeepEqual(withId("routes.unsupported/direct:", "executed"), []);
  assert.notDeepEqual(withId("routes.unsupported/direct:High", "executed"), []);
  assert.notDeepEqual(withId("routes.unsupported/direct:max", "source_only"), [], "an entry that did not run is one for its cell");
});

row("a second thing a test asserts is a row of its own, and the lines of that case are its lines", () => {
  // The refusing test also refuses an accepted level, for another reason, on one route.
  const cased = manifest();
  cased.rows.splice(2, 0, {
    id: "routes.conflict",
    leg: "routes",
    title: "An accepted level is refused when the session sets a control that conflicts with it.",
    case: "conflict",
    test: routesTest("unsupported_level_is_refused"),
    outcome: { kind: "refused", reason: "conflicting_control" },
    model: { rule: "catalog_model" },
    requested_level: { rule: "accepted_level" },
    cells: [{ cell: "copilot" }],
  });
  const ofCase = { test: "unsupported_level_is_refused", case: "conflict", cell: "copilot", model: "listed-model-a", level: "high", route: "copilot/responses" };
  const text = (line) => `${CELL_LINE_PREFIX}${JSON.stringify(line)}`;
  const run = (...lines) => routesResult({ output: Buffer.from(routesJson({ also: lines.map(text) })) });
  const build = (result, edit = cased) => {
    const change = withResult("routes.json", result);
    change.manifestText = serialize(edit);
    return buildReceipt(change);
  };

  // The reader keeps the case, after the test; a line without one sorts first.
  const result = run({ route: ofCase.route, level: ofCase.level, model: ofCase.model, cell: ofCase.cell, case: ofCase.case, test: ofCase.test });
  assert.equal(JSON.stringify(result.cell_lines.filter((line) => line.case !== undefined)), JSON.stringify([ofCase]), "the members in the schema's order");
  assert.deepEqual(
    result.cell_lines.map((line) => [line.test, line.case ?? "-", line.cell, line.level].join("/")),
    [
      "resumed_level_is_sent/-/direct/low",
      "supported_level_is_sent/-/copilot/low",
      "supported_level_is_sent/-/direct/high",
      "unsupported_level_is_refused/-/direct/none",
      "unsupported_level_is_refused/-/direct/max",
      "unsupported_level_is_refused/conflict/copilot/high",
    ],
  );

  // The case is one entry of its own row, with the row's outcome and the test's result.
  const receipt = build(result);
  const entry = entryOf(receipt, "routes.conflict/copilot");
  assert.deepEqual(
    [entry.row, entry.cell, entry.model, entry.requested_level, entry.outcome, entry.values_basis, entry.status, entry.result, entry.test.name, entry.title],
    ["routes.conflict", "copilot", { id: "listed-model-a" }, { level: "high" }, { kind: "refused", reason: "conflicting_control" }, "row_output", "executed", "fail", "unsupported_level_is_refused", cased.rows[2].title],
  );
  assert.ok(renderMarkdown(receipt).includes("| `routes.conflict/copilot` | openai | `listed-model-a` | responses | `copilot/responses` | `high` | refused: `conflicting_control` | executed, in process | fail |"));
  // The test's row without a case keeps its own lines and takes none of the case's.
  assert.deepEqual(
    receipt.entries.filter((item) => item.row === "routes.unsupported").map((item) => [item.id, item.model]),
    [
      ["routes.unsupported/direct:none", { id: "catalog-model-9" }],
      ["routes.unsupported/direct:max", { id: "catalog-model-9" }],
      ["routes.unsupported/copilot", { rule: "catalog_model" }],
    ],
  );
  // A line of the case and a line without one, of one cell and level, are two lines: one for each row.
  const both = build(run(ofCase, { test: ofCase.test, cell: "copilot", model: "listed-model-b", level: "high", route: "copilot/responses" }));
  assert.deepEqual(
    [entryOf(both, "routes.conflict/copilot").model, entryOf(both, "routes.unsupported/copilot:high").model],
    [{ id: "listed-model-a" }, { id: "listed-model-b" }],
  );
  // Without its line the case is still an entry, on the test's own result, named by the manifest's rules.
  const silent = entryOf(build(routesResult()), "routes.conflict/copilot");
  assert.deepEqual([silent.status, silent.result, silent.model, silent.requested_level, silent.values_basis], ["executed", "fail", { rule: "catalog_model" }, { rule: "accepted_level" }, "manifest"]);

  // A line belongs to exactly one row: the row of its test and its case that lists its cell.
  const strayLine = (line, pattern, what, edit) => refuses(() => build(run(line), edit), pattern, what);
  strayLine({ ...ofCase, case: "another" }, /cell line of unsupported_level_is_refused \(case another\), cell copilot, which is not a cell of exactly one row/, "a case no row has");
  strayLine({ ...ofCase, test: "supported_level_is_sent" }, /cell line of supported_level_is_sent \(case conflict\), cell copilot, which is not a cell/, "a case on a line of a test whose row has none");
  strayLine({ ...ofCase, cell: "direct", route: "openai_api/responses" }, /\(case conflict\), cell direct, which is not a cell of exactly one row/, "a cell the case's row does not list");
  strayLine(ofCase, /\(case conflict\), cell copilot, which is not a cell of exactly one row/, "a manifest without the case's row", manifest());
  const twice = structuredClone(cased);
  twice.rows.push({ ...twice.rows[2], id: "routes.conflict_again" });
  strayLine(ofCase, /\(case conflict\), cell copilot, which is not a cell of exactly one row/, "two rows of one test and one case", twice);
  // The case's row has one entry for its cell: a second level is one line too many.
  refuses(
    () => build(run(ofCase, { ...ofCase, level: "low" })),
    /two cell lines of unsupported_level_is_refused \(case conflict\), cell copilot, and row routes\.conflict of the manifest has one entry for a cell/,
    "two levels for the case",
  );
  // Its line is held to the manifest like any other.
  refuses(() => build(run({ ...ofCase, route: "copilot/chat_completions" })), /row routes\.conflict reports the route copilot\/chat_completions for the cell copilot/, "another route");
  // Two lines of the case that disagree, and the same line twice in a result file.
  refuses(() => run(ofCase, { ...ofCase, model: "listed-model-b" }), /says another thing of unsupported_level_is_refused \(case conflict\), cell copilot, level high/, "two lines of one case that disagree");
  assert.equal(run(ofCase, ofCase).cell_lines.length, 6, "the same line twice is one line");
  const doubled = structuredClone(result);
  doubled.cell_lines.push(ofCase);
  refuses(() => build(doubled), /two cell lines of one level of one cell of one test/, "a case's line twice in a result file");
  // The case is one word, on the line and in the manifest.
  refuses(() => run({ ...ofCase, case: "Second Case" }), /does not match its schema.*case/, "a case that is a phrase");
  const phrased = structuredClone(cased);
  phrased.rows[2].case = "Second Case";
  refuses(() => build(result, phrased), /manifest does not match its schema/, "a case of the manifest that is a phrase");
  const observedCase = manifest();
  observedCase.rows.find((item) => item.id === "live.matrix").case = "conflict";
  refuses(() => buildReceipt(input({ manifestText: serialize(observedCase) })), /manifest does not match its schema/, "a case on a row with observed cells");
});

row("--check fails on a failed entry and on a gap of the offline legs, never on a gap of a live leg", () => {
  // The fixture as it stands: two failed entries, one in-process entry its
  // log did not name, the built-binary row without a test; the live gaps.
  const receipt = buildReceipt(input());
  assert.deepEqual(checkReceipt(receipt), [
    "entry routes.unsupported/direct:none failed",
    "entry routes.unsupported/direct:max failed",
    "entry routes.unsupported/copilot failed",
    "leg binary is a gap of the offline legs: rows_not_built",
    "entry binary.supported/direct is a gap of the offline legs: no_test_row",
  ]);
  assert.ok(receipt.gaps.some((gap) => gap.scope === "leg" && gap.id === "live") && receipt.gaps.some((gap) => gap.id === "live.matrix/-"), "the live gaps are in the receipt");
  // The gaps of the live legs are named, each with its reason, and are no problem of the check.
  const liveOnes = ["leg live is a gap of the live legs: credential_unavailable", "entry live.matrix/- is a gap of the live legs: leg_not_run", "entry live.cli/- is a gap of the live legs: no_test_row"];
  assert.deepEqual(liveGaps(receipt), liveOnes);
  assert.equal(checkReceipt(receipt).filter((problem) => problem.includes(" is a gap of ")).length + liveGaps(receipt).length, receipt.gaps.length, "every gap is held against the receipt or named beside it");

  // Every offline entry executed and passed: the live gaps alone pass the check.
  const offline = manifest();
  offline.rows = offline.rows.filter((item) => item.leg !== "binary");
  offline.legs = offline.legs.filter((leg) => leg.id !== "binary");
  const green = routesJson().replace('"event":"failed","name":"demo-facade::effort_routes$unsupported_level_is_refused"', '"event":"ok","name":"demo-facade::effort_routes$unsupported_level_is_refused"');
  const passing = input({ manifestText: serialize(offline) });
  passing.results.delete("binary.json");
  passing.results.set("routes.json", file(routesResult({ output: Buffer.from(green), exitStatus: 0 })));
  const clean = buildReceipt(passing);
  assert.deepEqual([clean.totals.entries.failed, clean.gaps.map((gap) => `${gap.scope} ${gap.id} ${gap.reason}`)], [0, ["leg live credential_unavailable", "entry live.matrix/- leg_not_run", "entry live.cli/- no_test_row"]]);
  assert.deepEqual(checkReceipt(clean), [], "gaps of a live leg do not fail the check");
  assert.deepEqual(liveGaps(clean), liveOnes, "and are named all the same");

  // Each offline gap class fails it; a live failure fails it; a live skip does not.
  const kinds = (change) => checkReceipt(buildReceipt(change)).map((problem) => problem.replace(/^.*: /u, "").replace(/^entry .* failed$/u, "failed"));
  const withRoutes = (result) => {
    const change = input({ manifestText: serialize(offline) });
    change.results.delete("binary.json");
    change.results.set("routes.json", file(result));
    return change;
  };
  assert.deepEqual([...new Set(kinds(withRoutes(buildNotRunResult({ leg: "routes", reason: "not_scheduled" }))))], ["not_scheduled", "leg_not_run"]);
  assert.deepEqual([...new Set(kinds(withRoutes(buildNotRunResult({ leg: "routes", reason: "ran_at_another_commit" }))))], ["ran_at_another_commit", "leg_not_run"]);
  assert.deepEqual([...new Set(kinds(withRoutes(routesResult({ output: Buffer.from(green), exitStatus: 0, commit: OTHER_COMMIT }))))], ["ran_at_another_commit"]);
  const quiet = buildLegResult({ leg: "routes", commit: COMMIT, command: COMMAND, exitStatus: 100, format: "nextest_human_log", output: Buffer.from(routesLog({ quiet: true })), host: HOST });
  assert.deepEqual([...new Set(kinds(withRoutes(quiet)))], ["no_per_test_result"]);
  const skipping = green.replace('"event":"ok","name":"demo-facade::effort_routes$supported_level_is_sent"', '"event":"ignored","name":"demo-facade::effort_routes$supported_level_is_sent"');
  assert.deepEqual([...new Set(kinds(withRoutes(routesResult({ output: Buffer.from(skipping), exitStatus: 0 }))))], ["test_skipped"]);
  const live = (observations) => {
    const change = withRoutes(routesResult({ output: Buffer.from(green), exitStatus: 0 }));
    change.results.set("live.json", file(liveResult(observations)));
    return change;
  };
  assert.deepEqual(kinds(live([observation({ result: "skip" })])), [], "a cell a live leg skipped is a live gap");
  assert.deepEqual(kinds(live([observation(), observation({ model: "listed-model-b", result: "fail" })])), ["failed"], "a failed live cell fails the check");
  assert.deepEqual(kinds(live([])), [], "a live leg that recorded no cell is a live gap");
  // A gap the check cannot place is held against the receipt, not passed over.
  const stray = structuredClone(clean);
  stray.gaps.push({ scope: "entry", id: "routes.supported/elsewhere", reason: "leg_not_run" });
  assert.deepEqual(checkReceipt(stray), ["the gap of entry routes.supported/elsewhere names no entry of the receipt"]);
  assert.deepEqual(liveGaps(stray), liveOnes, "a gap that names no entry is no live gap");
  assert.deepEqual(liveGaps(buildReceipt(withRoutes(buildNotRunResult({ leg: "routes", reason: "not_scheduled" })))), liveOnes, "a gap of an offline leg is no live gap");
});

row("forbidden fields are absent: nothing of an account, a credential, a host or a run's text is written", () => {
  const result = routesResult();
  const { json, markdown } = generate(input());
  for (const text of [serialize(result), json, markdown]) {
    assert.ok(!text.includes("quota") && !text.includes("octo-org") && !text.includes("429"), "a failed test's captured output is not written");
    assert.ok(!text.includes(HOST.home) && !text.toLowerCase().includes("build-host-17"), "nothing of the host is written");
  }
  const forbidden = /^(?:account|login|user(?:name)?|org(?:ani[sz]ation)?|plan|token|secret|key|credential|authorization|host(?:name)?|url|endpoint|path|cwd|worktree|log|error|message|detail|stdout|stderr|started_at|ended_at|timestamp)$/u;
  const names = new Set([...memberNames(result), ...memberNames(JSON.parse(json)), ...propertyNames(schemas().receipt), ...propertyNames(schemas().inputs)]);
  assert.ok(names.has("result_file") && names.has("exit_status") && names.has("observations"));
  assert.deepEqual([...names].filter((name) => forbidden.test(name)), [], "no member of a leg result, a receipt or their schemas is one of these");

  const refusedCommand = (command, kind) => {
    refuses(() => routesResult({ command }), new RegExp(`holds ${kind} at \\$\\.command`), `record: ${kind}`);
    const smuggled = structuredClone(result);
    smuggled.command = command;
    refuses(() => buildReceipt(withResult("routes.json", smuggled)), new RegExp(`routes\\.json holds ${kind} at \\$\\.command`), `generate: ${kind}`);
  };
  refusedCommand("bash /home/operator/legs/run.sh routes", "an absolute path");
  refusedCommand("./scripts/repo-cargo nextest run --target-dir=/var/cache/target", "an absolute path");
  refusedCommand("./run --state-root ~/state", "an absolute path");
  refusedCommand("run.exe --root C:\\Users\\operator", "an absolute path");
  refusedCommand("curl https://api.example.invalid/v1/models", "a URL");
  refusedCommand(`env COPILOT_LIVE_USER_TOKEN=${"x".repeat(12)} make e2e-copilot-live`, "a value assigned to a secret's name");
  refusedCommand("env OPENAI_API_KEY=abc ./run", "a value assigned to a secret's name");
  refusedCommand(`./run --header ${CREDENTIAL}`, "a credential");
  refusedCommand(["./run --header 'Authorization: Bea", "rer ", "abcdefgh", "12345678'"].join(""), "a bearer credential");
  refusedCommand("./run --as someone@example.invalid", "an e-mail address");
  refusedCommand("./run on BUILD-HOST-17 now", "this host's name");
  refusedCommand("./run --note '(/home/operator)'", "this host's home directory");
  refusedCommand("./run first\n./run second", "a control character");
  // A command names what it unsets and which tests it selects without tripping.
  for (const command of [
    "env -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GEMINI_API_KEY ./scripts/repo-cargo nextest run -p demo-facade --features a,b --test effort_routes",
    "env CARGO_BUILD_JOBS=32 nice -n 5 ./scripts/repo-cargo nextest run -E 'package(demo-facade) & test(/level/)'",
    "make e2e-copilot-live",
  ]) {
    assert.equal(routesResult({ command }).command, command);
  }

  // The finished receipt is scanned again, whatever the inputs passed.
  const receipt = buildReceipt(input());
  assert.doesNotThrow(() => assertPublishable(receipt, HOST, "the receipt"));
  const leaking = structuredClone(receipt);
  leaking.legs[0].title = "Run by someone@example.invalid";
  refuses(() => assertPublishable(leaking, HOST, "the receipt"), /holds an e-mail address at \$\.legs\[0\]\.title/, "a string of the receipt");
  const keyed = structuredClone(receipt);
  keyed.totals["/home/operator"] = 1;
  refuses(() => assertPublishable(keyed, HOST, "the receipt"), /member name/, "a member name of the receipt");
  const titled = manifest();
  titled.legs[0].title = "See https://example.invalid/runs/1";
  refuses(() => buildReceipt(input({ manifestText: serialize(titled) })), /receipt holds a URL/, "a manifest title");
  for (const text of strings(receipt)) assert.ok(!/^\//u.test(text), `no string of the receipt starts at the file system root: ${text}`);
});

row("the version is read from Cargo.toml's workspace package", () => {
  assert.equal(workspaceVersion('[package]\nversion = "9.9.9"\n\n[workspace.package]\nedition = "2024"\nversion = "1.4.2" # the release\n\n[workspace.dependencies]\nversion = "0.0.1"\n'), "1.4.2");
  refuses(() => workspaceVersion('[package]\nversion = "9.9.9"\n'), /no \[workspace\.package\] version/, "a manifest without a workspace version");
  assert.equal(workspaceVersion(readFileSync(join(root, "Cargo.toml"), "utf8")).split(".").length, 3, "this repository's root manifest");
});

row("the committed manifest matches its schema, names tests the tree defines, and lists every cell", () => {
  const manifestText = readFileSync(join(root, MANIFEST_PATH), "utf8");
  const committed = JSON.parse(manifestText);
  assert.equal(serialize(committed).length > 0, true);
  const results = new Map(committed.legs.map((leg) => [`${leg.id}.json`, file(buildNotRunResult({ leg: leg.id, reason: "not_scheduled" }))]));
  const read = (path) => {
    try {
      return readFileSync(join(root, path));
    } catch {
      return null;
    }
  };
  // Fails when a row names a test its source file does not define.
  const { json, markdown } = generate({ manifestText, version: workspaceVersion(read("Cargo.toml").toString("utf8")), commit: COMMIT, results, readSource: read, host: HOST });
  const receipt = JSON.parse(json);
  let cells = 0;
  for (const item of committed.rows) cells += item.cells === "observed" ? 1 : item.cells.length;
  assert.equal(receipt.entries.length, cells, "one entry per cell of the committed manifest");
  assert.equal(receipt.totals.entries.executed, 0);
  assert.ok(receipt.entries.some((entry) => entry.status === "source_only") && receipt.entries.some((entry) => entry.status === "not_run"));
  assert.deepEqual(verifyReceipt({ jsonText: json, markdownText: markdown, results, host: HOST }), []);
  assert.equal(`scripts/${receipt.manifest.name}`, MANIFEST_PATH);
  // A test the manifest marks as awaited is one the tree does not define, and
  // its row's cells are the not-run cells that say so. When the test lands,
  // the mark goes.
  const awaitedRows = committed.rows.filter((item) => item.test?.awaited === true);
  for (const item of awaitedRows) {
    const source = read(item.test.source);
    assert.equal(source !== null && definesTest(source.toString("utf8"), item.test.name), false, `${item.id}: its test is in the tree now, so the awaited mark must leave the manifest`);
  }
  assert.deepEqual(
    [...new Set(receipt.entries.filter((entry) => entry.gap === "test_not_at_commit").map((entry) => entry.row))],
    awaitedRows.map((item) => item.id),
    "the cells that wait for their test are the cells of the rows marked so",
  );
  // The built-binary rows name their tests, one test a row: the three of the
  // direct routes over the three direct cells, and the supported row of the
  // self-hosted server. Its two refusal rows name none yet.
  assert.deepEqual(
    committed.rows.filter((item) => item.leg === "binary" && item.test !== null).map((item) => [item.id, item.test.package, item.test.target, item.test.name, item.cells.map((cell) => cell.cell).join(" ")]),
    [
      ["binary.supported", "rkat", "run_reasoning_effort_families", "cli_supported_run_reports_requested_and_sent_on_direct_routes", "openai_direct claude_direct gemini_direct"],
      ["binary.unsupported", "rkat", "run_reasoning_effort_families", "cli_unsupported_level_is_refused_on_direct_routes", "openai_direct claude_direct gemini_direct"],
      ["binary.unknown", "rkat", "run_reasoning_effort_families", "cli_unknown_support_is_refused_on_direct_routes", "openai_direct claude_direct gemini_direct"],
      ["binary.self_hosted_supported", "rkat", "run_reasoning_effort", "cli_supported_run_reports_requested_and_sent", "self_hosted_chat"],
    ],
  );
  assert.deepEqual(
    committed.rows.filter((item) => item.leg === "binary" && item.test === null).map((item) => [item.id, item.cells.map((cell) => cell.cell).join(" ")]),
    [
      ["binary.self_hosted_unsupported", "self_hosted_chat"],
      ["binary.self_hosted_unknown", "self_hosted_chat"],
    ],
  );
  // The route rows print their cells under the manifest's own names, and
  // every route row of the manifest prints them.
  const routesSource = "crates/meerkat/tests/reasoning_effort_routes.rs";
  const routes = read(routesSource).toString("utf8");
  const naming = /fn receipt_cell\(self\) -> &'static str \{([\s\S]*?)\n {4}\}/u.exec(routes);
  assert.ok(naming !== null, "the route rows name their cells");
  assert.deepEqual(
    [...naming[1].matchAll(/=> "([a-z0-9_]+)"/gu)].map((match) => match[1]).sort(),
    committed.cells.map((cell) => cell.id).sort(),
    "the cell names the route rows print are the manifest's cells",
  );
  assert.ok(routes.includes(`"${CELL_LINE_PREFIX}{}"`), "the route rows print the prefix the generator reads");
  assert.deepEqual(
    [...new Set([...routes.matchAll(/report_cell\(\s*"([A-Za-z0-9_]+)"/gu)].map((match) => match[1]))].sort(),
    [...new Set(committed.rows.filter((item) => item.test?.source === routesSource).map((item) => item.test.name))].sort(),
    "the tests that print their cells are the manifest's route rows",
  );
  // A case of a route row that the manifest lists as a row of its own
  // prints its line under the manifest's word for it.
  assert.deepEqual(
    [...routes.matchAll(/report_case\(\s*"([A-Za-z0-9_]+)",\s*"([a-z0-9_]+)"/gu)].map((match) => `${match[1]} ${match[2]}`).sort(),
    committed.rows.filter((item) => item.case !== undefined).map((item) => `${item.test.name} ${item.case}`).sort(),
    "the cases the route rows print are the manifest's rows with a case",
  );
  // Every refusal reason the route rows assert is the outcome of a row: a
  // refusal a row asserts without an entry of its own is a hole in the receipt.
  assert.deepEqual(
    [...new Set([...routes.matchAll(/ReasoningEffortRefusalReason::([A-Za-z]+)/gu)].map((match) => match[1].replace(/(?<=[a-z0-9])(?=[A-Z])/gu, "_").toLowerCase()))].sort(),
    [...new Set(committed.rows.filter((item) => item.test?.source === routesSource && item.outcome.kind === "refused").map((item) => item.outcome.reason))].sort(),
    "the refusal reasons the route rows assert are the refusal reasons of the manifest's route rows",
  );
  // The live rows: the manifest names their tests, and the rows they print
  // in their observation lines are its rows with observed cells and a test.
  const liveSource = "tests/integration/src/live_reasoning_matrix.rs";
  const live = read(liveSource).toString("utf8");
  const observedRows = committed.rows.filter((item) => item.cells === "observed" && item.test !== null);
  assert.deepEqual(
    observedRows.map((item) => [item.id, item.test.package, item.test.target, item.test.name]),
    [
      ["live.copilot_matrix", "meerkat-integration-tests", "copilot_live_e2e", "copilot_live_reasoning_effort_matrix"],
      ["live.direct_providers", "meerkat-integration-tests", "e2e_live_lane", "direct_provider_live_reasoning_effort"],
    ],
  );
  const lanes = /pub const fn receipt_row\(self\) -> &'static str \{([\s\S]*?)\n {4}\}/u.exec(live);
  assert.ok(lanes !== null, "the live rows name their receipt rows");
  assert.deepEqual(
    [...lanes[1].matchAll(/=> "([a-z0-9_.]+)"/gu)].map((match) => match[1]).sort(),
    observedRows.map((item) => item.id).sort(),
    "the rows the live rows print are the manifest's rows with observed cells and a test",
  );
  assert.ok(live.includes(`pub const RECEIPT_OBSERVATION_PREFIX: &str = "${OBSERVATION_LINE_PREFIX}";`), "the live rows print the prefix the generator reads");
  assert.ok(live.includes(`pub const REDACTED: &str = "${WITHHELD_MODEL_ID}";`), "what the live rows put for a withheld value is the constant the inputs schema takes for a model");
  // The one row with an entry for each level prints its line inside its loop
  // over the levels it refuses, and nowhere else.
  const refusing = "unsupported_level_is_refused_before_any_provider_request";
  assert.deepEqual(
    committed.rows.filter((item) => item.lines === "per_level").map((item) => item.test.name),
    [refusing],
  );
  assert.ok(
    new RegExp(`for level in levels \\{\\s*(?://[^\\n]*\\s*)*param\\.report_cell\\(\\s*"${refusing}",\\s*&fixture\\.model,\\s*level,\\s*\\);`, "u").test(routes),
    "the refusing row prints a line for each level it refuses",
  );
  assert.equal([...routes.matchAll(new RegExp(`report_cell\\(\\s*"${refusing}"`, "gu"))].length, 1);
  // Every route of the matrix has a row that sends a level and a row that refuses one.
  for (const cell of committed.cells) {
    for (const kind of ["sent", "refused"]) {
      assert.ok(
        receipt.entries.some((entry) => entry.cell === cell.id && entry.outcome.kind === kind && entry.test !== null),
        `${cell.id}: a tested row whose outcome is ${kind}`,
      );
    }
  }
});

row("the command line records a run, generates both files from the commit, and verifies them", () => {
  const work = scratchDirectory();
  const repo = join(work, "repo");
  const env = { ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_"))), GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" };
  const git = (...args) => {
    const done = spawnSync("git", ["-C", repo, "-c", "user.name=selftest", "-c", "user.email=selftest@example.invalid", "-c", "commit.gpgsign=false", ...args], { encoding: "utf8", env });
    assert.equal(done.status, 0, `git ${args.join(" ")}: ${done.stderr}`);
    return done.stdout.trim();
  };
  const write = (path, text) => {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), text);
  };
  mkdirSync(repo);
  git("init", "-q");
  write("Cargo.toml", '[workspace.package]\nversion = "1.4.2"\n');
  write(MANIFEST_PATH, serialize(manifest()));
  for (const [path, text] of SOURCES) write(path, text);
  git("add", "-A");
  git("commit", "-q", "-m", "tree");
  const head = git("rev-parse", "HEAD");
  // The working tree moves on; the receipt is of the commit.
  write("Cargo.toml", '[workspace.package]\nversion = "7.7.7"\n');
  write(ROUTES_SOURCE, "// every row deleted\n");

  const results = join(work, "results");
  mkdirSync(results);
  const output = join(work, "nextest.json");
  writeFileSync(output, routesJson());
  const recorded = cli(["record", "--leg", "routes", "--commit", head, "--command", COMMAND, "--exit-status", "100", "--nextest-json", output, "--output", join(results, "routes.json")], { env });
  assert.equal(recorded.status, 0, recorded.stdout + recorded.stderr);
  assert.equal(recorded.stdout, "recorded leg routes: 4 run, 3 passed, 1 failed, 1 skipped, 5 named, 5 cell lines, 0 observations\n");
  assert.equal(readFileSync(join(results, "routes.json"), "utf8"), serialize(routesResult({ commit: head })), "record writes the leg result as built");
  for (const [leg, reason] of [
    ["binary", "rows_not_built"],
    ["live", "credential_unavailable"],
  ]) {
    assert.equal(cli(["record", "--leg", leg, "--not-run", reason, "--output", join(results, `${leg}.json`)], { env }).status, 0);
  }

  const out = join(work, "out");
  const generated = cli(["generate", "--results", results, "--output-dir", out, "--repo", repo], { env });
  assert.equal(generated.status, 0, generated.stdout + generated.stderr);
  assert.equal(generated.stdout, `wrote ${RECEIPT_JSON_NAME} and ${RECEIPT_MARKDOWN_NAME}: 10 entries, 7 executed (4 passed, 3 failed), 1 source only, 2 not run, 5 gaps\n`);
  const json = readFileSync(join(out, RECEIPT_JSON_NAME), "utf8");
  const markdown = readFileSync(join(out, RECEIPT_MARKDOWN_NAME), "utf8");
  const receipt = JSON.parse(json);
  assert.deepEqual(receipt.product, { name: "meerkat", version: "1.4.2", commit: head }, "the version and the test sources are the commit's, not the working tree's");
  assert.equal(markdown, renderMarkdown(receipt));
  for (const text of [json, markdown]) assert.ok(!text.includes(work) && !text.includes(tmpdir()), "no path of the run is written");

  // The same results from another place, into another place: the same bytes.
  const moved = join(scratchDirectory(), "deep", "copy");
  cpSync(results, moved, { recursive: true });
  const again = join(work, "again");
  assert.equal(cli(["generate", "--results", moved, "--output-dir", again, "--repo", repo, "--commit", head], { env, cwd: work }).status, 0);
  assert.equal(readFileSync(join(again, RECEIPT_JSON_NAME), "utf8"), json);
  assert.equal(readFileSync(join(again, RECEIPT_MARKDOWN_NAME), "utf8"), markdown);

  const verified = cli(["verify", join(out, RECEIPT_JSON_NAME), "--markdown", join(out, RECEIPT_MARKDOWN_NAME), "--results", results], { env });
  assert.deepEqual([verified.status, verified.stdout], [0, "the receipt holds\n"]);
  assert.equal(cli(["render", join(out, RECEIPT_JSON_NAME)], { env }).stdout, markdown);

  // --check judges the same receipt: it is written first, whatever the check says.
  const checkedOut = join(work, "checked");
  const checked = cli(["generate", "--results", results, "--output-dir", checkedOut, "--repo", repo, "--commit", head, "--check"], { env });
  assert.equal(checked.status, 3, checked.stdout + checked.stderr);
  assert.deepEqual(checked.stdout.split("\n").slice(1), [
    "check: entry routes.unsupported/direct:none failed",
    "check: entry routes.unsupported/direct:max failed",
    "check: entry routes.unsupported/copilot failed",
    "check: leg binary is a gap of the offline legs: rows_not_built",
    "check: entry binary.supported/direct is a gap of the offline legs: no_test_row",
    "check: named, not held against the receipt: leg live is a gap of the live legs: credential_unavailable",
    "check: named, not held against the receipt: entry live.matrix/- is a gap of the live legs: leg_not_run",
    "check: named, not held against the receipt: entry live.cli/- is a gap of the live legs: no_test_row",
    "check failed: 5 failed entries or gaps of the offline legs",
    "",
  ]);
  assert.equal(readFileSync(join(checkedOut, RECEIPT_JSON_NAME), "utf8"), json, "the receipt a failed check leaves is the receipt");
  assert.equal(readFileSync(join(checkedOut, RECEIPT_MARKDOWN_NAME), "utf8"), markdown);
  const verifiedAndChecked = cli(["verify", join(out, RECEIPT_JSON_NAME), "--check"], { env });
  assert.deepEqual([verifiedAndChecked.status, verifiedAndChecked.stdout.split("\n")[0], verifiedAndChecked.stdout.split("\n").at(-2)], [3, "the receipt holds", "check failed: 5 failed entries or gaps of the offline legs"]);
  // A receipt whose only gaps are live passes it.
  const liveOnly = manifest();
  liveOnly.rows = liveOnly.rows.filter((item) => item.leg === "live");
  liveOnly.legs = liveOnly.legs.filter((leg) => leg.id === "live");
  const liveManifest = join(work, "live-rows.json");
  writeFileSync(liveManifest, serialize(liveOnly));
  const liveResults = join(work, "live-results");
  mkdirSync(liveResults);
  assert.equal(cli(["record", "--leg", "live", "--not-run", "ran_at_another_commit", "--output", join(liveResults, "live.json")], { env }).status, 0, "a leg may be recorded as run at another commit");
  const passed = cli(["generate", "--results", liveResults, "--output-dir", join(work, "live-out"), "--repo", repo, "--commit", head, "--manifest", liveManifest, "--check"], { env });
  assert.deepEqual(
    [passed.status, passed.stdout.split("\n").slice(1)],
    [
      0,
      [
        "check: named, not held against the receipt: leg live is a gap of the live legs: ran_at_another_commit",
        "check: named, not held against the receipt: entry live.matrix/- is a gap of the live legs: leg_not_run",
        "check: named, not held against the receipt: entry live.cli/- is a gap of the live legs: no_test_row",
        "check passed: no failed entry and no gap in the offline legs",
        "",
      ],
    ],
    passed.stdout,
  );
  const liveMarkdown = readFileSync(join(work, "live-out", RECEIPT_MARKDOWN_NAME), "utf8");
  assert.ok(liveMarkdown.includes("| `live` | live | not run here: it ran at another commit |") && liveMarkdown.includes("- Leg `live`: it ran at another commit."));
  // A receipt that does not hold is refused before it is judged.
  writeFileSync(join(checkedOut, RECEIPT_JSON_NAME), json.replace('"schema_version": 1', '"schema_version": 4'));
  assert.equal(cli(["verify", join(checkedOut, RECEIPT_JSON_NAME), "--check"], { env }).status, 1);
  writeFileSync(join(out, RECEIPT_MARKDOWN_NAME), markdown.replace("7 | 4 | 3", "7 | 7 | 0"));
  const edited = cli(["verify", join(out, RECEIPT_JSON_NAME), "--markdown", join(out, RECEIPT_MARKDOWN_NAME)], { env });
  assert.deepEqual([edited.status, edited.stdout], [1, "refused: the Markdown file is not the rendering of the receipt\n"]);

  // Refusals leave no receipt behind and exit 1; a wrong call exits 2.
  const refusal = (args, pattern, what) => {
    const target = join(scratchDirectory(), "out");
    const done = cli([...args, "--output-dir", target, "--repo", repo], { env });
    assert.deepEqual([done.status, pattern.test(done.stdout)], [1, true], `${what}: ${done.stdout}${done.stderr}`);
    assert.equal(existsSync(target), false, `${what}: nothing is written`);
  };
  const missing = join(scratchDirectory(), "results");
  cpSync(results, missing, { recursive: true });
  rmSync(join(missing, "live.json"));
  refusal(["generate", "--results", missing], /^refused: leg live has no result file live\.json/u, "a leg without a result file");
  writeFileSync(join(missing, "live.json"), "{");
  refusal(["generate", "--results", missing], /^refused: the result file live\.json is not JSON/u, "a result file that is not JSON");
  rmSync(join(missing, "live.json"));
  writeFileSync(join(missing, "live.json"), serialize(buildNotRunResult({ leg: "live", reason: "not_scheduled" })));
  writeFileSync(join(missing, "notes.txt"), "kept beside the results\n");
  refusal(["generate", "--results", missing], /^refused: notes\.txt in the results directory is not a leg result file/u, "a stray file in the results directory");
  refusal(["generate", "--results", join(work, "nowhere")], /^refused: the results directory cannot be read/u, "no results directory");
  refusal(["generate", "--results", results, "--commit", "no-such-revision"], /^refused: no-such-revision names no commit/u, "a revision that is none");
  git("commit", "-q", "-am", "the rows are gone");
  refusal(["generate", "--results", results], /^refused: row routes\.supported of the manifest names the test supported_level_is_sent, which/u, "a commit without the row's test");

  const quiet = join(work, "quiet.log");
  writeFileSync(quiet, "error: could not compile\n");
  const unrecorded = join(work, "unrecorded.json");
  const noSummary = cli(["record", "--leg", "routes", "--commit", head, "--command", COMMAND, "--exit-status", "101", "--nextest-log", quiet, "--output", unrecorded], { env });
  assert.deepEqual([noSummary.status, noSummary.stdout], [1, "refused: the nextest log has no summary line\n"]);
  const leaking = cli(["record", "--leg", "routes", "--commit", head, "--command", `bash ${work}/run.sh`, "--exit-status", "100", "--nextest-json", output, "--output", unrecorded], { env });
  assert.deepEqual([leaking.status, leaking.stdout], [1, "refused: the result of leg routes holds an absolute path at $.command\n"]);
  assert.equal(existsSync(unrecorded), false, "a refused record writes nothing");
  for (const args of [[], ["generate"], ["generate", "--results", results], ["generate", "--results", results, "--output-dir", out, "--check", "--check"], ["record", "--leg", "routes", "--output", unrecorded], ["record", "--leg", "routes", "--not-run", "not_scheduled", "--commit", head, "--output", unrecorded], ["record", "--leg", "routes", "--not-run", "not_scheduled", "--check", "--output", unrecorded], ["render"], ["verify"], ["publish"]]) {
    assert.equal(cli(args, { env }).status, 2, `usage: ${args.join(" ")}`);
  }
});

// --- Run -------------------------------------------------------------------

let failed = 0;
for (const { name, body } of rows) {
  try {
    body();
  } catch (error) {
    failed += 1;
    console.log(`FAIL ${name}`);
    console.log(`  ${String(error.message ?? error).split("\n").slice(0, 12).join("\n  ")}`);
  }
}
for (const directory of scratch) rmSync(directory, { recursive: true, force: true });
if (failed > 0) {
  console.log(`reasoning-effort-receipt: ${failed} of ${rows.length} rows failed`);
  process.exit(1);
}
console.log(`reasoning-effort-receipt contracts hold (${rows.length} rows)`);
