#!/usr/bin/env node
// Exact-main CI evidence for a release-projection commit.
//
// A commit whose diff from its first parent is only the release projection
// (scripts/release-projection-only.mjs: the forward patch bump
// scripts/release-hook.sh writes) changes no test input the parent's CI did
// not already run against, apart from the version strings the release
// checks verify and the files whose readers the plan still runs. CI then
// reuses the parent's test evidence instead of rebuilding the workspace
// cold: the version bump changes every crate's metadata, so nothing is
// cached.
//
// The evidence must be a COMPLETED, successful run of the CI workflow
// (ci.yml) for a push to main at exactly the parent commit, whose schema-4
// exact-tree attestation names the parent's commit and tree, planned the
// whole workspace (plan mode `workspace`) and records every lane as
// `success`. A narrower parent run is not evidence: a `none` or `packages`
// plan tests only its own diff, and an earlier main commit whose run was
// cancelled would otherwise ship untested behind it. A run that is still in
// progress is never evidence either; `find` can wait for it to finish.
//
// usage:
//   ci-release-evidence.mjs find   --repository OWNER/REPO --base REV --head REV
//                                  [--wait-seconds S]
//   ci-release-evidence.mjs verify --repository OWNER/REPO --base REV --head REV
//                                  --run-id ID --run-attempt N
//                                  [--github-output FILE]
//
// Both modes first require that head's first parent is base, that base..head
// is a release projection, and that no test, snapshot, fixture, SDK or Rust
// code line outside the projection pins the old or new version
// (scripts/release-version-pins.mjs): a pinned test would pass on the parent
// and fail after the bump, so pins send the commit to the ordinary plan,
// which runs it. The CI workflows run the copies of this script, the
// classifier and the pin ratchet from the parent commit's tree, so a commit
// cannot certify itself with edited scripts.
//
// find (the change classification): the NEWEST exact-main run of base
//   decides. While it is queued or running, find waits for it, up to
//   --wait-seconds (default 0). If it completed successfully and its
//   attestation verifies, find prints `<base sha>:<run id>:<attempt>`; a
//   newer failed or cancelled run is never passed over for an older
//   success. Exit 1 when there is no usable evidence, including any lookup
//   failure: the caller then plans the full workspace, never less.
// verify (the CI gate and the release workflow's require_ci_green): requires
//   the named run attempt to be a completed, successful exact-main run of
//   base, the run's LATEST attempt to be that same attempt (completed and
//   successful: no later re-run failed or is still running), and the
//   attestation to verify. Prints the evidence and, with --github-output,
//   records run_id, run_attempt, commit, tree, run_url,
//   attestation_artifact, from_version and to_version. Exit 1 on any failure.
//
// The GitHub CLI is `gh` (MEERKAT_CI_EVIDENCE_GH overrides it for the
// self-test's fake); MEERKAT_CI_EVIDENCE_POLL_SECONDS sets the wait poll
// interval (default 30).

