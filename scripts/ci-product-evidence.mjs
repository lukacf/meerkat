#!/usr/bin/env node
// Product-scoped CI evidence.
//
// ci.yml's gate records, on every push run to main or release/** that is not
// cancelled, which product each job of the run attempt belongs to and how it
// concluded (`record`), in the exact-tree attestation's `product_evidence`.
// A release of one product then accepts that run when every failure in it
// sits in rows the product does not depend on (`acceptance`):
//   - core: Core's rows and the downstream type-check rows (rows the planner
//     marks downstream: a pull request's `downstream <product>` Clippy rows
//     and, when the plan reaches Core, the products' closure-check rows);
//   - meta: Core's and Meta's rows;
//   - connect: every row, i.e. only a run whose validation_result is success.
// A run whose validation_result is success is accepted for every product, as
// before. Anything the rule cannot attribute refuses, which leaves the
// release on the whole-run requirement: no product_evidence or another
// version, a plan-level error in the gate, a release-projection plan, a job
// that ran without a known product, a lane the gate flagged without a failed
// job, or a failed job the product depends on.
//
// Products of jobs: a matrix row's from the planner's `row_products` output
// (scripts/ci-cargo-lanes.mjs), joined to the job by the lane's display name
// template below; a single job's from SINGLE_JOBS. Repository-wide single
// jobs are Core's (Core owns the root), so every product requires them.
//
// Usage:
//   ci-product-evidence.mjs record --output FILE [--jobs FILE]
//       env ROW_PRODUCTS, FAILED_LANES (JSON), PLAN_ERRORS; without --jobs it
//       lists GITHUB_RUN_ID's attempt GITHUB_RUN_ATTEMPT through `gh api`.
//   ci-product-evidence.mjs accepts --product core|meta|connect ATTESTATION
//       exit 0 accepted, 1 refused (the reasons are printed), 2 usage.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { inflateRawSync } from "node:zlib";

export const EVIDENCE_VERSION = 1;
export const PRODUCTS = ["core", "meta", "connect"];
export const GATE_JOB = "CI gate";

// Each planned lane of row_products: the ci.yml jobs that run its rows and the
// display name GitHub gives a row's job (the jobs' `name:` with the matrix
// filled in). scripts/ci-product-evidence-selftest.mjs checks both against
// ci.yml.
export const LANE_JOBS = {
  clippy: { jobs: ["clippy", "clippy-rest"], name: (row) => `Clippy (${row.name})` },
  unit: { jobs: ["unit"], name: (row) => `Unit tests (${row.name})` },
  main_unit: { jobs: ["main-unit", "main-unit-rest"], name: (row) => `Main unit tests (${row.name})` },
  main_archive_build: { jobs: ["main-unit-archive-build"], name: (row) => `Main unit build (${row.name})` },
  main_archive_run: { jobs: ["main-unit-archive-run"], name: (row) => `Main unit tests (${row.name})` },
  integration: { jobs: ["integration"], name: (row) => `Integration tests (${row.name})` },
  closure_tests: { jobs: ["closure-tests", "closure-tests-rest"], name: (row) => `Closure tests (${row.name})` },
  feature_check: { jobs: ["feature-check", "feature-check-rest"], name: (row) => `Feature combinations (${row.name})` },
  closure_check: { jobs: ["closure-check"], name: (row) => `Closure check (${row.name})` },
  rustdoc: { jobs: ["rustdoc"], name: (row) => `Rustdoc (${row.name}, ${row.features} features)` },
};

// Jobs without a planned matrix, by display name. TLC audit parts are Core's;
// a published-floor row is its product's.
export const SINGLE_JOBS = {
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
};
export const TLC_AUDITS = /^Bounded TLC audits \((.+)\)$/;
export const PUBLISHED_FLOOR = /^Published floor \((.+)\)$/;

// The lanes the gate's lane check names (require_success, require_ran,
// forbid_failure) and the jobs each one judges.
export const GATE_LANES = {
  "Change classification": { exact: ["Change classification"] },
  "Format + governance": { exact: ["Format + governance"] },
  "CI lane classifier selftest": { exact: ["CI lane classifier selftest"] },
  "Path classifier selftest": { exact: ["Path classifier selftest"] },
  "Generation ratchets": { exact: ["Generation ratchets"] },
  "Stagger the short rows": { exact: ["Stagger the short rows"] },
  "Bounded TLC audits": { prefix: ["Bounded TLC audits ("] },
  "Typed governance gates": { exact: ["Typed governance gates"] },
  "Typed governance gates (machine-authority docs)": { exact: ["Typed governance gates (machine-authority docs)"] },
  Clippy: { prefix: ["Clippy ("] },
  "Unit tests": { prefix: ["Unit tests ("] },
  "Closure check": { prefix: ["Closure check ("] },
  Rustdoc: { prefix: ["Rustdoc ("] },
  "Governed JSONL": { exact: ["Governed JSONL"] },
  "MCP confinement positive": { exact: ["MCP confinement positive (Linux)"] },
  "Feature combinations": { prefix: ["Feature combinations ("] },
  "Main unit tests (packed shards)": { prefix: ["Main unit tests ("] },
  "Main unit tests": { prefix: ["Main unit tests (", "Main unit build ("] },
  "wasm32 check": { exact: ["wasm32 check"] },
  "Python + TypeScript SDK suites": { exact: ["Python + TypeScript SDK suites"] },
  "Python + TypeScript SDK suites (example suites)": { exact: ["Python + TypeScript SDK suites"] },
  "Integration tests": { prefix: ["Integration tests ("] },
  "Closure tests": { prefix: ["Closure tests ("] },
  "Example web suites": { exact: ["Example web suites"] },
  "WASM timer ownership": { exact: ["WASM timer ownership"] },
  "Bazel graph check": { exact: ["Bazel graph check"] },
  "Published floor": { prefix: ["Published floor ("] },
  "Connect packaging, Console panel and docs": { exact: ["Connect packaging, host adapters, SDK, Console panel and docs"] },
};

