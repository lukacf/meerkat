#!/usr/bin/env node
// Self-test for scripts/ci-product-evidence.mjs: the product rule each
// release gate applies to a failed exact-main CI run, the record the CI gate
// writes, the artifact reader, and the tables that join ci.yml's jobs and
// the gate's lane names to products, checked against ci.yml itself. No
// network.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateRawSync } from "node:zlib";

import {
  EVIDENCE_INPUTS,
  EVIDENCE_VERSION,
  GATE_JOB,
  GATE_LANES,
  LANE_JOBS,
  PUBLISHED_FLOOR,
  SINGLE_JOBS,
  TLC_AUDITS,
  acceptance,
  attestationOfRun,
  listAttemptJobs,
  readZipEntry,
  recordEvidence,
} from "./ci-product-evidence.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const workflow = readFileSync(resolve(here, "../.github/workflows/ci.yml"), "utf8");

// --- The tables against ci.yml -------------------------------------------

// Each top-level job: its key, its `name:` and its whole block.
const jobs = new Map();
{
  const lines = workflow.split("\n");
  const start = lines.indexOf("jobs:");
  let key = null;
  let block = [];
  for (const line of lines.slice(start + 1)) {
    const header = /^  ([A-Za-z0-9_-]+):\s*$/.exec(line);
    if (header) {
      if (key) jobs.set(key, block.join("\n"));
      key = header[1];
      block = [];
    } else if (key) {
      block.push(line);
    }
  }
  if (key) jobs.set(key, block.join("\n"));
}
const nameOf = (key) => /^ {4}name: (.+)$/m.exec(jobs.get(key) ?? "")?.[1];