import { spawnSync } from "node:child_process";
import { appendFileSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { ReleaseProjectionError, classifyReleaseProjection } from "./release-projection-only.mjs";
import { findReleaseVersionPins } from "./release-version-pins.mjs";

export const CI_WORKFLOW_PATH = ".github/workflows/ci.yml";
export const CI_WORKFLOW_NAME = "CI";
export const ATTESTATION_BACKEND = "github-hosted-cargo";
export const RELEASE_PROJECTION_MODE = "release-projection";
export const REUSE_CLASSIFIER = "release-projection-only";
// Every lane of the CI attestation's component_results, all of which a
// workspace-mode main push runs. The unit lane is the one exception: a push
// runs its unit rows in the main unit lane (plan.unit_rows_on_main_count), so
// it is skipped there.
export const WORKSPACE_LANES = [
  "changes",
  "fmt_governance",
  "ratchets",
  "clippy",
  "unit",
  "integration",
  "closure_tests",
  "governed_jsonl",
  "mcp_confinement_positive",
  "main_unit",
  "closure_check",
  "rustdoc",
  "wasm_check",
  "sdk_host",
  "example_web",
  "wasm_timers",
  "bazel_graph",
];
const PENDING_STATUSES = new Set(["queued", "in_progress", "waiting", "requested", "pending"]);

export class EvidenceError extends Error {}

export function attestationArtifactName(commit, attempt) {
  return `meerkat-ci-attestation-${commit}-${attempt}`;
}

// Problems with a workflow run (or one attempt of it) as exact-main CI
// evidence for `commit`; empty when it qualifies.
export function runProblems(run, commit) {
  const problems = [];
  const expect = (field, actual, wanted) => {
    if (actual !== wanted) problems.push(`${field} is ${JSON.stringify(actual)}, not ${JSON.stringify(wanted)}`);
  };
  expect("workflow path", run?.path, CI_WORKFLOW_PATH);
  expect("workflow name", run?.name, CI_WORKFLOW_NAME);
  expect("event", run?.event, "push");
  expect("branch", run?.head_branch, "main");
  expect("head commit", run?.head_sha, commit);
  expect("status", run?.status, "completed");
  expect("conclusion", run?.conclusion, "success");
  if (!Number.isInteger(run?.run_attempt) || run.run_attempt < 1) {
    problems.push(`run attempt ${JSON.stringify(run?.run_attempt)} is not a positive integer`);
  }
  return problems;
}

// Problems with a schema-4 attestation as the exact-tree evidence of
// `commit`/`tree` from run `runId` attempt `runAttempt`; empty when it
// qualifies. Only a whole-workspace run whose every lane succeeded is
// evidence, and reused evidence is never evidence again.
export function attestationProblems(attestation, { repository, commit, tree, runId, runAttempt }) {
  const problems = [];
  const expect = (field, actual, wanted) => {
    if (actual !== wanted) problems.push(`${field} is ${JSON.stringify(actual)}, not ${JSON.stringify(wanted)}`);
  };
  expect("schema_version", attestation?.schema_version, 4);
  expect("repository", attestation?.repository, repository);
  expect("commit_sha", attestation?.commit_sha, commit);
  expect("tree_sha", attestation?.tree_sha, tree);
  expect("workflow", attestation?.workflow, CI_WORKFLOW_NAME);
  expect("workflow_run_id", attestation?.workflow_run_id, String(runId));
  expect("workflow_run_attempt", attestation?.workflow_run_attempt, String(runAttempt));
  expect("event", attestation?.event, "push");
  expect("ref", attestation?.ref, "refs/heads/main");
  expect("validation_backend", attestation?.validation_backend, ATTESTATION_BACKEND);
  expect("validation_result", attestation?.validation_result, "success");
  expect("plan.mode", attestation?.plan?.mode, "workspace");
  const components = attestation?.component_results;
  if (!components || typeof components !== "object" || Array.isArray(components)) {
    problems.push("component_results is missing");
  } else {
    const unitRowsOnMain = attestation?.plan?.unit_rows_on_main_count;
    for (const lane of WORKSPACE_LANES) {
      if (lane === "unit" && components.unit === "skipped" && Number.isInteger(unitRowsOnMain) && unitRowsOnMain > 0) continue;
      expect(`component_results.${lane}`, components[lane], "success");
    }
    for (const [lane, result] of Object.entries(components)) {
      if (!WORKSPACE_LANES.includes(lane) && result !== "success") {
        problems.push(`component_results.${lane} is ${JSON.stringify(result)}, not "success"`);
      }
    }
  }
  if (attestation && Object.hasOwn(attestation, "reused_test_evidence")) {
    problems.push("it records reused_test_evidence");
  }
  return problems;
}

// The evidence argument the classification hands the planner:
// `<base sha>:<run id>:<attempt>`.
export function parseEvidenceArgument(value) {
  const match = /^([0-9a-f]{40}):([1-9][0-9]*):([1-9][0-9]*)$/.exec(value ?? "");
  if (!match) throw new EvidenceError(`release evidence ${JSON.stringify(value)} is not <commit>:<run id>:<attempt>`);
  return { commit: match[1], runId: match[2], runAttempt: match[3] };
}

function git(args, cwd) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.status !== 0) {
    throw new EvidenceError(`git ${args.join(" ")} failed: ${(result.stderr ?? "").trim() || result.error?.message || "no output"}`);
  }
  return result.stdout.trim();
}