export function requires(product, job) {
  if (product === "core") return job.product === "core" || job.downstream === true;
  if (product === "meta") return job.product === "core" || job.product === "meta";
  return true;
}

function singleJob(name) {
  if (Object.hasOwn(SINGLE_JOBS, name)) return { product: SINGLE_JOBS[name], downstream: false };
  if (TLC_AUDITS.test(name)) return { product: "core", downstream: false };
  const floor = PUBLISHED_FLOOR.exec(name);
  if (floor && PRODUCTS.includes(floor[1])) return { product: floor[1], downstream: false };
  return null;
}

// The product_evidence of one run attempt: every job but the gate, with its
// conclusion and its product (null when nothing names it).
export function recordEvidence({ rowProducts, jobs, failedLanes, planErrors }) {
  const planned = new Map();
  for (const [lane, rows] of Object.entries(rowProducts ?? {})) {
    const spec = LANE_JOBS[lane];
    if (!spec || !Array.isArray(rows)) continue;
    for (const row of rows) {
      const product = PRODUCTS.includes(row?.product) ? row.product : null;
      planned.set(spec.name(row), { product, downstream: row?.downstream === true });
    }
  }
  return {
    version: EVIDENCE_VERSION,
    plan_errors: planErrors,
    failed_lanes: failedLanes,
    jobs: jobs
      .filter((job) => job.name !== GATE_JOB)
      .map((job) => {
        const known = planned.get(job.name) ?? singleJob(job.name);
        return {
          name: job.name,
          conclusion: job.conclusion ?? "pending",
          product: known?.product ?? null,
          downstream: known?.downstream ?? false,
        };
      }),
  };
}

function laneJobs(lane, jobs) {
  const spec = GATE_LANES[lane];
  if (!spec) return null;
  return jobs.filter(
    (job) => (spec.exact ?? []).includes(job.name) || (spec.prefix ?? []).some((prefix) => job.name.startsWith(prefix)),
  );
}

// Whether `attestation` lets a release of `product` proceed, and why.
export function acceptance(attestation, product) {
  if (!PRODUCTS.includes(product)) return { accepted: false, reasons: [`unknown product ${product}`] };
  if (attestation?.validation_result === "success") return { accepted: true, reasons: ["every lane succeeded"] };
  const reasons = [];
  if (product === "connect") return { accepted: false, reasons: ["Connect depends on every row"] };
  const evidence = attestation?.product_evidence;
  if (!evidence || evidence.version !== EVIDENCE_VERSION) {
    return { accepted: false, reasons: [`no product evidence of version ${EVIDENCE_VERSION}`] };
  }
  if (attestation?.validation_result !== "failure") reasons.push(`validation_result ${attestation?.validation_result}`);
  if (attestation?.plan?.mode === "release-projection") reasons.push("a release-projection plan is never product-scoped");
  if (evidence.plan_errors !== 0) reasons.push(`the gate found ${evidence.plan_errors} plan error(s)`);
  const jobs = Array.isArray(evidence.jobs) ? evidence.jobs : [];
  const failedLanes = Array.isArray(evidence.failed_lanes) ? evidence.failed_lanes : null;
  if (jobs.length === 0) reasons.push("no jobs recorded");
  if (failedLanes === null || failedLanes.length === 0) reasons.push("no failed lane recorded");
  for (const job of jobs) {
    if (job.conclusion !== "skipped" && !PRODUCTS.includes(job.product)) {
      reasons.push(`job ${job.name} ran (${job.conclusion}) with no known product`);
    } else if (PRODUCTS.includes(job.product) && requires(product, job) && !["success", "skipped"].includes(job.conclusion)) {
      reasons.push(`${product} depends on job ${job.name} (${job.conclusion})`);
    }
  }
  const failed = [];
  for (const lane of failedLanes ?? []) {
    const own = laneJobs(lane, jobs);
    if (own === null) {
      reasons.push(`the gate flagged lane ${lane}, which names no jobs here`);
      continue;
    }
    const unsuccessful = own.filter((job) => job.conclusion !== "success");
    if (unsuccessful.length === 0) reasons.push(`the gate flagged lane ${lane}, but none of its jobs failed`);
    for (const job of unsuccessful) {
      if (!PRODUCTS.includes(job.product) || requires(product, job)) {
        reasons.push(`lane ${lane}: ${product} depends on job ${job.name} (${job.conclusion})`);
      } else {
        failed.push(`${job.name} (${job.product}, ${job.conclusion})`);
      }
    }
  }
  if (reasons.length > 0) return { accepted: false, reasons: [...new Set(reasons)] };
  return { accepted: true, reasons: [`only rows ${product} does not depend on failed: ${[...new Set(failed)].join("; ")}`] };
}