// Each planned lane: the jobs that run its rows render the template's name
// and read the planner's matrix of that lane.
const LANE_MATRICES = {
  clippy: ["shard_matrix", "shard_rest_matrix"],
  unit: ["unit_shard_matrix"],
  main_unit: ["main_unit_shard_matrix", "main_unit_rest_shard_matrix"],
  main_archive_build: ["main_archive_build_matrix"],
  main_archive_run: ["main_archive_run_matrix"],
  integration: ["integration_matrix"],
  closure_tests: ["closure_test_matrix", "closure_test_rest_matrix"],
  feature_check: ["feature_check_matrix", "feature_check_rest_matrix"],
  closure_check: ["closure_check_matrix"],
  rustdoc: ["rustdoc_matrix"],
};
const placeholderRow = { name: "${{ matrix.name }}", features: "${{ matrix.features }}" };
const covered = new Set([ "gate" ]);
assert.deepEqual(Object.keys(LANE_JOBS).sort(), Object.keys(LANE_MATRICES).sort(), "every planned lane has its matrices");
for (const [lane, spec] of Object.entries(LANE_JOBS)) {
  spec.jobs.forEach((key, index) => {
    assert.ok(jobs.has(key), `ci.yml has the ${key} job of lane ${lane}`);
    assert.equal(nameOf(key), spec.name(placeholderRow), `ci.yml's ${key} job renders lane ${lane}'s name`);
    const matrix = LANE_MATRICES[lane][Math.min(index, LANE_MATRICES[lane].length - 1)];
    assert.ok(jobs.get(key).includes(`needs.changes.outputs.${matrix}`), `ci.yml's ${key} job reads ${matrix}`);
    covered.add(key);
  });
}
assert.equal(GATE_JOB, nameOf("gate"), "the gate job's name");
// Every other job is a single job of known product, a TLC audit part or a
// published-floor row: a new job needs a product before it can count.
for (const [key] of jobs) {
  if (covered.has(key)) continue;
  const name = nameOf(key);
  const known =
    Object.hasOwn(SINGLE_JOBS, name) ||
    TLC_AUDITS.test(name.replace("${{ matrix.part }}", "audits-a")) ||
    PUBLISHED_FLOOR.test(name.replace("${{ matrix.product }}", "meta"));
  assert.ok(known, `ci.yml job ${key} (${name}) has no product in scripts/ci-product-evidence.mjs`);
}
for (const name of Object.keys(SINGLE_JOBS)) {
  assert.ok([...jobs.keys()].some((key) => nameOf(key) === name), `SINGLE_JOBS names ${name}, which is no ci.yml job`);
}
// Each single job's product, pinned: moving a job to another product changes
// which releases its failure can block, so it changes this list too.
assert.deepEqual(
  SINGLE_JOBS,
  {
    "Change classification": "core",
    "Stagger the short rows": "core",
    "Format + governance": "core",
    "CI lane classifier selftest": "core",
    "Path classifier selftest": "core",
    "Generation ratchets": "core",
    "Typed governance gates": "core",
    "Typed governance gates (machine-authority docs)": "core",
    "Governed JSONL": "core",
    "MCP confinement positive (Linux)": "core",
    "wasm32 check": "core",
    "Python + TypeScript SDK suites": "core",
    "Example web suites": "core",
    "WASM timer ownership": "core",
    "Bazel graph check": "core",
    "Connect packaging, host adapters, SDK, Console panel and docs": "connect",
  },
  "every single job's product",
);
{
  const evidence = recordEvidence({
    rowProducts: {},
    jobs: [
      { name: "Bounded TLC audits (audits-a)", conclusion: "success" },
      { name: "Published floor (core)", conclusion: "success" },
      { name: "Published floor (meta)", conclusion: "success" },
      { name: "Published floor (connect)", conclusion: "success" },
    ],
    failedLanes: [],
    planErrors: 0,
  });
  assert.deepEqual(
    evidence.jobs.map((entry) => [entry.name, entry.product]),
    [
      ["Bounded TLC audits (audits-a)", "core"],
      ["Published floor (core)", "core"],
      ["Published floor (meta)", "meta"],
      ["Published floor (connect)", "connect"],
    ],
    "a TLC audit part is Core's and a published-floor row its product's",
  );
}
// Every lane the gate's lane check names judges jobs that exist.
const gateBlock = jobs.get("gate");
const checked = new Set(
  [...gateBlock.matchAll(/(?:require_success|require_ran|forbid_failure) "([^"$][^"]*)"/g)].map((match) => match[1]),
);
for (const match of gateBlock.matchAll(/for lane in ((?:"?[A-Za-z0-9 +()-]+"?:"\$\{[A-Z_]+\}" ?)+); do/g)) {
  for (const entry of match[1].matchAll(/"?([A-Za-z0-9 +()-]+?)"?:"\$\{/g)) checked.add(entry[1].trim());
}
assert.ok(checked.size >= 20, `the lane check names its lanes (${checked.size} found)`);
for (const lane of checked) {
  assert.ok(Object.hasOwn(GATE_LANES, lane), `the gate's lane ${lane} is not in GATE_LANES`);
}
const jobNames = [...jobs.keys()].map(nameOf);
for (const [lane, spec] of Object.entries(GATE_LANES)) {
  for (const exact of spec.exact ?? []) assert.ok(jobNames.includes(exact), `GATE_LANES ${lane}: no job is named ${exact}`);
  for (const prefix of spec.prefix ?? []) {
    assert.ok(jobNames.some((name) => name.startsWith(prefix)), `GATE_LANES ${lane}: no job name starts with ${prefix}`);
  }
}
// The lane check records what the evidence reads.
for (const needle of ["flagged_lanes+=(", "plan_errors=$((plan_errors + 1))", "failed_lanes=", "plan_errors="]) {
  assert.ok(gateBlock.includes(needle), `the gate's lane check records ${needle}`);
}
// The attestation is green only when every gate step that can fail the gate
// before it succeeded, the budget step included.
assert.ok(gateBlock.includes("        id: budget\n"), "the budget step has an id the attestation reads");
assert.ok(gateBlock.includes("          BUDGET_OUTCOME: ${{ steps.budget.outcome }}"), "the attestation reads the budget step");
assert.match(
  gateBlock,
  /if \[\[ "\$\{LANES_OUTCOME\}" == "success" && .*"\$\{BUDGET_OUTCOME\}" == "success" \]\]; then\n\s+validation_result=success/,
  "validation_result is success only when the lane check and the budget step succeeded",
);
// The record lists the attempt's jobs 30 a page, retrying a server error as
// the budget step does, and knows the plan's count for each lane wave: every
// count it reads is the one that lane job's `if:` reads.
assert.ok(gateBlock.includes("          API_RETRY_DELAYS: 5 10 20\n"), "the record retries a server error");
const waveCounts = /          WAVE_COUNTS: >-\n((?:            .*\n)+)/.exec(gateBlock);
assert.ok(waveCounts, "the record step names the plan's wave counts");
const counted = [...waveCounts[1].matchAll(/"([a-z-]+)": "\$\{\{ needs\.changes\.outputs\.([a-z_]+) \}\}"/g)];
const countedJobs = Object.values(LANE_JOBS).flatMap((spec) => spec.jobs).filter((key) => !["closure-check", "rustdoc"].includes(key));
assert.deepEqual(counted.map((match) => match[1]).sort(), [...countedJobs].sort(), "a count for every lane job that shares a lane's name");
for (const [, key, output] of counted) {
  assert.match(jobs.get(key) ?? "", new RegExp(`^    if: .*needs\\.changes\\.outputs\\.${output} != '0'`, "m"), `${key} runs on ${output}`);
}
JSON.parse(waveCounts[1].replace(/\$\{\{[^}]*\}\}/g, "0").replaceAll("\n", " "));

// --- The rule --------------------------------------------------------------

const job = (name, conclusion, product, downstream = false) => ({ name, conclusion, product, downstream });
const GREEN = [
  job("Change classification", "success", "core"),
  job("Format + governance", "success", "core"),
  job("Clippy (core)", "success", "core"),
  job("Clippy (downstream meta)", "success", "meta", true),
  job("Clippy (meta)", "success", "meta"),
  job("Main unit tests (core)", "success", "core"),
  job("Main unit tests (meta)", "success", "meta"),
  job("Main unit tests (connect[console])", "success", "connect"),
  job("Connect packaging, host adapters, SDK, Console panel and docs", "success", "connect"),
  job("Unit tests (none)", "skipped", null),
];
function attestation({ jobs: list = GREEN, failedLanes = [], planErrors = 0, result = "failure", mode = "workspace", version = EVIDENCE_VERSION } = {}) {
  return {
    schema_version: 4,
    validation_result: result,
    plan: { mode },
    product_evidence: { version, plan_errors: planErrors, failed_lanes: failedLanes, jobs: list },
  };
}
const fail = (name, conclusion = "failure") => GREEN.map((entry) => (entry.name === name ? { ...entry, conclusion } : entry));
// The judged commit changed none of the evidence's inputs (see below).
const verdict = (record, product) => acceptance(record, product, { changedPaths: ["crates/meerkat-core/src/lib.rs"] });
const expectRule = (label, record, expected) => {
  for (const [product, accepted] of Object.entries(expected)) {
    const { accepted: actual, reasons } = verdict(record, product);
    assert.equal(actual, accepted, `${label}: ${product} ${accepted ? "accepts" : "refuses"} (${reasons.join("; ")})`);
  }
};

expectRule(
  "a failed run whose attestation says every lane succeeded",
  { schema_version: 4, validation_result: "success" },
  { core: false, meta: false, connect: false },
);
expectRule(
  "a Connect row red (0.9.11)",
  attestation({ jobs: fail("Main unit tests (connect[console])"), failedLanes: ["Main unit tests"] }),
  { core: true, meta: true, connect: false },
);
expectRule(
  "Connect's checks red",
  attestation({
    jobs: fail("Connect packaging, host adapters, SDK, Console panel and docs"),
    failedLanes: ["Connect packaging, Console panel and docs"],
  }),
  { core: true, meta: true, connect: false },
);
expectRule(
  "a Meta row red",
  attestation({ jobs: fail("Main unit tests (meta)"), failedLanes: ["Main unit tests"] }),
  { core: true, meta: false, connect: false },
);
expectRule(
  "a downstream type-check row red",
  attestation({ jobs: fail("Clippy (downstream meta)"), failedLanes: ["Clippy"] }),
  { core: false, meta: false, connect: false },
);
expectRule(
  "a Core row red",
  attestation({ jobs: fail("Main unit tests (core)"), failedLanes: ["Main unit tests"] }),
  { core: false, meta: false, connect: false },
);
expectRule(
  "a Core row cancelled",
  attestation({ jobs: fail("Clippy (core)", "cancelled"), failedLanes: ["Clippy"] }),
  { core: false, meta: false },
);
expectRule(
  "a repository-wide job red",
  attestation({ jobs: fail("Format + governance"), failedLanes: ["Format + governance"] }),
  { core: false, meta: false },
);
const connectRed = fail("Main unit tests (connect[console])");
expectRule(
  "a job that ran with no product",
  attestation({ jobs: [...connectRed, job("Mystery lane", "success", null)], failedLanes: ["Main unit tests"] }),
  { core: false, meta: false },
);
expectRule(
  "a job with an unknown product",
  attestation({ jobs: [...connectRed, job("Clippy (other)", "success", "other")], failedLanes: ["Main unit tests"] }),
  { core: false, meta: false },
);
expectRule(
  "a flagged lane without a failed job",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests", "Bazel graph check"] }),
  { core: false, meta: false },
);
expectRule(
  "a flagged lane the table does not know",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests", "New lane"] }),
  { core: false, meta: false },
);
expectRule(
  "a Core lane skipped while the gate required it",
  attestation({
    jobs: [...connectRed, job("Python + TypeScript SDK suites", "skipped", "core")],
    failedLanes: ["Main unit tests", "Python + TypeScript SDK suites"],
  }),
  { core: false, meta: false },
);
expectRule(
  "a plan error",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests"], planErrors: 1 }),
  { core: false, meta: false },
);
expectRule(
  "an unknown plan-error count",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests"], planErrors: null }),
  { core: false, meta: false },
);
expectRule(
  "a release-projection plan",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests"], mode: "release-projection" }),
  { core: false, meta: false },
);
expectRule(
  "no flagged lane",
  attestation({ jobs: connectRed, failedLanes: [] }),
  { core: false, meta: false },
);
expectRule(
  "another evidence version",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests"], version: EVIDENCE_VERSION + 1 }),
  { core: false, meta: false },
);
expectRule(
  "no product evidence",
  { schema_version: 4, validation_result: "failure" },
  { core: false, meta: false, connect: false },
);
expectRule(
  "a result that is neither",
  attestation({ jobs: connectRed, failedLanes: ["Main unit tests"], result: "unknown" }),
  { core: false, meta: false },
);
assert.equal(verdict(attestation(), "other").accepted, false, "an unknown product refuses");
assert.match(
  verdict(attestation({ jobs: connectRed, failedLanes: ["Main unit tests"] }), "core").reasons.join(" "),
  /Main unit tests \(connect\[console\]\) \(connect, failure\)/,
  "an accepted run names the failed rows",
);

// --- The judge ----------------------------------------------------------------

// A run of commit R is judged by R's first parent's copy of this module (the
// release gates read it with `git show`), as reused CI evidence is: R cannot
// certify its own red run with an edited job-to-product table. The copy is
// told what R changed against that parent and refuses when R changed what
// decides the evidence (this module, the planner's row products, the
// products file, ci.yml's gate), or when nobody told it.
{
  const red = attestation({ jobs: connectRed, failedLanes: ["Main unit tests"] });
  assert.deepEqual(
    [...EVIDENCE_INPUTS].sort(),
    [".github/workflows/ci.yml", "ci/products.json", "scripts/ci-cargo-lanes.mjs", "scripts/ci-product-evidence.mjs", "scripts/ci-products.mjs"],
    "the inputs that decide a run's product evidence",
  );
  assert.equal(acceptance(red, "core", { changedPaths: [] }).accepted, true, "a commit that changed nothing the evidence reads");
  assert.equal(acceptance(red, "meta", { changedPaths: ["meta/README.md"] }).accepted, true, "a commit that changed other files");
  for (const options of [undefined, {}, { changedPaths: null }, { changedPaths: "scripts/x.mjs" }]) {
    const { accepted, reasons } = acceptance(red, "core", options);
    assert.equal(accepted, false, `unknown changes refuse (${JSON.stringify(options)})`);
    assert.match(reasons.join(" "), /changes against its first parent are unknown/);
  }
  for (const input of EVIDENCE_INPUTS) {
    for (const product of ["core", "meta"]) {
      const { accepted, reasons } = acceptance(red, product, { changedPaths: ["crates/meerkat-core/src/lib.rs", input] });
      assert.equal(accepted, false, `a commit that changes ${input} refuses ${product}`);
      assert.match(reasons.join(" "), new RegExp(`changes ${input.replaceAll(".", "\\.")}`));
    }
  }
}

// --- The record -------------------------------------------------------------

{
  const rowProducts = {
    clippy: [{ name: "core", product: "core" }, { name: "downstream meta", product: "meta", downstream: true }],
    main_unit: [{ name: "core", product: "core" }, { name: "connect[console]", product: "connect" }],
    rustdoc: [{ name: "meta, 2 crates", features: "all", product: "meta" }],
    closure_tests: [{ name: "tests odd", product: null }],
    retired_lane: [{ name: "x", product: "core" }],
  };
  const listed = [
    { name: "Clippy (core)", conclusion: "success" },
    { name: "Clippy (downstream meta)", conclusion: "failure" },
    { name: "Main unit tests (connect[console])", conclusion: "failure" },
    { name: "Rustdoc (meta, 2 crates, all features)", conclusion: "success" },
    { name: "Closure tests (tests odd)", conclusion: "success" },
    { name: "Bounded TLC audits (audits-q)", conclusion: "success" },
    { name: "Published floor (connect)", conclusion: "success" },
    { name: "Published floor (other)", conclusion: "success" },
    { name: "Connect packaging, host adapters, SDK, Console panel and docs", conclusion: "success" },
    { name: "Unknown job", conclusion: null },
    { name: GATE_JOB, conclusion: null },
  ];
  const evidence = recordEvidence({ rowProducts, jobs: listed, failedLanes: ["Clippy"], planErrors: 0 });
  assert.equal(evidence.version, EVIDENCE_VERSION);
  assert.deepEqual(evidence.failed_lanes, ["Clippy"]);
  assert.equal(evidence.plan_errors, 0);
  const of = Object.fromEntries(evidence.jobs.map((entry) => [entry.name, entry]));
  assert.ok(!(GATE_JOB in of), "the gate is never a row");
  assert.deepEqual(of["Clippy (core)"], job("Clippy (core)", "success", "core"));
  assert.deepEqual(of["Clippy (downstream meta)"], job("Clippy (downstream meta)", "failure", "meta", true));
  assert.equal(of["Main unit tests (connect[console])"].product, "connect");
  assert.equal(of["Rustdoc (meta, 2 crates, all features)"].product, "meta");
  assert.equal(of["Closure tests (tests odd)"].product, null, "a row with no product stays unknown");
  assert.equal(of["Bounded TLC audits (audits-q)"].product, "core");
  assert.equal(of["Published floor (connect)"].product, "connect");
  assert.equal(of["Published floor (other)"].product, null, "a floor row of no product stays unknown");
  assert.equal(of["Connect packaging, host adapters, SDK, Console panel and docs"].product, "connect");
  assert.deepEqual(of["Unknown job"], job("Unknown job", "pending", null));
  const recorded = acceptance({ validation_result: "failure", plan: { mode: "workspace" }, product_evidence: evidence }, "core", { changedPaths: [] });
  assert.equal(recorded.accepted, false, "a recorded downstream failure refuses Core");
  assert.match(recorded.reasons.join(" "), /core depends on job Clippy \(downstream meta\)/, "for that failure");
}

// --- Unplanned waves ----------------------------------------------------------

// GitHub lists a lane job its `if:` skipped under the job's unexpanded name
// (run 42615640098: `Clippy (${{ matrix.name }})`). The record marks it an
// unplanned wave when the plan's count for one of the jobs that render that
// name is 0, so a red row of a product no release depends on, next to an
// empty rest wave, is still judged; with no zero count it stays unknown.
{
  const rowProducts = {
    closure_tests: [{ name: "tests connect 1/2", product: "connect" }, { name: "tests core 1/9", product: "core" }],
  };
  const listed = [
    { name: "Change classification", conclusion: "success" },
    { name: "Closure tests (tests connect 1/2)", conclusion: "failure" },
    { name: "Closure tests (tests core 1/9)", conclusion: "success" },
    { name: "Closure tests (${{ matrix.name }})", conclusion: "skipped" },
  ];
  const record = (waveCounts) =>
    recordEvidence({ rowProducts, jobs: listed, failedLanes: ["Closure tests"], planErrors: 0, waveCounts });
  const restEmpty = record({ "closure-tests": 2, "closure-tests-rest": 0 });
  const skipped = restEmpty.jobs.find((entry) => entry.name.includes("${{"));
  assert.deepEqual(skipped, { name: "Closure tests (${{ matrix.name }})", conclusion: "skipped", product: null, downstream: false, unplanned: true });
  const judged = (evidence) =>
    Object.fromEntries(
      ["core", "meta"].map((product) => [
        product,
        acceptance({ validation_result: "failure", plan: { mode: "workspace" }, product_evidence: evidence }, product, { changedPaths: [] }).accepted,
      ]),
    );
  assert.deepEqual(judged(restEmpty), { core: true, meta: true }, "a Connect row red beside an empty rest wave");
  for (const counts of [{ "closure-tests": 2, "closure-tests-rest": 1 }, {}, { "closure-tests": Number.NaN, "closure-tests-rest": Number.NaN }]) {
    const evidence = record(counts);
    assert.equal(evidence.jobs.find((entry) => entry.name.includes("${{")).unplanned, undefined, `no zero count: ${JSON.stringify(counts)}`);
    assert.deepEqual(judged(evidence), { core: false, meta: false }, "an unknown skipped job in a flagged lane refuses");
  }
  // Only a skipped job is a wave the plan did not run.
  const ran = recordEvidence({
    rowProducts,
    jobs: [{ name: "Closure tests (${{ matrix.name }})", conclusion: "failure" }],
    failedLanes: ["Closure tests"],
    planErrors: 0,
    waveCounts: { "closure-tests-rest": 0 },
  });
  assert.equal(ran.jobs[0].unplanned, undefined, "a job that ran is never an unplanned wave");
  // Two jobs render "Main unit tests (...)": either one's zero count counts.
  const archive = recordEvidence({
    rowProducts: {},
    jobs: [{ name: "Main unit tests (${{ matrix.name }})", conclusion: "skipped" }],
    failedLanes: [],
    planErrors: 0,
    waveCounts: { "main-unit": 3, "main-unit-rest": 2, "main-unit-archive-run": 0 },
  });
  assert.equal(archive.jobs[0].unplanned, true, "the archived run lane's empty wave");
}

// --- The job listing -----------------------------------------------------------

{
  const calls = [];
  const sleeps = [];
  const answers = (list) => (args) => {
    calls.push(args);
    return list.shift();
  };
  const ok = { status: 0, stdout: '{"name":"Clippy (core)","conclusion":"success"}\n{"name":"CI gate","conclusion":null}\n', stderr: "" };
  const listed = listAttemptJobs({
    repository: "o/r",
    runId: 42,
    attempt: 2,
    delays: [5, 10, 20],
    run: answers([{ status: 1, stdout: "", stderr: "gh: HTTP 502: Bad Gateway" }, { status: 1, stdout: "", stderr: "HTTP 503" }, ok]),
    sleep: (seconds) => sleeps.push(seconds),
  });
  assert.deepEqual(listed, [{ name: "Clippy (core)", conclusion: "success" }, { name: "CI gate", conclusion: null }]);
  assert.deepEqual(sleeps, [5, 10], "a server error is retried after each delay");
  assert.deepEqual(calls[0], ["api", "--paginate", "repos/o/r/actions/runs/42/attempts/2/jobs?per_page=30", "--jq", ".jobs[] | {name, conclusion}"]);
  assert.throws(
    () => listAttemptJobs({ repository: "o/r", runId: 42, attempt: 2, delays: [5], run: answers([{ status: 1, stdout: "", stderr: "HTTP 404: Not Found" }]), sleep: () => {} }),
    /HTTP 404/,
    "any other failure fails at once",
  );
  assert.throws(
    () =>
      listAttemptJobs({
        repository: "o/r",
        runId: 42,
        attempt: 2,
        delays: [5],
        run: answers([{ status: 1, stdout: "", stderr: "HTTP 502" }, { status: 1, stdout: "", stderr: "HTTP 502" }]),
        sleep: () => {},
      }),
    /HTTP 502/,
    "a server error after the last retry fails",
  );
}

// --- The artifact reader ---------------------------------------------------

// A zip archive of the given files, each stored or deflated.
function zip(files) {
  const locals = [];
  const central = [];
  let offset = 0;
  for (const [name, text, method] of files) {
    const raw = Buffer.from(text);
    const body = method === 8 ? deflateRawSync(raw) : raw;
    const nameBytes = Buffer.from(name);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(method, 8);
    local.writeUInt32LE(body.length, 18);
    local.writeUInt32LE(raw.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    const header = Buffer.alloc(46);
    header.writeUInt32LE(0x02014b50, 0);
    header.writeUInt16LE(20, 4);
    header.writeUInt16LE(20, 6);
    header.writeUInt16LE(method, 10);
    header.writeUInt32LE(body.length, 20);
    header.writeUInt32LE(raw.length, 24);
    header.writeUInt16LE(nameBytes.length, 28);
    header.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, body);
    central.push(header, nameBytes);
    offset += local.length + nameBytes.length + body.length;
  }
  const directory = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}
{
  const record = JSON.stringify(attestation());
  for (const method of [0, 8]) {
    const archive = zip([["other.txt", "x", 0], ["attestation.json", record, method]]);
    assert.equal(readZipEntry(archive, "attestation.json").toString("utf8"), record, `zip method ${method}`);
    assert.equal(readZipEntry(archive.buffer.slice(archive.byteOffset, archive.byteOffset + archive.length), "attestation.json").toString("utf8"), record, "an ArrayBuffer download");
  }
  assert.throws(() => readZipEntry(zip([["other.txt", "x", 0]]), "attestation.json"), /not in the archive/);
  assert.throws(() => readZipEntry(Buffer.from("not a zip archive at all, not even close"), "attestation.json"), /not a zip archive/);
}

// --- The artifact lookup ---------------------------------------------------

{
  const commit = "0123456789abcdef0123456789abcdef01234567";
  const record = attestation({ jobs: fail("Main unit tests (connect[console])"), failedLanes: ["Main unit tests"] });
  const calls = [];
  const github = (artifacts) => ({
    rest: {
      actions: {
        listWorkflowRunArtifacts: async (params) => {
          calls.push(["list", params]);
          return { data: { artifacts } };
        },
        downloadArtifact: async (params) => {
          calls.push(["download", params]);
          return { data: zip([["attestation.json", JSON.stringify(record), 8]]) };
        },
      },
    },
  });
  const name = `meerkat-ci-attestation-${commit}-2`;
  const found = await attestationOfRun(github([{ id: 7, name, expired: false }]), { owner: "o", repo: "r", runId: 42, attempt: 2, commit });
  assert.deepEqual(found, record, "the attempt's attestation");
  assert.deepEqual(calls[0], ["list", { owner: "o", repo: "r", run_id: 42, name, per_page: 100 }]);
  assert.deepEqual(calls[1], ["download", { owner: "o", repo: "r", artifact_id: 7, archive_format: "zip" }]);
  assert.equal(
    await attestationOfRun(github([{ id: 7, name, expired: true }]), { owner: "o", repo: "r", runId: 42, attempt: 2, commit }),
    null,
    "an expired attestation is none",
  );
  assert.equal(
    await attestationOfRun(github([{ id: 8, name: `meerkat-ci-attestation-${commit}-1`, expired: false }]), {
      owner: "o",
      repo: "r",
      runId: 42,
      attempt: 2,
      commit,
    }),
    null,
    "another attempt's attestation is none",
  );
}

console.log("ci-product-evidence contracts hold");