// The precondition both modes and the planner share: head's first parent is
// base, and base..head is a release projection. Returns the resolved commits,
// base's tree and the projection's versions.
export function releaseProjectionBase({ base, head, cwd = process.cwd() }) {
  if (!base || !head) throw new EvidenceError("base and head revisions are required");
  const baseSha = git(["rev-parse", "--verify", "--quiet", `${base}^{commit}`], cwd);
  const headSha = git(["rev-parse", "--verify", "--quiet", `${head}^{commit}`], cwd);
  const parent = spawnSync("git", ["rev-parse", "--verify", "--quiet", `${headSha}^1`], { cwd, encoding: "utf8" });
  const parentSha = parent.status === 0 ? parent.stdout.trim() : "";
  if (parentSha !== baseSha) {
    throw new EvidenceError(`${baseSha} is not the first parent of ${headSha} (first parent: ${parentSha || "none"})`);
  }
  let classification;
  try {
    classification = classifyReleaseProjection({ base: baseSha, head: headSha, cwd });
  } catch (error) {
    if (error instanceof ReleaseProjectionError) throw new EvidenceError(error.message);
    throw error;
  }
  if (!classification.projection) {
    throw new EvidenceError(`${baseSha}..${headSha} is not a release projection: ${classification.reason}`);
  }
  let pins;
  try {
    pins = findReleaseVersionPins({ from: classification.oldVersion, to: classification.newVersion, revision: headSha, cwd });
  } catch (error) {
    throw new EvidenceError(error.message);
  }
  if (pins.length > 0) {
    throw new EvidenceError(
      `${pins.length} line(s) outside the projection pin ${classification.oldVersion} or ${classification.newVersion} ` +
        `(first: ${pins[0].path}:${pins[0].line}); their tests must run, so nothing is reused`,
    );
  }
  return {
    baseSha,
    headSha,
    baseTree: git(["rev-parse", "--verify", `${baseSha}^{tree}`], cwd),
    fromVersion: classification.oldVersion,
    toVersion: classification.newVersion,
  };
}

function gh(args) {
  const command = process.env.MEERKAT_CI_EVIDENCE_GH || "gh";
  const result = spawnSync(command, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.status !== 0) {
    throw new EvidenceError(`${command} ${args.join(" ")} failed: ${(result.stderr ?? "").trim() || result.error?.message || `status ${result.status}`}`);
  }
  return result.stdout;
}

function ghJson(path) {
  const text = gh(["api", path]);
  try {
    return JSON.parse(text);
  } catch {
    throw new EvidenceError(`gh api ${path} returned invalid JSON`);
  }
}

