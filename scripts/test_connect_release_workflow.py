#!/usr/bin/env python3
"""Offline contracts for .github/workflows/connect-release.yml.

A Connect tag may be pushed together with Core's and Meta's: its CI gate
waits (bounded) for green exact-main CI instead of refusing, and its
published-floor check first waits (bounded) until every crate of Core's
publish list and Meta's publish order is on crates.io at the versions Connect
declares; the release object is created only after both. The tests run the
gate's real JavaScript under a fake clock, the wait's real shell with stub
curl, date and sleep (and the real jq), the declared-versions step with a
stub cargo, and the job graph under GitHub's status semantics. No network.
Modeled on meta/scripts/test_release_workflow.py.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/connect-release.yml"
JOBS = ("resolve", "require_ci_green", "floor_check", "create_release", "build", "attest", "accept", "publish")


def job_block(name: str) -> str:
    """The workflow text of exactly one job, bounded by the next job header."""
    lines = WORKFLOW.read_text().splitlines()
    start = lines.index(f"  {name}:")
    for offset, line in enumerate(lines[start + 1:], start=start + 1):
        if re.fullmatch(r"  [A-Za-z_][A-Za-z0-9_-]*:", line):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


def job_condition(name: str) -> str:
    block = job_block(name).splitlines()
    matches = [i for i, line in enumerate(block) if line.startswith("    if:")]
    if not matches:
        return "true"
    value = block[matches[0]].split(":", 1)[1].strip()
    if value not in (">-", ">"):
        return value
    body = []
    for line in block[matches[0] + 1:]:
        if not line.startswith("      "):
            break
        body.append(line.strip())
    return " ".join(body)


def job_needs(name: str) -> list[str]:
    lines = [line for line in job_block(name).splitlines() if line.startswith("    needs:")]
    if not lines:
        return []
    if len(lines) != 1 or not re.fullmatch(r"    needs: \[[a-z_, ]+\]", lines[0]):
        raise AssertionError(f"unsupported needs declaration for {name}: {lines}")
    return [value.strip() for value in lines[0].split("[", 1)[1][:-1].split(",")]


def evaluate_condition(expression: str, **context) -> bool:
    """A GitHub `if:` expression under an explicit context; an unsupplied
    name raises instead of reading as false."""
    tokens = re.findall(r"\s+|'(?:[^']|'')*'|(?:github|needs|inputs)(?:\.[A-Za-z_][A-Za-z0-9_-]*)+"
                        r"|[A-Za-z_][A-Za-z0-9_]*|&&|\|\||==|!=|[!()]", expression)
    if "".join(tokens) != expression:
        raise AssertionError(f"unsupported expression syntax: {expression}")
    translated = []
    for token in tokens:
        if token.startswith(("github.", "needs.", "inputs.")):
            name = token.replace(".", "_").replace("-", "_")
            if name not in context:
                raise AssertionError(f"condition references unsupplied name: {name}")
            translated.append(name)
        elif re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token):
            if token not in ("true", "false"):
                raise AssertionError(f"unsupported expression identifier: {token}")
            translated.append({"true": "True", "false": "False"}[token])
        else:
            translated.append({"&&": " and ", "||": " or ", "!": " not "}.get(token, token))
    return bool(eval("".join(translated).strip(), {"__builtins__": {}}, dict(context)))


def job_ancestors(name: str) -> list[str]:
    seen, pending = set(), list(job_needs(name))
    while pending:
        job = pending.pop()
        if job not in seen:
            seen.add(job)
            pending.extend(job_needs(job))
    return sorted(seen, key=JOBS.index)


def job_runs(name: str, **context) -> bool:
    """GitHub's semantics for these conditions, none of which calls a status
    function: the implicit success() reads every ancestor."""
    expression = job_condition(name)
    references = set(re.findall(r"\bneeds\.([A-Za-z_][A-Za-z0-9_]*)\.", expression))
    if references - set(job_needs(name)):
        raise AssertionError(f"{name} references undeclared needs")
    if re.search(r"\b(?:always|success|failure|cancelled)\s*\(", expression):
        raise AssertionError(f"{name}: a status function needs the full model")
    statuses = [context[f"needs_{job}_result"] for job in job_ancestors(name)]
    return evaluate_condition(expression, **context) and all(status == "success" for status in statuses)


def results(event: str, mode: str, failures: dict | None = None) -> dict:
    context = {"github_event_name": event, "needs_resolve_outputs_mode": mode}
    outcome = {}
    for job in JOBS:
        ran = job_runs(job, **context, **{f"needs_{other}_result": outcome[other] for other in outcome})
        outcome[job] = (failures or {}).get(job, "success") if ran else "skipped"
    return outcome


def step_block(step: str, job: str) -> str:
    lines = job_block(job).splitlines()
    start = lines.index(f"      - name: {step}")
    for offset, line in enumerate(lines[start + 1:], start=start + 1):
        if line.startswith("      - name: "):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


def indented_body(step: str, job: str, key: str, indent: int) -> str:
    lines = step_block(step, job).splitlines()
    start = lines.index(" " * (indent - 2) + f"{key}: |")
    body = []
    for line in lines[start + 1:]:
        if line and not line.startswith(" " * indent):
            break
        body.append(line[indent:] if line else "")
    return "\n".join(body)


class TempDir:
    def __enter__(self) -> Path:
        self.path = Path(tempfile.mkdtemp(prefix="connect-release-workflow-"))
        return self.path

    def __exit__(self, *exc) -> None:
        shutil.rmtree(self.path)


class ConnectJobGraphTests(unittest.TestCase):
    def test_a_tag_push_waits_for_ci_then_the_floor_then_creates_the_release(self):
        outcome = results("push", "tag")
        self.assertEqual({job for job, result in outcome.items() if result == "success"},
                         {"resolve", "require_ci_green", "floor_check", "create_release"})
        self.assertEqual(job_needs("require_ci_green"), ["resolve"])
        self.assertEqual(job_needs("floor_check"), ["resolve", "require_ci_green"])
        self.assertEqual(job_needs("create_release"), ["resolve", "require_ci_green", "floor_check"])

    def test_nothing_is_created_without_green_ci_and_the_floor(self):
        for job in ("require_ci_green", "floor_check"):
            for status in ("failure", "cancelled", "skipped"):
                with self.subTest(job=job, status=status):
                    outcome = results("push", "tag", {job: status})
                    self.assertEqual(outcome["create_release"], "skipped")
                    if job == "require_ci_green":
                        self.assertEqual(outcome["floor_check"], "skipped")

    def test_dispatch_modes_run_only_their_jobs(self):
        for mode, expected in (("candidate", {"build", "attest"}), ("accept", {"accept"}),
                               ("publish", {"publish"})):
            with self.subTest(mode=mode):
                outcome = results("workflow_dispatch", mode)
                ran = {job for job, result in outcome.items() if result == "success"} - {"resolve"}
                self.assertEqual(ran, expected)

    def test_the_floor_waits_for_the_crates_before_it_builds(self):
        block = job_block("floor_check")
        order = [block.index(f"      - name: {step}") for step in (
            "Read the Core and Meta versions Connect declares",
            "Wait until every Core and Meta crate is on crates.io at the declared versions",
            "Build Connect against published Core and Meta")]
        self.assertEqual(order, sorted(order))

    def test_the_floor_job_timeout_covers_the_wait_and_the_build(self):
        timeout = int(re.search(r"(?m)^    timeout-minutes: ([0-9]+)$", job_block("floor_check")).group(1))
        wait = indented_body("Wait until every Core and Meta crate is on crates.io at the declared versions",
                             "floor_check", "run", 10)
        deadline_seconds = int(re.search(r"deadline=\$\(\( \$\(date \+%s\) \+ ([0-9]+) \)\)", wait).group(1))
        self.assertGreaterEqual(timeout, deadline_seconds // 60 + 90)


# Runs a github-script body as actions/github-script does against a stub runs
# API that serves the scenario's ci.yml runs for each poll (the last list
# stays) and a fake clock each poll's sleep advances.
GATE_HARNESS = r"""
"use strict";
const fs = require("fs");
const [scriptPath, scenarioPath, resultPath] = process.argv.slice(2);
const body = fs.readFileSync(scriptPath, "utf8");
const scenario = JSON.parse(fs.readFileSync(scenarioPath, "utf8"));
const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor;
const START = 1000000;
const clock = { now: START, sleeps: 0 };
Date.now = () => clock.now;
globalThis.setTimeout = (callback, ms, ...args) => {
  clock.now += Number(ms) || 0;
  clock.sleeps += 1;
  setImmediate(() => callback(...args));
  return 0;
};
const result = { failed: null, warnings: [], calls: [], error: null };
let count = 0;
const github = { rest: { actions: { listWorkflowRuns: async (params) => {
  count += 1;
  if (count > 100000) throw new Error("no verdict after 100000 API calls");
  result.calls.push(params);
  const runs = scenario.ci[Math.min(clock.sleeps, scenario.ci.length - 1)];
  return { data: { workflow_runs: runs.map((run) => ({ ...run })) } };
} } } };
const core = {
  debug: () => {}, info: () => {}, notice: () => {}, error: () => {},
  warning: (message) => result.warnings.push(String(message)),
  setFailed: (message) => { if (result.failed === null) result.failed = String(message); },
  setOutput: () => {},
};
(async () => {
  try {
    await new AsyncFunction("require", "github", "context", "core", body)(
      require, github, { sha: scenario.sha, repo: { owner: "lukacf", repo: "meerkat" } }, core);
  } catch (error) {
    result.error = String((error && error.stack) || error);
  }
  result.sleeps = clock.sleeps;
  result.elapsed_minutes = (clock.now - START) / 60000;
  fs.writeFileSync(resultPath, JSON.stringify(result));
})();
"""


class ConnectCiGateTests(unittest.TestCase):
    """The CI gate's real JavaScript: it polls exact-main CI every 30 seconds
    until a run is green or 120 minutes elapsed; a green re-run counts."""

    CI = {"id": 42, "run_attempt": 1, "head_sha": "deadbeef", "head_branch": "main", "event": "push",
          "status": "completed", "conclusion": "success", "html_url": "https://example.invalid/runs/42"}

    def gate(self, timeline):
        body = indented_body("Wait for green exact-main CI at this commit", "require_ci_green", "script", 12)
        with TempDir() as tmp:
            (tmp / "harness.js").write_text(GATE_HARNESS)
            (tmp / "gate.js").write_text(body)
            (tmp / "scenario.json").write_text(json.dumps({"sha": "deadbeef", "ci": timeline}))
            subprocess.run(["node", str(tmp / "harness.js"), str(tmp / "gate.js"), str(tmp / "scenario.json"),
                            str(tmp / "result.json")], check=True, capture_output=True, text=True)
            result = json.loads((tmp / "result.json").read_text())
        self.assertIsNone(result["error"])
        return result

    def test_green_ci_is_accepted_at_once(self):
        result = self.gate([[self.CI]])
        self.assertEqual((result["failed"], result["sleeps"]), (None, 0))
        self.assertEqual({key: result["calls"][0][key] for key in ("workflow_id", "branch", "event", "head_sha")},
                         {"workflow_id": "ci.yml", "branch": "main", "event": "push", "head_sha": "deadbeef"})

    def test_pending_ci_is_awaited_until_green(self):
        pending = {**self.CI, "status": "in_progress", "conclusion": None}
        result = self.gate([[], [pending], [pending], [self.CI]])
        self.assertEqual((result["failed"], result["sleeps"]), (None, 3))

    def test_a_green_re_run_of_a_red_or_cancelled_run_counts(self):
        for conclusion, warnings in (("failure", 0), ("cancelled", 1)):
            with self.subTest(conclusion=conclusion):
                red = {**self.CI, "conclusion": conclusion}
                result = self.gate([[red], [red], [{**self.CI, "run_attempt": 2}]])
                self.assertIsNone(result["failed"])
                self.assertEqual(len(result["warnings"]), warnings)
        self.assertIn("gh run rerun 42 --repo lukacf/meerkat", result["warnings"][0])

    def test_the_gate_fails_at_its_deadline(self):
        for runs in ([], [{**self.CI, "status": "queued", "conclusion": None}], [{**self.CI, "conclusion": "failure"}]):
            with self.subTest(runs=runs):
                result = self.gate([runs])
                self.assertIn("has no green exact-main CI after 120 minutes", result["failed"])
                self.assertEqual((result["elapsed_minutes"], result["sleeps"]), (120, 240))

    def test_runs_of_another_commit_branch_or_event_never_count(self):
        for change in ({"head_sha": "other"}, {"head_branch": "feature"}, {"event": "pull_request"}):
            with self.subTest(change=change):
                self.assertIn("after 120 minutes", self.gate([[{**self.CI, **change}]])["failed"])


STUB = (
    f"#!{sys.executable}\n"
    "import json, os, sys\n"
    "state_path = os.environ['WAIT_STATE']\n"
    "state = json.load(open(state_path))\n"
    "tool, args = os.path.basename(sys.argv[0]), sys.argv[1:]\n"
    "def save():\n"
    "    json.dump(state, open(state_path, 'w'))\n"
    "if tool == 'date':\n"
    "    assert args == ['+%s'], args\n"
    "    print(state['now'])\n"
    "elif tool == 'sleep':\n"
    "    state['now'] += int(args[0]); state['sleeps'].append(int(args[0])); save()\n"
    "elif tool == 'cargo':\n"
    "    print(os.environ['STUB_METADATA'])\n"
    "else:\n"
    "    prefix = 'https://index.crates.io/'\n"
    "    path = args[-1][len(prefix):]\n"
    "    state['reads'].append(path); save()\n"
    "    name = path.rsplit('/', 1)[1]\n"
    "    versions = list(json.loads(os.environ['WAIT_INDEX']).get(name, []))\n"
    "    after = json.loads(os.environ['WAIT_AFTER']).get(name)\n"
    "    if after and state['now'] >= after[0]:\n"
    "        versions.append(after[1])\n"
    "    if not versions:\n"
    "        print('curl: (22) The requested URL returned error: 404', file=sys.stderr)\n"
    "        sys.exit(22)\n"
    "    for version in versions:\n"
    "        print(json.dumps({'name': name, 'vers': version, 'yanked': False}))\n"
)


def run_step(step: str, *, core=(), meta=(), index=None, after=None, env=None):
    """Run one floor_check step's shell, verbatim, in a fixture checkout with
    stub cargo, curl, date and sleep. Returns the process result, the index
    paths read, the sleeps and the GITHUB_OUTPUT text."""
    script = indented_body(step, "floor_check", "run", 10)
    with TempDir() as root:
        (root / "bin").mkdir()
        (root / "scripts").mkdir()
        (root / "ci").mkdir()
        listing = root / "scripts/release-rust-crates.sh"
        listing.write_text("#!/usr/bin/env bash\n" + "".join(f"echo {name}\n" for name in core))
        listing.chmod(0o755)
        (root / "ci/products.json").write_text(json.dumps({"products": [
            {"name": "core", "root": "", "publish_order": "scripts/release-rust-crates.sh"},
            {"name": "meta", "root": "meta", "publish_order": list(meta)}]}))
        for tool in ("curl", "date", "sleep", "cargo"):
            (root / "bin" / tool).write_text(STUB)
            (root / "bin" / tool).chmod(0o755)
        state = root / "state.json"
        state.write_text(json.dumps({"now": 1000, "reads": [], "sleeps": []}))
        output = root / "github-output"
        result = subprocess.run(["bash", "-c", script], cwd=root, capture_output=True, text=True, env={
            **os.environ, "PATH": f"{root / 'bin'}:{os.environ['PATH']}", "WAIT_STATE": str(state),
            "WAIT_INDEX": json.dumps(index or {}), "WAIT_AFTER": json.dumps(after or {}),
            "GITHUB_OUTPUT": str(output), **(env or {})})
        recorded = json.loads(state.read_text())
        return result, recorded["reads"], recorded["sleeps"], output.read_text() if output.exists() else ""


WAIT = "Wait until every Core and Meta crate is on crates.io at the declared versions"
CORE_CRATES = ("meerkat-core", "meerkat", "rkat")
META_CRATES = ("meta-extension-state", "meerkat-meta")
VERSIONS = {"CORE_VERSION": "0.9.12", "META_VERSION": "0.9.13"}


class ConnectFloorWaitTests(unittest.TestCase):
    """The floor check waits for Core's publish list at Core's declared
    version and Meta's publish order at Meta's, bounded at 120 minutes."""

    def published(self, *, but=()):
        index = {name: ["0.9.11", "0.9.12"] for name in CORE_CRATES}
        index.update({name: ["0.9.11", "0.9.13"] for name in META_CRATES})
        for name in but:
            index[name] = ["0.9.11"]
        return index

    def wait(self, index, **kwargs):
        return run_step(WAIT, core=CORE_CRATES, meta=META_CRATES, index=index, env=VERSIONS, **kwargs)

    def test_every_crate_at_its_declared_version_ends_the_wait_at_once(self):
        result, reads, sleeps, _ = self.wait(self.published())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(reads, ["me/er/meerkat-core", "me/er/meerkat", "rk/at/rkat",
                                 "me/ta/meta-extension-state", "me/er/meerkat-meta"])
        self.assertEqual(sleeps, [])

    def test_meta_published_after_core_is_awaited(self):
        result, reads, sleeps, _ = self.wait(self.published(but=META_CRATES),
                                             after={"meerkat-meta": [1090, "0.9.13"],
                                                    "meta-extension-state": [1030, "0.9.13"]})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sleeps, [30, 30, 30])
        self.assertIn("waiting for 2 of 5 Core and Meta crates", result.stdout)
        self.assertEqual(reads[5:7], ["me/ta/meta-extension-state", "me/er/meerkat-meta"])

    def test_each_product_is_awaited_at_its_own_version(self):
        index = self.published()
        index["meerkat-meta"] = ["0.9.12"]
        result, _, sleeps, _ = self.wait(index)
        self.assertEqual(result.returncode, 1)
        self.assertEqual((len(sleeps), sum(sleeps)), (240, 7200))
        self.assertIn("1 of 5 Core and Meta crates are not on crates.io after 120 minutes (meerkat-meta@0.9.13)",
                      result.stderr)

    def test_unpublished_core_is_awaited_and_named(self):
        result, _, _, _ = self.wait(self.published(but=("rkat",)))
        self.assertEqual(result.returncode, 1)
        self.assertIn("(rkat@0.9.12)", result.stderr)

    def test_an_empty_list_fails(self):
        result, reads, _, _ = run_step(WAIT, core=CORE_CRATES, meta=(), index=self.published(), env=VERSIONS)
        self.assertEqual(result.returncode, 1)
        self.assertIn("is empty", result.stderr)
        self.assertEqual(reads, [])


def metadata(*packages):
    return json.dumps({"workspace_root": "/ws", "packages": [
        {"name": name, "manifest_path": f"/ws/{path}/Cargo.toml",
         "dependencies": [{"name": dep, "req": req} for dep, req in deps]}
        for name, path, deps in packages]})


class ConnectDeclaredVersionsTests(unittest.TestCase):
    """The versions the wait uses come from Connect's own manifests: the one
    exact requirement on meerkat-core, and on meerkat-meta (which Connect
    renames meerkat-mobkit)."""

    STEP = "Read the Core and Meta versions Connect declares"

    def declared(self, packages):
        return run_step(self.STEP, env={"STUB_METADATA": metadata(*packages)})

    def test_the_exact_core_and_meta_requirements_are_read(self):
        result, _, _, output = self.declared([
            ("meerkat-connect", "connect/crates/meerkat-connect",
             [("meerkat-core", "=0.9.12"), ("meerkat-meta", "=0.9.13"), ("serde", "^1")]),
            ("toolkit-mobkit", "connect/crates/toolkit-mobkit", [("meerkat-core", "=0.9.12")]),
            ("meerkat-meta", "meta/crates/meerkat-meta", [("meerkat-core", "=0.9.11")]),
        ])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "core_version=0.9.12\nmeta_version=0.9.13\n")

    def test_a_missing_or_inexact_requirement_fails(self):
        for case, deps in (("two Core versions", [("meerkat-core", "=0.9.12"), ("meerkat-core", "=0.9.11"),
                                                  ("meerkat-meta", "=0.9.13")]),
                           ("a range", [("meerkat-core", "^0.9.12"), ("meerkat-meta", "=0.9.13")]),
                           ("no Meta", [("meerkat-core", "=0.9.12")])):
            with self.subTest(case=case):
                result, _, _, output = self.declared([("meerkat-connect", "connect/crates/meerkat-connect", deps)])
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")
        result, _, _, _ = self.declared([("meerkat-meta", "meta/crates/meerkat-meta", [("meerkat-core", "=0.9.11")])])
        self.assertIn("no Connect package", result.stderr)


if __name__ == "__main__":
    unittest.main()