// The single file `name` of a zip archive (an Actions artifact download).
export function readZipEntry(buffer, name) {
  const data = Buffer.from(buffer);
  let end = data.length - 22;
  while (end >= 0 && data.readUInt32LE(end) !== 0x06054b50) end -= 1;
  if (end < 0) throw new Error("not a zip archive");
  const entries = data.readUInt16LE(end + 10);
  let offset = data.readUInt32LE(end + 16);
  for (let index = 0; index < entries; index += 1) {
    if (data.readUInt32LE(offset) !== 0x02014b50) throw new Error("malformed zip directory");
    const method = data.readUInt16LE(offset + 10);
    const compressed = data.readUInt32LE(offset + 20);
    const nameLength = data.readUInt16LE(offset + 28);
    const extraLength = data.readUInt16LE(offset + 30);
    const commentLength = data.readUInt16LE(offset + 32);
    const local = data.readUInt32LE(offset + 42);
    const entryName = data.toString("utf8", offset + 46, offset + 46 + nameLength);
    if (entryName === name) {
      if (data.readUInt32LE(local) !== 0x04034b50) throw new Error("malformed zip entry");
      const start = local + 30 + data.readUInt16LE(local + 26) + data.readUInt16LE(local + 28);
      const body = data.subarray(start, start + compressed);
      if (method === 0) return body;
      if (method === 8) return inflateRawSync(body);
      throw new Error(`unsupported zip method ${method}`);
    }
    offset += 46 + nameLength + extraLength + commentLength;
  }
  throw new Error(`${name} is not in the archive`);
}

// The exact-tree attestation of one run attempt of ci.yml, through the
// Actions artifacts API (as actions/github-script's `github` client), or null
// when the run has none or it expired.
export async function attestationOfRun(github, { owner, repo, runId, attempt, commit }) {
  const name = `meerkat-ci-attestation-${commit}-${attempt}`;
  const { data } = await github.rest.actions.listWorkflowRunArtifacts({ owner, repo, run_id: runId, name, per_page: 100 });
  const artifact = (data?.artifacts ?? []).find((candidate) => candidate.name === name && !candidate.expired);
  if (!artifact) return null;
  const download = await github.rest.actions.downloadArtifact({ owner, repo, artifact_id: artifact.id, archive_format: "zip" });
  return JSON.parse(readZipEntry(download.data, "attestation.json").toString("utf8"));
}

function option(args, name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

function runJobs() {
  const repository = process.env.GITHUB_REPOSITORY;
  const runId = process.env.GITHUB_RUN_ID;
  const attempt = process.env.GITHUB_RUN_ATTEMPT;
  const output = execFileSync(
    "gh",
    ["api", "--paginate", `repos/${repository}/actions/runs/${runId}/attempts/${attempt}/jobs?per_page=100`, "--jq", ".jobs[] | {name, conclusion}"],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  return output.split("\n").filter((line) => line.trim()).map((line) => JSON.parse(line));
}

function main(args) {
  const [command] = args;
  if (command === "record") {
    const output = option(args, "--output");
    if (!output) return usage();
    const jobsFile = option(args, "--jobs");
    const jobs = jobsFile ? JSON.parse(readFileSync(jobsFile, "utf8")) : runJobs();
    const planErrors = Number.parseInt(process.env.PLAN_ERRORS ?? "", 10);
    const evidence = recordEvidence({
      rowProducts: JSON.parse(process.env.ROW_PRODUCTS || "{}"),
      jobs,
      failedLanes: JSON.parse(process.env.FAILED_LANES || "null"),
      planErrors: Number.isInteger(planErrors) ? planErrors : null,
    });
    writeFileSync(output, `${JSON.stringify(evidence, null, 2)}\n`);
    const unknown = evidence.jobs.filter((job) => job.product === null && job.conclusion !== "skipped");
    console.log(`recorded ${evidence.jobs.length} jobs; ${unknown.length} that ran have no known product`);
    return 0;
  }
  if (command === "accepts") {
    const product = option(args, "--product");
    const file = args.at(-1);
    if (!product || !file || file === product) return usage();
    const verdict = acceptance(JSON.parse(readFileSync(file, "utf8")), product);
    console.log(`${verdict.accepted ? "accepted" : "refused"} for ${product}: ${verdict.reasons.join("; ")}`);
    return verdict.accepted ? 0 : 1;
  }
  return usage();
}

function usage() {
  console.error("usage: ci-product-evidence.mjs record --output FILE [--jobs FILE] | accepts --product PRODUCT ATTESTATION");
  return 2;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  process.exit(main(process.argv.slice(2)));
}