function downloadAttestation(repository, runId, artifact) {
  const scratch = mkdtempSync(join(tmpdir(), "meerkat-ci-evidence-"));
  try {
    gh(["run", "download", String(runId), "--repo", repository, "--name", artifact, "--dir", scratch]);
    let text;
    try {
      text = readFileSync(join(scratch, "attestation.json"), "utf8");
    } catch {
      throw new EvidenceError(`artifact ${artifact} of run ${runId} has no attestation.json`);
    }
    try {
      return JSON.parse(text);
    } catch {
      throw new EvidenceError(`artifact ${artifact} of run ${runId} holds invalid JSON`);
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

function verifyAttestation({ repository, baseSha, baseTree, runId, runAttempt }) {
  const artifact = attestationArtifactName(baseSha, runAttempt);
  const attestation = downloadAttestation(repository, runId, artifact);
  const problems = attestationProblems(attestation, {
    repository,
    commit: baseSha,
    tree: baseTree,
    runId,
    runAttempt,
  });
  if (problems.length > 0) {
    throw new EvidenceError(`attestation ${artifact} of run ${runId} does not qualify: ${problems.join("; ")}`);
  }
  return artifact;
}

const sleep = (seconds) => new Promise((done) => setTimeout(done, Math.max(1, seconds) * 1000));

async function find({ repository, base, head, waitSeconds }) {
  const projection = releaseProjectionBase({ base, head });
  const pollSeconds = Number.parseInt(process.env.MEERKAT_CI_EVIDENCE_POLL_SECONDS || "30", 10);
  const deadline = Date.now() + waitSeconds * 1000;
  for (;;) {
    const listing = ghJson(
      `repos/${repository}/actions/workflows/ci.yml/runs?branch=main&event=push&head_sha=${projection.baseSha}&per_page=100`,
    );
    // Newest first: a later run of the same commit supersedes an older one.
    const runs = (listing?.workflow_runs ?? [])
      .filter(
        (run) =>
          run.head_sha === projection.baseSha &&
          run.head_branch === "main" &&
          run.event === "push" &&
          run.path === CI_WORKFLOW_PATH,
      )
      .sort((a, b) => b.id - a.id);
    // The newest run decides: wait while it runs, and never pass over a
    // newer failure or cancellation for an older success.
    const [newest] = runs;
    if (newest && PENDING_STATUSES.has(newest.status) && Date.now() < deadline) {
      process.stderr.write(
        `waiting for CI run ${newest.id} of ${projection.baseSha} (${newest.status}); reuse needs it completed\n`,
      );
      await sleep(pollSeconds);
      continue;
    }
    if (!newest) throw new EvidenceError(`no exact-main CI run of ${projection.baseSha} to reuse`);
    const problems = runProblems(newest, projection.baseSha);
    if (problems.length > 0) {
      throw new EvidenceError(
        `the newest exact-main CI run of ${projection.baseSha}, ${newest.id} attempt ${newest.run_attempt}, ` +
          `is not reusable: ${problems.join("; ")}`,
      );
    }
    const artifact = verifyAttestation({
      repository,
      baseSha: projection.baseSha,
      baseTree: projection.baseTree,
      runId: newest.id,
      runAttempt: newest.run_attempt,
    });
    return { ...projection, runId: String(newest.id), runAttempt: String(newest.run_attempt), url: newest.html_url ?? "", artifact };
  }
}

function verify({ repository, base, head, runId, runAttempt }) {
  const projection = releaseProjectionBase({ base, head });
  if (!/^[1-9][0-9]*$/.test(runId ?? "")) throw new EvidenceError(`--run-id ${JSON.stringify(runId)} is not a run id`);
  if (!/^[1-9][0-9]*$/.test(runAttempt ?? "")) {
    throw new EvidenceError(`--run-attempt ${JSON.stringify(runAttempt)} is not an attempt number`);
  }
  const run = ghJson(`repos/${repository}/actions/runs/${runId}/attempts/${runAttempt}`);
  const problems = runProblems(run, projection.baseSha);
  if (String(run?.id) !== String(runId)) problems.push(`run id is ${JSON.stringify(run?.id)}, not ${runId}`);
  if (String(run?.run_attempt) !== runAttempt) problems.push(`attempt is ${JSON.stringify(run?.run_attempt)}, not ${runAttempt}`);
  // The run's latest attempt must be the pinned one: a later re-run that
  // failed, was cancelled or is still running withdraws the evidence.
  const latest = ghJson(`repos/${repository}/actions/runs/${runId}`);
  if (String(latest?.run_attempt) !== runAttempt) {
    problems.push(`the run's latest attempt is ${JSON.stringify(latest?.run_attempt)}, not the pinned ${runAttempt}`);
  }
  if (latest?.status !== "completed" || latest?.conclusion !== "success") {
    problems.push(`the run's latest attempt is ${latest?.status ?? "unknown"}/${latest?.conclusion ?? "none"}, not completed/success`);
  }
  if (problems.length > 0) {
    throw new EvidenceError(`CI run ${runId} attempt ${runAttempt} is not successful exact-main evidence for ${projection.baseSha}: ${problems.join("; ")}`);
  }
  const artifact = verifyAttestation({
    repository,
    baseSha: projection.baseSha,
    baseTree: projection.baseTree,
    runId,
    runAttempt,
  });
  return { ...projection, runId: String(runId), runAttempt, url: run.html_url ?? "", artifact };
}

function parseArgs(argv) {
  const [mode, ...rest] = argv;
  if (mode !== "find" && mode !== "verify") throw new EvidenceError("usage: ci-release-evidence.mjs find|verify --repository OWNER/REPO --base REV --head REV [...]");
  const args = { mode, repository: "", base: "", head: "", runId: "", runAttempt: "", waitSeconds: 0, githubOutput: "" };
  for (let i = 0; i < rest.length; i += 1) {
    const value = () => {
      const next = rest[i + 1];
      if (next === undefined) throw new EvidenceError(`${rest[i]} needs a value`);
      i += 1;
      return next;
    };
    switch (rest[i]) {
      case "--repository": args.repository = value(); break;
      case "--base": args.base = value(); break;
      case "--head": args.head = value(); break;
      case "--run-id": args.runId = value(); break;
      case "--run-attempt": args.runAttempt = value(); break;
      case "--wait-seconds": {
        args.waitSeconds = Number.parseInt(value(), 10);
        if (!Number.isInteger(args.waitSeconds) || args.waitSeconds < 0) throw new EvidenceError("--wait-seconds needs a non-negative integer");
        break;
      }
      case "--github-output": args.githubOutput = value(); break;
      default: throw new EvidenceError(`unknown argument: ${rest[i]}`);
    }
  }
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(args.repository)) throw new EvidenceError("--repository OWNER/REPO is required");
  if (mode === "verify" && (!args.runId || !args.runAttempt)) throw new EvidenceError("verify needs --run-id and --run-attempt");
  return args;
}

async function main(argv) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (error) {
    process.stderr.write(`ci-release-evidence: ${error.message}\n`);
    return 2;
  }
  try {
    if (args.mode === "find") {
      const found = await find(args);
      process.stderr.write(
        `release projection ${found.fromVersion} -> ${found.toVersion}: reusing CI run ${found.runId} ` +
          `(attempt ${found.runAttempt}) of ${found.baseSha} ${found.url}\n`,
      );
      process.stdout.write(`${found.baseSha}:${found.runId}:${found.runAttempt}\n`);
      return 0;
    }
    const verified = verify(args);
    process.stdout.write(
      `verified release-projection evidence: ${verified.fromVersion} -> ${verified.toVersion}, CI run ${verified.runId} ` +
        `attempt ${verified.runAttempt} of ${verified.baseSha} (tree ${verified.baseTree}) ${verified.url}\n`,
    );
    if (args.githubOutput) {
      appendFileSync(
        args.githubOutput,
        [
          `run_id=${verified.runId}`,
          `run_attempt=${verified.runAttempt}`,
          `commit=${verified.baseSha}`,
          `tree=${verified.baseTree}`,
          `run_url=${verified.url}`,
          `attestation_artifact=${verified.artifact}`,
          `from_version=${verified.fromVersion}`,
          `to_version=${verified.toVersion}`,
          "",
        ].join("\n"),
      );
    }
    return 0;
  } catch (error) {
    if (!(error instanceof EvidenceError)) throw error;
    process.stderr.write(`ci-release-evidence ${args.mode}: ${error.message}\n`);
    return 1;
  }
}

const invokedPath = process.argv[1] ? realpathSync(process.argv[1]) : "";
if (invokedPath === realpathSync(fileURLToPath(import.meta.url))) {
  process.exitCode = await main(process.argv.slice(2));
}
