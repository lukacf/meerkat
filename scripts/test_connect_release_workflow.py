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

The macOS executables are built the way Meta's gateways are (Cargo on a
hosted macos-15 runner, checked, ad-hoc signed, archived, attested) and are
added to the release only after its gates, add-only. The tests run the
resolver's real shell, the build job's portable steps (source check, smoke
run, packaging) with stand-in executables, and the publisher
(scripts/connect_release_macos_assets.py) against a stub `gh`; the steps
that need Apple's tools (lipo, file, codesign) are read, not run.

The complete macOS installation is staged by a dispatch (`macos-stage`): two
hosted macos-15 jobs call the producer once per step, pass the work root
between them, and upload the stage record as a workflow artifact. They
create no release and no tag, hold no token and no provider key, and upload
nothing outside the run. The tests read the job graph, permissions, paths
and step order, and run the portable shell (the layout check, the work root
pack and restore, the choice of the consumer check's rkat) with stand-ins.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/connect-release.yml"
PUBLISHER = ROOT / "scripts/connect_release_macos_assets.py"
JOBS = ("resolve", "require_ci_green", "floor_check", "create_release", "build_macos", "publish_macos",
        "recover_macos", "stage_macos_build", "stage_macos_check", "assemble_macos", "build", "attest", "accept",
        "publish")


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
                         {"resolve", "require_ci_green", "floor_check", "create_release",
                          "build_macos", "publish_macos"})
        self.assertEqual(job_needs("require_ci_green"), ["resolve"])
        self.assertEqual(job_needs("floor_check"), ["resolve", "require_ci_green"])
        self.assertEqual(job_needs("create_release"), ["resolve", "require_ci_green", "floor_check"])

    def test_nothing_is_created_without_green_ci_and_the_floor(self):
        for job in ("require_ci_green", "floor_check"):
            for status in ("failure", "cancelled", "skipped"):
                with self.subTest(job=job, status=status):
                    outcome = results("push", "tag", {job: status})
                    self.assertEqual(outcome["create_release"], "skipped")
                    self.assertEqual(outcome["publish_macos"], "skipped")
                    if job == "require_ci_green":
                        self.assertEqual(outcome["floor_check"], "skipped")

    def test_dispatch_modes_run_only_their_jobs(self):
        for mode, expected in (("candidate", {"build", "attest"}), ("accept", {"accept"}),
                               ("publish", {"publish"}), ("macos-build", {"build_macos"}),
                               ("macos-assets", {"build_macos", "recover_macos"})):
            with self.subTest(mode=mode):
                outcome = results("workflow_dispatch", mode)
                ran = {job for job, result in outcome.items() if result == "success"} - {"resolve"}
                self.assertEqual(ran, expected)

    def test_the_macos_build_runs_beside_the_gates_and_is_published_only_after_them(self):
        # The build waits for no gate (it takes hours and writes nothing
        # outside its run); the publisher waits for the release object, which
        # exists only after green CI and the floor check, and for the build.
        self.assertEqual(job_needs("build_macos"), ["resolve"])
        self.assertEqual(job_needs("publish_macos"), ["resolve", "create_release", "build_macos"])
        self.assertEqual(job_ancestors("publish_macos"),
                         ["resolve", "require_ci_green", "floor_check", "create_release", "build_macos"])
        for status in ("failure", "cancelled", "skipped"):
            with self.subTest(status=status):
                outcome = results("push", "tag", {"build_macos": status})
                self.assertEqual(outcome["create_release"], "success")
                self.assertEqual(outcome["publish_macos"], "skipped")

    def test_a_build_only_dispatch_publishes_nothing(self):
        outcome = results("workflow_dispatch", "macos-build")
        for job in ("create_release", "publish_macos", "recover_macos", "publish"):
            self.assertEqual(outcome[job], "skipped")
        # Only the two publishers and the release creator may write the
        # repository's contents; the build job cannot.
        writers = {job for job in JOBS if "contents: write" in job_block(job)}
        self.assertEqual(writers, {"create_release", "publish_macos", "recover_macos", "publish"})

    def test_an_asset_recovery_adds_only_after_its_own_build(self):
        self.assertEqual(job_needs("recover_macos"), ["resolve", "build_macos"])
        for status in ("failure", "cancelled", "skipped"):
            with self.subTest(status=status):
                outcome = results("workflow_dispatch", "macos-assets", {"build_macos": status})
                self.assertEqual(outcome["recover_macos"], "skipped")

    def test_both_publishers_run_the_same_steps(self):
        def steps(job):
            lines = job_block(job).splitlines()
            body = []
            for line in lines[lines.index("    steps:") + 1:]:
                if line and not line.startswith("      "):
                    break
                body.append(line)
            return "\n".join(body).rstrip()
        self.assertTrue(steps("publish_macos"))
        self.assertEqual(steps("publish_macos"), steps("recover_macos"))
        self.assertIn("python3 scripts/connect_release_macos_assets.py", steps("publish_macos"))
        self.assertNotIn("--clobber", WORKFLOW.read_text())

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


def run_step(step: str, *, core=(), meta=(), index=None, after=None, env=None, job="floor_check"):
    """Run one step's shell (floor_check's unless `job` names another),
    verbatim, in a fixture checkout with stub cargo, curl, date and sleep.
    Returns the process result, the index paths read, the sleeps and the
    GITHUB_OUTPUT text."""
    script = indented_body(step, job, "run", 10)
    with TempDir() as root:
        (root / "bin").mkdir()
        (root / "scripts").mkdir()
        (root / "ci").mkdir()
        manifest = root / "connect/crates/meerkat-connect/Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(f'[package]\nname = "meerkat-connect"\nversion = "{TREE_VERSION}"\n')
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
# The Connect version of the fixture checkout every step runs in.
TREE_VERSION = "0.9.15"
TARGET = "aarch64-apple-darwin"


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


class ConnectResolveTests(unittest.TestCase):
    """The resolver's real shell: a tag push names the tag's own commit; a
    build-only dispatch names the dispatched commit; an asset recovery names
    an existing tag and takes the version from it, not from the dispatched
    tree."""

    SHA = "1" * 40

    def resolve(self, event, *, ref="", mode="", release_tag=""):
        result, _, _, output = run_step("Resolve", job="resolve", env={
            "EVENT": event, "REF": ref, "MODE": mode, "RELEASE_TAG": release_tag, "GITHUB_SHA": self.SHA})
        return result, dict(line.split("=", 1) for line in output.splitlines())

    def test_a_tag_push_builds_the_tag(self):
        result, output = self.resolve("push", ref=f"refs/tags/connect-v{TREE_VERSION}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, {"mode": "tag", "version": TREE_VERSION, "tag": f"connect-v{TREE_VERSION}",
                                  "source_ref": f"refs/tags/connect-v{TREE_VERSION}"})

    def test_a_tag_that_does_not_name_the_version_fails(self):
        result, output = self.resolve("push", ref="refs/tags/connect-v0.9.14")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(output, {})

    def test_a_build_only_dispatch_builds_the_dispatched_commit(self):
        result, output = self.resolve("workflow_dispatch", mode="macos-build")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, {"mode": "macos-build", "version": TREE_VERSION,
                                  "tag": f"connect-v{TREE_VERSION}", "source_ref": self.SHA})

    def test_an_asset_recovery_builds_the_named_tag_at_its_own_version(self):
        result, output = self.resolve("workflow_dispatch", mode="macos-assets", release_tag="connect-v0.9.14")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, {"mode": "macos-assets", "version": "0.9.14", "tag": "connect-v0.9.14",
                                  "source_ref": "refs/tags/connect-v0.9.14"})

    def test_an_asset_recovery_refuses_anything_but_a_connect_release_tag(self):
        for tag in ("", "v0.9.14", "meta-v0.9.14", "connect-v0.9", "connect-v0.9.14-rc1", "connect-v0.9.14 x",
                    "connect-v0.9.14;id", "refs/tags/connect-v0.9.14", "main"):
            with self.subTest(tag=tag):
                result, output = self.resolve("workflow_dispatch", mode="macos-assets", release_tag=tag)
                self.assertEqual(result.returncode, 1)
                self.assertEqual(output, {})

    def test_a_release_tag_is_refused_in_every_other_mode(self):
        for mode in ("candidate", "accept", "publish", "macos-build"):
            with self.subTest(mode=mode):
                result, output = self.resolve("workflow_dispatch", mode=mode, release_tag="connect-v0.9.14")
                self.assertEqual(result.returncode, 1)
                self.assertEqual(output, {})

    def test_the_candidate_modes_resolve_as_before(self):
        for mode in ("candidate", "accept", "publish"):
            with self.subTest(mode=mode):
                result, output = self.resolve("workflow_dispatch", mode=mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((output["mode"], output["version"], output["tag"]),
                                 (mode, TREE_VERSION, f"connect-v{TREE_VERSION}"))


BUILD_STEPS = (
    "Checkout the release source",
    "Check that the checkout is the release source at the release version",
    "Install Rust",
    "Build the executables",
    "Verify macOS binary architecture",
    "Smoke-run the executables",
    "Ad-hoc codesign macOS binaries",
    "Package artifacts",
    "Attest build provenance",
    "Retain original provenance bundle",
    "Upload artifacts",
)
ARCHIVES = {"meerkat-connect": f"meerkat-connect-{TREE_VERSION}-{TARGET}.tar.gz",
            "toolkit_gateway": f"meerkat-connect-gateway-{TREE_VERSION}-{TARGET}.tar.gz"}


def run_build_step(step: str, root: Path, env: dict):
    script = indented_body(step, "build_macos", "run", 10)
    return subprocess.run(["bash", "-c", script], cwd=root, capture_output=True, text=True,
                          env={**os.environ, "TARGET": TARGET, "VERSION": TREE_VERSION, **env})


def stand_in_executables(root: Path, outputs: dict):
    """target/<triple>/release/<bin> stand-ins that print `outputs[bin]` for
    --version, as the smoke run and the packager read the real ones."""
    release = root / "target" / TARGET / "release"
    release.mkdir(parents=True)
    for name, output in outputs.items():
        path = release / name
        path.write_text(f"#!/bin/sh\necho '{output}'\n")
        path.chmod(0o755)
    return release


class ConnectMacosBuildTests(unittest.TestCase):
    """The build job is Meta's gateway leg for Connect's two executables:
    one hosted macos-15 row, Cargo through the repository wrapper, then
    architecture check, smoke run, ad-hoc signing, one archive per
    executable with its checksum, provenance, artifact."""

    def test_one_hosted_apple_silicon_row(self):
        block = job_block("build_macos")
        self.assertIn("    runs-on: ${{ matrix.runs-on }}\n", block)
        rows = re.findall(r"(?m)^          - runs-on: (\S+)\n            target: (\S+)$", block)
        self.assertEqual(rows, [("macos-15", TARGET)])

    def test_the_steps_run_in_metas_order(self):
        block = job_block("build_macos")
        named = re.findall(r"(?m)^      - name: (.+)$", block)
        self.assertEqual(tuple(named), BUILD_STEPS)

    def test_the_build_is_the_linux_lanes_cargo_selection_without_a_resource_digest(self):
        body = indented_body("Build the executables", "build_macos", "run", 10)
        command = " ".join(line.strip().rstrip("\\").strip() for line in body.splitlines()
                           if line.strip() and not line.strip().startswith("#"))
        self.assertIn(
            "env -u MEERKAT_CONNECT_RESOURCES_SHA256 ./scripts/repo-cargo build --locked --release "
            '--target "${TARGET}" -p meerkat-connect '
            "--features meerkat-connect/console,toolkit-mobkit/native-host "
            "--bin meerkat-connect --bin toolkit_gateway", command)
        self.assertIn("uses: dtolnay/rust-toolchain@f08ac54821369508305fcb9e0121ec6e30ae9ada # 1.97.0",
                      step_block("Install Rust", "build_macos"))
        self.assertIn("targets: ${{ matrix.target }}", step_block("Install Rust", "build_macos"))

    def test_both_executables_are_checked_and_ad_hoc_signed_with_apples_tools(self):
        arch = indented_body("Verify macOS binary architecture", "build_macos", "run", 10)
        sign = indented_body("Ad-hoc codesign macOS binaries", "build_macos", "run", 10)
        for body in (arch, sign):
            self.assertIn("for bin in meerkat-connect toolkit_gateway; do", body)
        self.assertIn('aarch64-apple-darwin) expected="arm64" ;;', arch)
        self.assertIn('/usr/bin/lipo -archs "${path}"', arch)
        self.assertIn('/usr/bin/file -b "${path}"', arch)
        self.assertIn('/usr/bin/codesign --force --sign - "target/${TARGET}/release/${bin}"', sign)
        # No other signing identity, notarization or stapling anywhere.
        text = WORKFLOW.read_text()
        for word in ("notarytool", "stapler", "altool", "Developer ID", "--sign \""):
            self.assertNotIn(word, text)

    def test_the_build_job_cannot_write_the_repository(self):
        block = job_block("build_macos")
        permissions = block[block.index("    permissions:"):block.index("    strategy:")]
        self.assertEqual(sorted(line.strip() for line in permissions.splitlines()[1:]),
                         ["attestations: write", "contents: read", "id-token: write"])
        self.assertNotIn("GH_TOKEN", block)
        self.assertIn("persist-credentials: false", step_block("Checkout the release source", "build_macos"))
        self.assertIn("ref: ${{ needs.resolve.outputs.source_ref }}",
                      step_block("Checkout the release source", "build_macos"))

    def test_the_smoke_run_requires_both_executables_to_print_the_release_version(self):
        good = {"meerkat-connect": f"meerkat-connect {TREE_VERSION}",
                "toolkit_gateway": f"toolkit_gateway {TREE_VERSION} (local Toolkit host over MobKit SDK RPC)"}
        with TempDir() as root:
            stand_in_executables(root, good)
            result = run_build_step("Smoke-run the executables", root, {})
            self.assertEqual(result.returncode, 0, result.stderr)
        for name, output in (("meerkat-connect", "meerkat-connect 0.9.14"),
                             ("meerkat-connect", f"meerkat-connect {TREE_VERSION}0"),
                             ("toolkit_gateway", f"toolkit_gateway {TREE_VERSION}1 (host)"),
                             ("toolkit_gateway", f"gateway {TREE_VERSION} (host)")):
            with self.subTest(name=name, output=output), TempDir() as root:
                stand_in_executables(root, {**good, name: output})
                result = run_build_step("Smoke-run the executables", root, {})
                self.assertEqual(result.returncode, 1)
                self.assertIn(f"{name} --version printed", result.stderr)

    def test_packaging_writes_one_archive_per_executable_with_its_checksum(self):
        with TempDir() as root:
            stand_in_executables(root, {"meerkat-connect": "a", "toolkit_gateway": "b"})
            result = run_build_step("Package artifacts", root, {})
            self.assertEqual(result.returncode, 0, result.stderr)
            dist = root / "dist"
            self.assertEqual(sorted(path.name for path in dist.iterdir()),
                             sorted([*ARCHIVES.values(), *(f"{name}.sha256" for name in ARCHIVES.values())]))
            for member, archive in ARCHIVES.items():
                listing = subprocess.run(["tar", "-tzf", str(dist / archive)], check=True, capture_output=True,
                                         text=True).stdout.split()
                self.assertEqual(listing, [member])
                digest = subprocess.run(["sha256sum", archive], cwd=dist, check=True, capture_output=True,
                                        text=True).stdout
                self.assertEqual((dist / f"{archive}.sha256").read_text(), digest)

    def test_packaging_fails_without_an_executable(self):
        with TempDir() as root:
            stand_in_executables(root, {"meerkat-connect": "a"})
            result = run_build_step("Package artifacts", root, {})
            self.assertEqual(result.returncode, 1)
            self.assertIn("missing expected binary", result.stderr)

    def source_check(self, *, mode, head_is_expected=True, version=TREE_VERSION):
        git = ["git", "-c", "user.name=test", "-c", "user.email=test@example.invalid",
               "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"]
        with TempDir() as root:
            manifest = root / "connect/crates/meerkat-connect/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text(f'[package]\nname = "meerkat-connect"\nversion = "{version}"\n')
            for command in (["init", "-q", "-b", "main"], ["add", "-A"], ["commit", "-q", "-m", "release"],
                            ["tag", "-a", "-m", "release", f"connect-v{TREE_VERSION}"]):
                subprocess.run([*git, *command], cwd=root, check=True, capture_output=True)
            released = subprocess.run([*git, "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True,
                                      text=True).stdout.strip()
            if not head_is_expected:
                (root / "later").write_text("later\n")
                for command in (["add", "-A"], ["commit", "-q", "-m", "later"]):
                    subprocess.run([*git, *command], cwd=root, check=True, capture_output=True)
            return run_build_step("Check that the checkout is the release source at the release version", root,
                                  {"MODE": mode, "TAG": f"connect-v{TREE_VERSION}", "GITHUB_SHA": released})

    def test_the_build_refuses_a_checkout_that_is_not_the_release_source(self):
        for mode in ("tag", "macos-assets", "macos-build"):
            with self.subTest(mode=mode):
                self.assertEqual(self.source_check(mode=mode).returncode, 0)
                wrong = self.source_check(mode=mode, head_is_expected=False)
                self.assertEqual(wrong.returncode, 1)
                self.assertIn("is not", wrong.stderr)
                stale = self.source_check(mode=mode, version="0.9.14")
                self.assertEqual(stale.returncode, 1)
                self.assertIn("0.9.14", stale.stderr)


GH_STUB = (
    f"#!{sys.executable}\n"
    "import json, os, shutil, sys\n"
    "state_path, store = os.environ['GH_STATE'], os.environ['GH_STORE']\n"
    "state = json.load(open(state_path))\n"
    "args = sys.argv[1:]\n"
    "state['calls'].append(args)\n"
    "def save():\n"
    "    json.dump(state, open(state_path, 'w'))\n"
    "save()\n"
    "def value(flag):\n"
    "    return args[args.index(flag) + 1]\n"
    "if '--clobber' in args or args[:2] not in (['release', 'view'], ['release', 'download'], ['release', 'upload']):\n"
    "    print('stub gh: refused ' + ' '.join(args), file=sys.stderr)\n"
    "    sys.exit(97)\n"
    "if value('--repo') != 'lukacf/meerkat':\n"
    "    sys.exit(96)\n"
    "release = state['release']\n"
    "if release is None:\n"
    "    print('release not found', file=sys.stderr)\n"
    "    sys.exit(1)\n"
    "if args[1] == 'view':\n"
    "    assert value('--json') == 'tagName,isDraft,assets', args\n"
    "    hidden = set(state.get('hidden', []))\n"
    "    print(json.dumps({**release, 'assets': [{'name': name} for name in sorted(os.listdir(store))\n"
    "                                            if name not in hidden]}))\n"
    "elif args[1] == 'download':\n"
    "    source = os.path.join(store, value('--pattern'))\n"
    "    if not os.path.isfile(source):\n"
    "        print('no assets match the file pattern', file=sys.stderr)\n"
    "        sys.exit(1)\n"
    "    shutil.copyfile(source, os.path.join(value('--dir'), value('--pattern')))\n"
    "else:\n"
    "    path = args[3]\n"
    "    name = os.path.basename(path)\n"
    "    if os.path.exists(os.path.join(store, name)):\n"
    "        print('asset under the same name already exists', file=sys.stderr)\n"
    "        sys.exit(1)\n"
    "    shutil.copyfile(path, os.path.join(store, name))\n"
    "    state['uploads'].append(name)\n"
    "    save()\n"
)


def write_archive(path: Path, members: dict):
    """A gzip tar of `members` (name to bytes), each an executable file."""
    with tarfile.open(path, "w:gz") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(data))


def sha256_line(path: Path) -> str:
    return f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"


def macos_names(version=TREE_VERSION):
    archives = [f"meerkat-connect-{version}-{TARGET}.tar.gz", f"meerkat-connect-gateway-{version}-{TARGET}.tar.gz"]
    return [archives[0], f"{archives[0]}.sha256", archives[1], f"{archives[1]}.sha256",
            f"connect-{TARGET}-provenance.jsonl"]


def write_artifact(download: Path, *, attempt=1, version=TREE_VERSION, payload=b"built"):
    """What one build_macos attempt uploads: two archives, their checksum
    files and the provenance bundle."""
    folder = download / f"connect-macos-binaries-{TARGET}-{attempt}"
    folder.mkdir(parents=True)
    names = macos_names(version)
    write_archive(folder / names[0], {"meerkat-connect": payload + b" cli"})
    write_archive(folder / names[2], {"toolkit_gateway": payload + b" gateway"})
    for archive in (names[0], names[2]):
        (folder / f"{archive}.sha256").write_text(sha256_line(folder / archive))
    (folder / names[4]).write_text('{"bundle": 1}\n')
    return folder


class ConnectMacosPublisherTests(unittest.TestCase):
    """scripts/connect_release_macos_assets.py adds exactly the archives one
    build attempt uploaded (with their checksum files and provenance) to an
    existing, published release: every file is checked first, an asset
    already there with the same bytes is left, one with other bytes fails the
    run before anything is uploaded, and nothing is ever replaced."""

    TAG = f"connect-v{TREE_VERSION}"
    # What the release holds before: the release owner's own files.
    OWNER_ASSETS = {"SHA256SUMS": b"owner sums\n", "RELEASE-MANIFEST.json": b"{}\n",
                    f"meerkat-connect-{TREE_VERSION}-linux-x86_64.tar.gz": b"linux\n"}

    def publish(self, prepare=None, *, release="published", existing=None, tag=None, version=TREE_VERSION,
                hidden=()):
        with TempDir() as root:
            download, store, work = root / "download", root / "store", root / "work"
            download.mkdir()
            store.mkdir()
            (root / "bin").mkdir()
            (root / "bin/gh").write_text(GH_STUB)
            (root / "bin/gh").chmod(0o755)
            artifact = write_artifact(download)
            if prepare:
                prepare(download, artifact)
            for name, data in {**self.OWNER_ASSETS, **(existing or {})}.items():
                (store / name).write_bytes(data)
            state = root / "state.json"
            state.write_text(json.dumps({
                "calls": [], "uploads": [], "hidden": list(hidden),
                "release": None if release is None else {"tagName": tag or self.TAG,
                                                         "isDraft": release == "draft"}}))
            result = subprocess.run(
                [sys.executable, str(PUBLISHER), "--tag", self.TAG, "--version", version,
                 "--download", str(download), "--work", str(work)],
                capture_output=True, text=True,
                env={**os.environ, "PATH": f"{root / 'bin'}:{os.environ['PATH']}", "GH_STATE": str(state),
                     "GH_STORE": str(store), "GITHUB_REPOSITORY": "lukacf/meerkat"})
            recorded = json.loads(state.read_text())
            stored = {path.name: path.read_bytes() for path in store.iterdir()}
            built = {path.name: path.read_bytes() for path in artifact.iterdir()} if artifact.exists() else {}
            return result, recorded, stored, built

    def assert_refused(self, result, recorded, stored, message):
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(message, result.stderr)
        self.assertEqual(recorded["uploads"], [])
        for name, data in self.OWNER_ASSETS.items():
            self.assertEqual(stored[name], data)

    def test_a_published_release_gets_exactly_the_built_files(self):
        result, recorded, stored, built = self.publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(recorded["uploads"], macos_names())
        self.assertEqual(set(stored), set(self.OWNER_ASSETS) | set(macos_names()))
        for name in macos_names():
            self.assertEqual(stored[name], built[name])
        for name, data in self.OWNER_ASSETS.items():
            self.assertEqual(stored[name], data)
        self.assertFalse([call for call in recorded["calls"] if "--clobber" in call])

    def test_a_second_run_leaves_the_same_bytes_and_uploads_nothing(self):
        captured = {}

        def keep(download, artifact):
            captured.update({path.name: path.read_bytes() for path in artifact.iterdir()})
        result, recorded, stored, _ = self.publish(keep, existing=None)
        self.assertEqual(result.returncode, 0, result.stderr)
        # The same bytes again, already on the release.
        again, recorded, stored, _ = self.publish(
            lambda download, artifact: [(artifact / name).write_bytes(data) for name, data in captured.items()],
            existing=captured)
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(recorded["uploads"], [])
        self.assertIn("already on the release with the same bytes", again.stdout)

    def test_only_the_missing_files_are_added(self):
        captured = {}

        def keep(download, artifact):
            captured.update({path.name: path.read_bytes() for path in artifact.iterdir()})
        self.publish(keep)
        present = {name: captured[name] for name in macos_names()[:2]}
        result, recorded, stored, _ = self.publish(
            lambda download, artifact: [(artifact / name).write_bytes(data) for name, data in captured.items()],
            existing=present)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(recorded["uploads"], macos_names()[2:])

    def test_an_asset_with_other_bytes_fails_before_any_upload(self):
        for name in macos_names():
            with self.subTest(name=name):
                result, recorded, stored, _ = self.publish(existing={name: b"other bytes\n"})
                self.assert_refused(result, recorded, stored, "other bytes")
                self.assertEqual(stored[name], b"other bytes\n")

    def test_no_release_a_draft_or_another_tag_is_refused(self):
        for kwargs, message in (({"release": None}, "no release"), ({"release": "draft"}, "draft"),
                                ({"tag": "connect-v0.9.14"}, "connect-v0.9.14")):
            with self.subTest(**kwargs):
                result, recorded, stored, _ = self.publish(**kwargs)
                self.assert_refused(result, recorded, stored, message)

    def test_an_incomplete_or_changed_artifact_is_refused(self):
        names = macos_names()

        def remove(name):
            return lambda download, artifact: (artifact / name).unlink()

        def extra(download, artifact):
            (artifact / "SHA256SUMS").write_text("x\n")

        def changed(download, artifact):
            with open(artifact / names[0], "ab") as archive:
                archive.write(b"\0")

        def wrong_name(download, artifact):
            (artifact / f"{names[2]}.sha256").write_text(
                sha256_line(artifact / names[2]).replace(names[2], names[0]))

        def two_members(download, artifact):
            write_archive(artifact / names[0], {"meerkat-connect": b"a", "extra": b"b"})
            (artifact / f"{names[0]}.sha256").write_text(sha256_line(artifact / names[0]))

        def wrong_member(download, artifact):
            write_archive(artifact / names[2], {"meerkat-connect": b"a"})
            (artifact / f"{names[2]}.sha256").write_text(sha256_line(artifact / names[2]))

        def empty_bundle(download, artifact):
            (artifact / names[4]).write_text("")

        def no_artifact(download, artifact):
            shutil.rmtree(artifact)

        cases = [(f"without {name}", remove(name), name) for name in names]
        cases += [("an extra file", extra, "SHA256SUMS"), ("changed archive bytes", changed, "checksum"),
                  ("a checksum file naming another archive", wrong_name, "checksum"),
                  ("two members", two_members, "exactly one"), ("another member", wrong_member, "toolkit_gateway"),
                  ("an empty provenance bundle", empty_bundle, "empty"),
                  ("no artifact", no_artifact, "no built macOS archives")]
        for case, prepare, message in cases:
            with self.subTest(case=case):
                result, recorded, stored, _ = self.publish(prepare)
                self.assert_refused(result, recorded, stored, message)
                # Refused before the release is even read.
                self.assertEqual(recorded["calls"], [])

    def test_the_newest_build_attempt_is_the_one_published(self):
        def rerun(download, artifact):
            write_artifact(download, attempt=10, payload=b"rebuilt")
        result, recorded, stored, _ = self.publish(rerun)
        self.assertEqual(result.returncode, 0, result.stderr)
        with TempDir() as scratch:
            expected = write_artifact(scratch, attempt=10, payload=b"rebuilt")
            with tarfile.open(expected / macos_names()[0]) as archive:
                wanted = archive.extractfile("meerkat-connect").read()
        with tarfile.open(fileobj=io.BytesIO(stored[macos_names()[0]])) as archive:
            self.assertEqual(archive.extractfile("meerkat-connect").read(), wanted)

    def test_archives_of_another_version_are_refused(self):
        result, recorded, stored, _ = self.publish(version="0.9.16")
        self.assert_refused(result, recorded, stored, "0.9.16")

    def test_an_upload_the_release_does_not_show_fails_the_run(self):
        result, recorded, stored, _ = self.publish(hidden=[macos_names()[4]])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not list", result.stderr)

    def test_the_workflow_passes_the_resolved_tag_and_version(self):
        step = step_block("Add exactly the built archives (never replacing one)", "publish_macos")
        self.assertIn("TAG: ${{ needs.resolve.outputs.tag }}", step)
        self.assertIn("VERSION: ${{ needs.resolve.outputs.version }}", step)
        self.assertIn('--tag "${TAG}" --version "${VERSION}"', step)
        download = step_block("Download this run's macOS archives", "publish_macos")
        self.assertIn('gh run download "${GITHUB_RUN_ID}"', download)
        self.assertIn("--pattern 'connect-macos-binaries-*'", download)


STAGE_JOBS = ("stage_macos_build", "stage_macos_check")
CHECKOUT_ROOT = "/Users/runner/work/meerkat/meerkat"
WORK_ROOT = "/private/tmp/cw"
PINS_DIRECTORY = "/private/tmp/cw/pins"
RESOLVED_PINS = "/private/tmp/cw/pins/library-pins-macos-arm64.resolved.json"
PRODUCER = "python3 connect/scripts/connect-candidate.py macos"
WITH_PINS = '--work "${CONNECT_WORK}" --pins "${CONNECT_RESOLVED_PINS}" --pinned-inputs "${CONNECT_PINS}"'
STAGE_BUILD_STEPS = (
    "Checkout the dispatched source",
    "Check the runner layout",
    "Select Python",
    "Install Rust",
    "Install zstd",
    "Preflight",
    "Fetch the pinned public inputs",
    "Build the integration library",
    "Package the resources",
    "Build and sign the executables",
    "Free the Cargo output",
    "Pack the work root for the check job",
    "Upload the work root",
)
STAGE_CHECK_STEPS = (
    "Checkout the dispatched source",
    "Check the runner layout",
    "Select Python",
    "Install Rust",
    "Install zstd",
    "Download the work root",
    "Restore the work root at its path",
    "Stage the products and build the test executables",
    "Check the installed gateway",
    "Check the installed CLI",
    "Check a fresh rkat consumer",
    "Check the setup conversation start",
    "Check the official Playwright install",
    "Write the stage record",
    "Attest the stage record",
    "Retain the provenance bundle",
    "Upload the stage record",
    "Upload the stage provenance",
)
# The producer's one call per step: job, workflow step, the words after `connect-candidate.py macos`.
PRODUCER_CALLS = (
    ("stage_macos_build", "Preflight", 'preflight --work "${CONNECT_WORK}" --source-sha "${GITHUB_SHA}"'),
    ("stage_macos_build", "Build the integration library", f"library {WITH_PINS}"),
    ("stage_macos_build", "Package the resources", f"resources {WITH_PINS}"),
    ("stage_macos_build", "Build and sign the executables", 'build --work "${CONNECT_WORK}"'),
    ("stage_macos_check", "Stage the products and build the test executables", f"stage {WITH_PINS}"),
    ("stage_macos_check", "Check the installed gateway", 'check-gateway --work "${CONNECT_WORK}"'),
    ("stage_macos_check", "Check the installed CLI", 'check-cli --work "${CONNECT_WORK}"'),
    ("stage_macos_check", "Check the setup conversation start", 'check-conversation --work "${CONNECT_WORK}"'),
    ("stage_macos_check", "Check the official Playwright install",
     'check-official-install --work "${CONNECT_WORK}"'),
    ("stage_macos_check", "Write the stage record",
     'stage-record --work "${CONNECT_WORK}" --output "${CONNECT_WORK}/artifact"'),
)
UNAME_STUB = "#!/bin/sh\ncase \"$1\" in -s) echo Darwin ;; -m) echo arm64 ;; *) echo Darwin ;; esac\n"
PRODUCER_STUB = (
    f"#!{sys.executable}\n"
    "import json, os, sys\n"
    "with open(os.environ['STUB_LOG'], 'a') as log:\n"
    "    log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
)
CURL_STUB = (
    f"#!{sys.executable}\n"
    "import json, os, shutil, sys\n"
    "args = sys.argv[1:]\n"
    "url, destination = args[-1], args[args.index('-o') + 1]\n"
    "with open(os.environ['CURL_LOG'], 'a') as log:\n"
    "    log.write(url + '\\n')\n"
    "source = os.path.join(os.environ['CURL_FILES'], url.rsplit('/', 1)[1])\n"
    "if not os.path.exists(source):\n"
    "    print('curl: (22) The requested URL returned error: 404', file=sys.stderr)\n"
    "    sys.exit(22)\n"
    "shutil.copyfile(source, destination)\n"
)


def one_line(step: str, job: str) -> str:
    """A step's shell as one line: continuations joined, comments dropped."""
    body = indented_body(step, job, "run", 10)
    return " ".join(line.strip().rstrip("\\").strip() for line in body.splitlines()
                    if line.strip() and not line.strip().startswith("#"))


def job_mapping(job: str, key: str) -> dict:
    """A job's own `permissions:` or `env:` mapping (the four-space key and its six-space rows)."""
    lines = job_block(job).splitlines()
    start = lines.index(f"    {key}:")
    rows = {}
    for line in lines[start + 1:]:
        if not line.startswith("      ") or line.startswith("       "):
            break
        name, value = line.strip().split(":", 1)
        rows[name] = value.strip().strip('"').strip("'")
    return rows


def run_stage_step(step: str, job: str, root: Path, env: dict):
    script = indented_body(step, job, "run", 10)
    return subprocess.run(["bash", "-c", script], cwd=root, capture_output=True, text=True,
                          env={**os.environ, **env})


def load_fetch_script():
    import importlib.util
    spec = importlib.util.spec_from_file_location("connect_fetch_pinned_inputs",
                                                  ROOT / "connect/scripts/fetch-pinned-inputs.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ConnectMacosStageTests(unittest.TestCase):
    """Stage one of the complete macOS installation: a dispatch, two hosted macos-15 jobs, the producer
    once per step, the stage record as a workflow artifact, nothing published."""

    def test_the_stage_is_a_dispatch_mode_with_an_optional_core_release(self):
        text = WORKFLOW.read_text()
        self.assertIn("        options: [candidate, accept, publish, macos-build, macos-assets, macos-stage, "
                      "macos-assemble]\n", text)
        inputs = text[text.index("    inputs:\n"):text.index("\npermissions:\n")]
        block = inputs[inputs.index("      rkat_release_tag:\n"):]
        self.assertIn('        required: false\n        default: ""\n        type: string\n', block[:400])

    def test_only_the_stage_dispatch_runs_the_stage_jobs(self):
        outcome = results("workflow_dispatch", "macos-stage")
        self.assertEqual({job for job, result in outcome.items() if result == "success"},
                         {"resolve", *STAGE_JOBS})
        for event, mode in (("push", "tag"), ("workflow_dispatch", "candidate"), ("workflow_dispatch", "accept"),
                            ("workflow_dispatch", "publish"), ("workflow_dispatch", "macos-build"),
                            ("workflow_dispatch", "macos-assets")):
            with self.subTest(mode=mode):
                outcome = results(event, mode)
                self.assertEqual([outcome[job] for job in STAGE_JOBS], ["skipped", "skipped"])
        self.assertEqual(job_needs("stage_macos_build"), ["resolve"])
        self.assertEqual(job_needs("stage_macos_check"), ["resolve", "stage_macos_build"])
        for status in ("failure", "cancelled", "skipped"):
            with self.subTest(status=status):
                outcome = results("workflow_dispatch", "macos-stage", {"stage_macos_build": status})
                self.assertEqual(outcome["stage_macos_check"], "skipped")

    def test_the_resolver_names_the_dispatched_commit(self):
        sha = "2" * 40
        result, _, _, output = run_step("Resolve", job="resolve", env={
            "EVENT": "workflow_dispatch", "REF": "", "MODE": "macos-stage", "RELEASE_TAG": "", "GITHUB_SHA": sha})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(dict(line.split("=", 1) for line in output.splitlines()),
                         {"mode": "macos-stage", "version": TREE_VERSION, "tag": f"connect-v{TREE_VERSION}",
                          "source_ref": sha})
        result, _, _, output = run_step("Resolve", job="resolve", env={
            "EVENT": "workflow_dispatch", "REF": "", "MODE": "macos-stage", "RELEASE_TAG": "connect-v0.9.14",
            "GITHUB_SHA": sha})
        self.assertEqual((result.returncode, output), (1, ""))

    def test_both_jobs_are_hosted_macos_15_jobs_inside_the_job_limit(self):
        for job in STAGE_JOBS:
            with self.subTest(job=job):
                block = job_block(job)
                self.assertIn("    runs-on: macos-15\n", block)
                timeout = int(re.search(r"(?m)^    timeout-minutes: ([0-9]+)$", block).group(1))
                self.assertLessEqual(timeout, 360)

    def test_the_stage_jobs_hold_no_token_and_publish_nothing(self):
        self.assertEqual(job_mapping("stage_macos_build", "permissions"), {"contents": "read"})
        self.assertEqual(job_mapping("stage_macos_check", "permissions"),
                         {"contents": "read", "id-token": "write", "attestations": "write"})
        for job in STAGE_JOBS:
            block = job_block(job)
            for word in ("secrets.", "github.token", "GH_TOKEN", "GITHUB_TOKEN", "API_KEY", "gh release",
                         "gh api", "gh run", "git push", "git tag", "contents: write", "--clobber",
                         "notarytool", "Developer ID"):
                with self.subTest(job=job, word=word):
                    self.assertNotIn(word, block)
            # Whatever leaves a job leaves it as a workflow artifact of this run.
            uses = re.findall(r"(?m)^        uses: ([^@\s]+)@", block)
            self.assertLessEqual(set(uses), {"actions/checkout", "actions/setup-python", "dtolnay/rust-toolchain",
                                             "actions/upload-artifact", "actions/download-artifact",
                                             "actions/attest-build-provenance"})

    def test_the_checkout_and_the_work_root_are_the_same_literals_in_both_jobs(self):
        fetch = load_fetch_script()
        self.assertEqual(RESOLVED_PINS, f"{PINS_DIRECTORY}/{fetch.resolved_name('macos-arm64')}")
        for job in STAGE_JOBS:
            with self.subTest(job=job):
                self.assertEqual(job_mapping(job, "env"), {
                    "CONNECT_CHECKOUT": CHECKOUT_ROOT, "CONNECT_WORK": WORK_ROOT, "CONNECT_PINS": PINS_DIRECTORY,
                    "CONNECT_RESOLVED_PINS": RESOLVED_PINS, "PYTHONDONTWRITEBYTECODE": "1",
                    "CARGO_TERM_COLOR": "never"})
                checkout = step_block("Checkout the dispatched source", job)
                for line in ("ref: ${{ needs.resolve.outputs.source_ref }}", "fetch-depth: 0",
                             "persist-credentials: false"):
                    self.assertIn(f"          {line}\n", checkout + "\n")
                self.assertNotIn("          path:", checkout)
        self.assertEqual(step_block("Check the runner layout", "stage_macos_build").rstrip(),
                         step_block("Check the runner layout", "stage_macos_check").rstrip())

    def test_the_steps_run_in_the_producers_order(self):
        named = {job: tuple(re.findall(r"(?m)^      - name: (.+)$", job_block(job))) for job in STAGE_JOBS}
        self.assertEqual(named["stage_macos_build"], STAGE_BUILD_STEPS)
        self.assertEqual(named["stage_macos_check"], STAGE_CHECK_STEPS)
        for job, step, words in PRODUCER_CALLS:
            with self.subTest(step=step):
                self.assertEqual(one_line(step, job), f"set -euo pipefail {PRODUCER} {words}")
        called = re.findall(r"connect-candidate\.py macos ([a-z-]+)", "".join(job_block(job) for job in STAGE_JOBS))
        self.assertEqual(called, ["preflight", "library", "resources", "build", "stage", "check-gateway",
                                  "check-cli", "check-consumer", "check-consumer", "check-conversation",
                                  "check-official-install", "stage-record"])
        for job in STAGE_JOBS:
            for step in ("Select Python", "Install Rust"):
                self.assertEqual(step_block(step, "stage_macos_build").rstrip(), step_block(step, job).rstrip())
        self.assertIn("python-version: '3.11'", step_block("Select Python", "stage_macos_build"))
        rust = step_block("Install Rust", "stage_macos_build")
        self.assertIn("uses: dtolnay/rust-toolchain@f08ac54821369508305fcb9e0121ec6e30ae9ada # 1.97.0", rust)
        self.assertIn("targets: aarch64-apple-darwin", rust)
        zstd = step_block("Install zstd", "stage_macos_build")
        self.assertIn("HOMEBREW_NO_AUTO_UPDATE: '1'", zstd)
        self.assertIn("brew install zstd", zstd)
        self.assertEqual(zstd.rstrip(), step_block("Install zstd", "stage_macos_check").rstrip())

    def layout(self, *, change=None, uname=True, env=None):
        git = ["git", "-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"]
        with TempDir() as scratch:
            scratch = scratch.resolve()
            root, bin_dir = scratch / "checkout", scratch / "bin"
            root.mkdir()
            bin_dir.mkdir()
            (scratch / "private").mkdir()
            (root / "file").write_text("tracked\n")
            for command in (["init", "-q", "-b", "main"], ["add", "-A"], ["commit", "-q", "-m", "source"]):
                subprocess.run([*git, *command], cwd=root, check=True, capture_output=True)
            head = subprocess.run([*git, "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True,
                                  text=True).stdout.strip()
            if uname:
                (bin_dir / "uname").write_text(UNAME_STUB)
                (bin_dir / "uname").chmod(0o755)
            values = {"CONNECT_CHECKOUT": str(root), "CONNECT_WORK": str(scratch / "private" / "cw"),
                      "GITHUB_SHA": head, "PATH": f"{bin_dir}:{os.environ['PATH']}", **(env or {})}
            if change:
                change(scratch, root, values)
            return run_stage_step("Check the runner layout", "stage_macos_build", root, values)

    def test_the_layout_check_accepts_exactly_the_recorded_layout(self):
        result = self.layout()
        self.assertEqual(result.returncode, 0, result.stderr)

        def elsewhere(scratch, root, values):
            values["CONNECT_CHECKOUT"] = CHECKOUT_ROOT

        def work_exists(scratch, root, values):
            (scratch / "private" / "cw").mkdir()

        def linked_parent(scratch, root, values):
            (scratch / "link").symlink_to(scratch / "private")
            values["CONNECT_WORK"] = str(scratch / "link" / "cw")

        def another_commit(scratch, root, values):
            values["GITHUB_SHA"] = "3" * 40

        def dirty(scratch, root, values):
            (root / "untracked").write_text("left behind\n")

        for name, change, message in (("another checkout path", elsewhere, "the checkout is at"),
                                      ("an existing work root", work_exists, "already exists"),
                                      ("a linked parent", linked_parent, "not a real directory"),
                                      ("another commit", another_commit, "is not the dispatched commit"),
                                      ("a dirty checkout", dirty, "not clean")):
            with self.subTest(name=name):
                result = self.layout(change=change)
                self.assertEqual(result.returncode, 1)
                self.assertIn(message, result.stderr)
        if os.uname().sysname != "Darwin" or os.uname().machine != "arm64":
            result = self.layout(uname=False)
            self.assertEqual(result.returncode, 1)
            self.assertIn("not a macOS arm64 runner", result.stderr)

    def test_the_fetch_step_derives_the_crates_from_the_lock_and_keeps_its_record(self):
        fetch = load_fetch_script()
        step = step_block("Fetch the pinned public inputs", "stage_macos_build")
        self.assertIn('          CONNECT_PINS_TOOL_CLOSURE: "1"\n', step)
        command = one_line("Fetch the pinned public inputs", "stage_macos_build")
        arguments = " ".join('"{p}"' if word == "{p}" else word for word in fetch.TOOL_CLOSURE_ARGUMENTS)
        parts = [
            "(cd connect && python3 -B -m unittest scripts.test_fetch_pinned_inputs)",
            'connect/scripts/repo-cargo tree --locked --manifest-path "${CONNECT_CHECKOUT}/Cargo.toml" '
            f'{arguments} > "${{CONNECT_WORK}}/tool-closure.txt"',
            "python3 -I connect/scripts/fetch-pinned-inputs.py --pins connect/release/library-pins-macos-arm64.json "
            '--closure "${CONNECT_WORK}/tool-closure.txt" --lock Cargo.lock --destination "${CONNECT_PINS}"',
            'mkdir -p "${CONNECT_WORK}/records/pinned-inputs"',
            'cp "${CONNECT_PINS}/FETCHED.json" "${CONNECT_RESOLVED_PINS}" "${CONNECT_WORK}/records/pinned-inputs/"',
        ]
        self.assertEqual(command, "set -euo pipefail " + " ".join(parts))
        # The fetch step is the only step of either job that names a download host or tool.
        for job in STAGE_JOBS:
            for name in (STAGE_BUILD_STEPS if job == "stage_macos_build" else STAGE_CHECK_STEPS):
                block = step_block(name, job)
                if name not in ("Fetch the pinned public inputs", "Check a fresh rkat consumer", "Install zstd"):
                    for word in ("curl", "wget", "https://", "pip install", "npm install", "brew "):
                        self.assertNotIn(word, block, f"{job}: {name}")

    def test_cargo_text_that_a_step_hands_to_a_reader_is_forced_plain(self):
        # The jobs' Rust setup exports CARGO_TERM_COLOR=always, and Cargo then colours `cargo tree`
        # even into a file. Every Cargo command of the three jobs whose output a step redirects, pipes
        # or captures must say `--color never` itself; the reader refuses a line it does not know.
        fetch = load_fetch_script()
        self.assertEqual(fetch.TOOL_CLOSURE_ARGUMENTS[-2:], ("--color", "never"))
        read = []
        for job in (*STAGE_JOBS, "assemble_macos"):
            for step in re.findall(r"(?m)^      - name: (.+)$", job_block(job)):
                if "        run: |\n" not in step_block(step, job) + "\n":
                    continue
                body = re.sub(r"\\\n\s*", " ", indented_body(step, job, "run", 10))
                for command in body.splitlines():
                    if re.search(r"(?:^|[\s(/])(?:repo-)?cargo\s", command) and re.search(r"[|>]|\$\(", command):
                        read.append((job, step))
                        self.assertIn(" --color never ", command, f"{job}: {step}")
        self.assertEqual(read, [("stage_macos_build", "Fetch the pinned public inputs")])

    def test_the_three_jobs_ask_cargo_for_plain_output_before_the_rust_setup_can_ask_for_colour(self):
        # The second line behind the per-command `--color never`: the Rust setup action writes
        # CARGO_TERM_COLOR=always into the job only when the variable is unset, so a job that sets it
        # keeps `never` for every step. No step of the three jobs sets another value, and the
        # per-command flag stays (the test above): a command must not depend on this variable.
        for job in (*STAGE_JOBS, "assemble_macos"):
            with self.subTest(job=job):
                block = job_block(job)
                self.assertEqual(job_mapping(job, "env").get("CARGO_TERM_COLOR"), "never")
                self.assertEqual(re.findall(r"(?m)^ +CARGO_TERM_COLOR:.*$", block), ["      CARGO_TERM_COLOR: never"])
                self.assertNotIn("CARGO_TERM_COLOR=", re.sub(r"(?m)^ *#.*$", "", block))

    def work_root(self, scratch: Path):
        work = scratch / "private" / "cw"
        (work / "records").mkdir(parents=True)
        (work / "pins").mkdir()
        (work / "binary").mkdir()
        (work / "records/preflight.step.json").write_text("{}\n")
        (work / "pins/library-pins-macos-arm64.resolved.json").write_text("{}\n")
        (work / "binary/meerkat-connect").write_bytes(b"\xcf\xfa\xed\xfe signed")
        (work / "binary/meerkat-connect").chmod(0o755)
        (work / "records/build.log").write_text("log\n")
        (work / "records/build.log").chmod(0o644)
        (work / "current").symlink_to("binary/meerkat-connect")
        return work

    def pack(self, scratch: Path, work: Path):
        (scratch / "build-temp").mkdir()
        output = scratch / "github-output"
        result = run_stage_step("Pack the work root for the check job", "stage_macos_build", scratch, {
            "CONNECT_WORK": str(work), "RUNNER_TEMP": str(scratch / "build-temp"), "GITHUB_OUTPUT": str(output),
            "GITHUB_RUN_ID": "77", "GITHUB_RUN_ATTEMPT": "2"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.read_text(), "work_artifact=connect-macos-arm64-work-77-2\n")
        self.assertEqual(sorted(path.name for path in (scratch / "build-temp").iterdir()),
                         ["connect-work.tar", "connect-work.tar.sha256"])
        # What download-artifact leaves for the check job.
        downloaded = scratch / "check-temp" / "connect-work"
        downloaded.parent.mkdir()
        shutil.move(str(scratch / "build-temp"), str(downloaded))
        return downloaded

    def restore(self, scratch: Path, work: Path):
        return run_stage_step("Restore the work root at its path", "stage_macos_check", scratch, {
            "CONNECT_WORK": str(work), "RUNNER_TEMP": str(scratch / "check-temp"),
            "CONNECT_RESOLVED_PINS": str(work / "pins/library-pins-macos-arm64.resolved.json")})

    def test_the_work_root_travels_between_the_jobs_at_its_own_path(self):
        with TempDir() as scratch:
            scratch = scratch.resolve()
            work = self.work_root(scratch)
            downloaded = self.pack(scratch, work)
            shutil.rmtree(work)
            result = self.restore(scratch, work)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((work / "binary/meerkat-connect").read_bytes(), b"\xcf\xfa\xed\xfe signed")
            self.assertEqual((work / "binary/meerkat-connect").stat().st_mode & 0o777, 0o755)
            self.assertEqual((work / "records/build.log").stat().st_mode & 0o777, 0o644)
            self.assertEqual(os.readlink(work / "current"), "binary/meerkat-connect")
            self.assertEqual(sorted(path.name for path in work.iterdir()), ["binary", "current", "pins", "records"])
            self.assertFalse((downloaded / "connect-work.tar").exists(), "the copy is removed to free the disk")

    def test_the_restore_refuses_other_bytes_an_existing_root_and_an_incomplete_one(self):
        with TempDir() as scratch:
            scratch = scratch.resolve()
            work = self.work_root(scratch)
            downloaded = self.pack(scratch, work)
            result = self.restore(scratch, work)
            self.assertEqual(result.returncode, 1)
            self.assertIn("already exists", result.stderr)
            shutil.rmtree(work)
            with open(downloaded / "connect-work.tar", "ab") as archive:
                archive.write(b"changed")
            result = self.restore(scratch, work)
            self.assertEqual(result.returncode, 1)
            self.assertFalse(work.exists())
        with TempDir() as scratch:
            scratch = scratch.resolve()
            work = self.work_root(scratch)
            (work / "records/preflight.step.json").unlink()
            self.pack(scratch, work)
            shutil.rmtree(work)
            result = self.restore(scratch, work)
            self.assertEqual(result.returncode, 1)
            self.assertIn("is not a preflighted work root", result.stderr)

    def test_the_work_root_artifact_is_named_by_the_job_that_made_it(self):
        build = job_block("stage_macos_build")
        self.assertIn("    outputs:\n      work_artifact: ${{ steps.pack.outputs.work_artifact }}\n", build)
        self.assertIn("        id: pack\n", step_block("Pack the work root for the check job", "stage_macos_build") + "\n")
        upload = step_block("Upload the work root", "stage_macos_build") + "\n"
        self.assertIn("        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4\n", upload)
        for line in ("name: ${{ steps.pack.outputs.work_artifact }}", "if-no-files-found: error",
                     "overwrite: false", "include-hidden-files: true"):
            self.assertIn(f"          {line}\n", upload)
        download = step_block("Download the work root", "stage_macos_check") + "\n"
        self.assertIn("          name: ${{ needs.stage_macos_build.outputs.work_artifact }}\n", download)
        self.assertIn("          path: ${{ runner.temp }}/connect-work\n", download)
        # The Cargo output is freed before the pack: no later step reads it, and the runner's disk is small.
        self.assertEqual(one_line("Free the Cargo output", "stage_macos_build"),
                         "set -euo pipefail connect/scripts/repo-cargo clean df -k \"${CONNECT_WORK}\"")
        self.assertIn("          RUST_LANE_ID: connect-macos-arm64\n",
                      step_block("Free the Cargo output", "stage_macos_build") + "\n")

    def consumer(self, tag, *, files=None):
        with TempDir() as scratch:
            scratch = scratch.resolve()
            (scratch / "connect/scripts").mkdir(parents=True)
            (scratch / "connect/scripts/connect-candidate.py").write_text(PRODUCER_STUB)
            (scratch / "bin").mkdir()
            (scratch / "bin/curl").write_text(CURL_STUB)
            (scratch / "bin/curl").chmod(0o755)
            (scratch / "served").mkdir()
            (scratch / "temp").mkdir()
            for name, data in (files or {}).items():
                (scratch / "served" / name).write_bytes(data)
            calls, urls = scratch / "calls", scratch / "urls"
            result = run_stage_step("Check a fresh rkat consumer", "stage_macos_check", scratch, {
                "RKAT_RELEASE_TAG": tag, "CONNECT_WORK": WORK_ROOT, "RUNNER_TEMP": str(scratch / "temp"),
                "GITHUB_REPOSITORY": "lukacf/meerkat", "STUB_LOG": str(calls), "CURL_LOG": str(urls),
                "CURL_FILES": str(scratch / "served"), "PATH": f"{scratch / 'bin'}:{os.environ['PATH']}"})
            return (result, [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else [],
                    urls.read_text().splitlines() if urls.exists() else [], scratch / "temp")

    def test_the_consumer_check_builds_rkat_from_the_checkout_when_no_release_is_named(self):
        result, calls, urls, _ = self.consumer("")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [["macos", "check-consumer", "--work", WORK_ROOT, "--rkat-from-checkout"]])
        self.assertEqual(urls, [])
        step = step_block("Check a fresh rkat consumer", "stage_macos_check")
        self.assertIn("          RKAT_RELEASE_TAG: ${{ inputs.rkat_release_tag }}\n", step)

    def test_the_consumer_check_takes_cores_published_archive_by_its_release_checksum(self):
        archive = "rkat-0.9.15-aarch64-apple-darwin.tar.gz"
        data = b"published rkat archive"
        digest = hashlib.sha256(data).hexdigest()
        checksums = (f"{'0' * 64}  rkat-0.9.15-x86_64-unknown-linux-gnu.tar.gz\n{digest}  {archive}\n").encode()
        result, calls, urls, temp = self.consumer("v0.9.15", files={archive: data, "checksums.sha256": checksums})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [["macos", "check-consumer", "--work", WORK_ROOT, "--rkat-archive",
                                  f"{temp}/rkat-release/{archive}", "--rkat-archive-sha256", digest]])
        base = "https://github.com/lukacf/meerkat/releases/download/v0.9.15"
        self.assertEqual(sorted(urls), sorted([f"{base}/checksums.sha256", f"{base}/{archive}"]))

    def test_the_consumer_check_refuses_another_tag_form_and_other_bytes(self):
        for tag in ("0.9.15", "v0.9", "v0.9.15-rc1", "v0.9.15;id", "connect-v0.9.15", "v0.9.15 x"):
            with self.subTest(tag=tag):
                result, calls, urls, _ = self.consumer(tag)
                self.assertEqual(result.returncode, 1)
                self.assertEqual((calls, urls), ([], []))
        archive = "rkat-0.9.15-aarch64-apple-darwin.tar.gz"
        good = hashlib.sha256(b"published rkat archive").hexdigest()
        for name, files in (
                ("other bytes", {archive: b"other bytes", "checksums.sha256": f"{good}  {archive}\n".encode()}),
                ("no row", {archive: b"published rkat archive",
                            "checksums.sha256": f"{good}  rkat-0.9.15-x86_64-apple-darwin.tar.gz\n".encode()}),
                ("two rows", {archive: b"published rkat archive",
                              "checksums.sha256": f"{good}  {archive}\n{good}  {archive}\n".encode()}),
                ("no archive", {"checksums.sha256": f"{good}  {archive}\n".encode()}),
                ("no checksum file", {archive: b"published rkat archive"})):
            with self.subTest(name=name):
                result, calls, _, _ = self.consumer("v0.9.15", files=files)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])

    def test_the_stage_record_is_attested_and_uploaded_as_the_producer_wrote_it(self):
        attest = step_block("Attest the stage record", "stage_macos_check") + "\n"
        self.assertIn("        uses: actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8 # v4\n",
                      attest)
        self.assertIn(f"          subject-path: {WORK_ROOT}/artifact/STAGE.json\n", attest)
        upload = step_block("Upload the stage record", "stage_macos_check") + "\n"
        self.assertIn("        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4\n", upload)
        for line in ("name: connect-macos-arm64-stage-${{ github.run_id }}-${{ github.run_attempt }}",
                     f"path: {WORK_ROOT}/artifact", "if-no-files-found: error", "overwrite: false",
                     "include-hidden-files: true", "retention-days: 90"):
            self.assertIn(f"          {line}\n", upload)
        # The provenance bundle is an artifact of its own: the stage record holds exactly the producer's files.
        retain = indented_body("Retain the provenance bundle", "stage_macos_check", "run", 10)
        self.assertNotIn("/artifact", retain)
        self.assertIn('"${RUNNER_TEMP}/stage-provenance/connect-macos-arm64-stage-provenance.jsonl"', retain)
        provenance = step_block("Upload the stage provenance", "stage_macos_check") + "\n"
        for line in ("name: connect-macos-arm64-stage-provenance-${{ github.run_id }}-${{ github.run_attempt }}",
                     "path: ${{ runner.temp }}/stage-provenance/connect-macos-arm64-stage-provenance.jsonl",
                     "if-no-files-found: error", "overwrite: false", "retention-days: 90"):
            self.assertIn(f"          {line}\n", provenance)


STAGE_DIRECTORY = "/private/tmp/cs"
ASSEMBLY_DIRECTORY = "/private/tmp/ca"
ASSEMBLE_STEPS = (
    "Checkout the dispatched source",
    "Check the runner layout",
    "Select Python",
    "Read the stage run and the stated review digest",
    "Download the stage record and its provenance",
    "Verify the stage record's attestation and commit",
    "Accept the reviewed shutdown record",
    "Assemble both products",
    "Upload the assembly",
)
GH_RECORDER = (
    f"#!{sys.executable}\n"
    "import json, os, sys\n"
    "with open(os.environ['GH_LOG'], 'a') as log:\n"
    "    log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
    "sys.exit(int(os.environ.get('GH_STATUS', '0')))\n"
)


class ConnectMacosAssembleTests(unittest.TestCase):
    """Stage two, hosted: a dispatch downloads one stage record of the dispatched commit, checks its
    attestation, reproduces the accepted shutdown review the reader stated by digest, assembles both
    products at the stage's own paths and uploads them as a workflow artifact. Nothing is published."""

    JOB = "assemble_macos"

    def test_the_assembly_is_a_dispatch_mode_that_reuses_two_inputs(self):
        text = WORKFLOW.read_text()
        inputs = text[text.index("    inputs:\n"):text.index("\npermissions:\n")]
        names = re.findall(r"(?m)^      ([a-z_0-9]+):$", inputs)
        # A dispatch takes few inputs: the stage run and the stated digest travel in two existing ones.
        self.assertLessEqual(len(names), 10)
        for name in ("candidate_run_id", "acceptance_sha256"):
            block = inputs[inputs.index(f"      {name}:\n"):]
            self.assertIn("macos-assemble", block.splitlines()[1])
        step = step_block("Read the stage run and the stated review digest", self.JOB)
        self.assertIn("          STAGE_RUN: ${{ inputs.candidate_run_id }}\n", step)
        self.assertIn("          REVIEW_SHA256: ${{ inputs.acceptance_sha256 }}\n", step)

    def test_only_the_assembly_dispatch_runs_the_job(self):
        outcome = results("workflow_dispatch", "macos-assemble")
        self.assertEqual({job for job, result in outcome.items() if result == "success"}, {"resolve", self.JOB})
        for event, mode in (("push", "tag"), ("workflow_dispatch", "candidate"), ("workflow_dispatch", "accept"),
                            ("workflow_dispatch", "publish"), ("workflow_dispatch", "macos-build"),
                            ("workflow_dispatch", "macos-assets"), ("workflow_dispatch", "macos-stage")):
            with self.subTest(mode=mode):
                self.assertEqual(results(event, mode)[self.JOB], "skipped")
        self.assertEqual(job_needs(self.JOB), ["resolve"])
        sha = "4" * 40
        result, _, _, output = run_step("Resolve", job="resolve", env={
            "EVENT": "workflow_dispatch", "REF": "", "MODE": "macos-assemble", "RELEASE_TAG": "", "GITHUB_SHA": sha})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"source_ref={sha}\n", output)

    def test_the_job_reads_artifacts_and_can_write_nothing_but_its_own(self):
        block = job_block(self.JOB)
        self.assertIn("    runs-on: macos-15\n", block)
        self.assertLessEqual(int(re.search(r"(?m)^    timeout-minutes: ([0-9]+)$", block).group(1)), 360)
        self.assertEqual(job_mapping(self.JOB, "permissions"), {"actions": "read", "contents": "read"})
        for word in ("secrets.", "gh release", "gh api", "gh workflow", "git push", "git tag", "contents: write",
                     "id-token", "attestations: write", "--clobber", "API_KEY", "notarytool", "Developer ID",
                     "codesign", "repo-cargo", "cargo "):
            with self.subTest(word=word):
                self.assertNotIn(word, block)
        # The run's own token is used for two reads: another run's artifacts and the attestation check.
        self.assertEqual(sorted(set(re.findall(r"(?m)^\s+gh ([a-z]+ [a-z]+) ", block))),
                         ["attestation verify", "run download"])
        uses = re.findall(r"(?m)^        uses: ([^@\s]+)@", block)
        self.assertEqual(uses, ["actions/checkout", "actions/setup-python", "actions/upload-artifact"])

    def test_the_assembly_runs_at_the_stages_own_paths(self):
        self.assertEqual(job_mapping(self.JOB, "env"), {
            "CONNECT_CHECKOUT": CHECKOUT_ROOT, "CONNECT_WORK": WORK_ROOT, "CONNECT_STAGE": STAGE_DIRECTORY,
            "CONNECT_ASSEMBLY": ASSEMBLY_DIRECTORY, "PYTHONDONTWRITEBYTECODE": "1", "CARGO_TERM_COLOR": "never"})
        for step in ("Checkout the dispatched source", "Check the runner layout", "Select Python"):
            with self.subTest(step=step):
                self.assertEqual(step_block(step, self.JOB).rstrip(), step_block(step, "stage_macos_check").rstrip())
        self.assertEqual(tuple(re.findall(r"(?m)^      - name: (.+)$", job_block(self.JOB))), ASSEMBLE_STEPS)

    def request(self, stage_run, review):
        with TempDir() as scratch:
            output = scratch / "github-output"
            result = run_stage_step("Read the stage run and the stated review digest", self.JOB, scratch, {
                "STAGE_RUN": stage_run, "REVIEW_SHA256": review, "GITHUB_OUTPUT": str(output)})
            return result, output.read_text() if output.exists() else ""

    def test_the_request_names_one_stage_run_attempt_and_one_digest(self):
        digest = "a1" * 32
        for stage_run, expected in (("987654", ("987654", "1")), ("987654-3", ("987654", "3"))):
            with self.subTest(stage_run=stage_run):
                result, output = self.request(stage_run, digest)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output, f"run_id={expected[0]}\nattempt={expected[1]}\nreview_sha256={digest}\n")
        for stage_run in ("", "0", "12a", "987654-0", "987654-2-3", "987654 ", "-2", "987654;id", "main"):
            with self.subTest(stage_run=stage_run):
                result, output = self.request(stage_run, digest)
                self.assertEqual((result.returncode, output), (1, ""))
        for review in ("", digest.upper(), digest[:-1], digest + "0", "sha256:" + digest):
            with self.subTest(review=review):
                result, output = self.request("987654", review)
                self.assertEqual((result.returncode, output), (1, ""))

    def gh_step(self, step, *, stage=None, env=None, status="0"):
        with TempDir() as scratch:
            scratch = scratch.resolve()
            (scratch / "bin").mkdir()
            (scratch / "bin/gh").write_text(GH_RECORDER)
            (scratch / "bin/gh").chmod(0o755)
            (scratch / "stage").mkdir()
            if stage is not None:
                (scratch / "stage/STAGE.json").write_text(json.dumps(stage))
            log = scratch / "gh-log"
            result = run_stage_step(step, self.JOB, scratch, {
                "GH_LOG": str(log), "GH_STATUS": status, "GITHUB_REPOSITORY": "lukacf/meerkat",
                "CONNECT_STAGE": str(scratch / "stage"), "RUNNER_TEMP": str(scratch / "temp"),
                "PATH": f"{scratch / 'bin'}:{os.environ['PATH']}", **(env or {})})
            calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            return result, calls, scratch

    def test_the_stage_record_and_its_provenance_come_from_the_named_run_attempt(self):
        result, calls, scratch = self.gh_step("Download the stage record and its provenance",
                                              env={"RUN_ID": "987654", "ATTEMPT": "3"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [
            ["run", "download", "987654", "--repo", "lukacf/meerkat", "--name",
             "connect-macos-arm64-stage-987654-3", "--dir", str(scratch / "stage")],
            ["run", "download", "987654", "--repo", "lukacf/meerkat", "--name",
             "connect-macos-arm64-stage-provenance-987654-3", "--dir", str(scratch / "temp" / "stage-provenance")],
        ])
        step = step_block("Download the stage record and its provenance", self.JOB)
        self.assertIn("          GH_TOKEN: ${{ github.token }}\n", step)
        self.assertIn("          RUN_ID: ${{ steps.request.outputs.run_id }}\n", step)
        self.assertIn("          ATTEMPT: ${{ steps.request.outputs.attempt }}\n", step)

    def test_a_stage_of_another_commit_or_without_this_workflows_attestation_is_refused(self):
        sha = "5" * 40
        step = "Verify the stage record's attestation and commit"
        result, calls, scratch = self.gh_step(step, stage={"revision": sha}, env={"GITHUB_SHA": sha})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [[
            "attestation", "verify", str(scratch / "stage" / "STAGE.json"), "--repo", "lukacf/meerkat",
            "--bundle", str(scratch / "temp" / "stage-provenance" / "connect-macos-arm64-stage-provenance.jsonl"),
            "--signer-workflow", "lukacf/meerkat/.github/workflows/connect-release.yml",
            "--source-digest", sha]])
        result, calls, _ = self.gh_step(step, stage={"revision": "6" * 40}, env={"GITHUB_SHA": sha})
        self.assertEqual(result.returncode, 1)
        self.assertIn("not the dispatched commit", result.stderr)
        result, calls, _ = self.gh_step(step, stage={"revision": sha}, env={"GITHUB_SHA": sha}, status="1")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(calls), 1)
        for stage in ({}, {"revision": 7}, None):
            with self.subTest(stage=stage):
                result, _, _ = self.gh_step(step, stage=stage, env={"GITHUB_SHA": sha})
                self.assertNotEqual(result.returncode, 0)

    def test_the_stated_review_is_reproduced_and_both_products_are_assembled(self):
        review = '"${RUNNER_TEMP}/native-shutdown-review.accepted.json"'
        self.assertEqual(
            one_line("Accept the reviewed shutdown record", self.JOB),
            f'set -euo pipefail {PRODUCER} accept-review --stage "${{CONNECT_STAGE}}" --output {review} '
            '--review-sha256 "${REVIEW_SHA256}"')
        self.assertEqual(
            one_line("Assemble both products", self.JOB),
            f'set -euo pipefail {PRODUCER} assemble --work "${{CONNECT_WORK}}" --stage "${{CONNECT_STAGE}}" '
            f'--review {review} --review-sha256 "${{REVIEW_SHA256}}" --output "${{CONNECT_ASSEMBLY}}"')
        for step in ("Accept the reviewed shutdown record", "Assemble both products"):
            self.assertIn("          REVIEW_SHA256: ${{ steps.request.outputs.review_sha256 }}\n",
                          step_block(step, self.JOB))
        called = re.findall(r"connect-candidate\.py macos ([a-z-]+)", job_block(self.JOB))
        self.assertEqual(called, ["accept-review", "assemble"])

    def test_one_artifact_holds_both_archives_their_checksums_and_the_assembly_record(self):
        upload = step_block("Upload the assembly", self.JOB) + "\n"
        self.assertIn("        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4\n", upload)
        for line in ("name: connect-macos-arm64-assembly-${{ github.run_id }}-${{ github.run_attempt }}",
                     "if-no-files-found: error", "overwrite: false", "retention-days: 90"):
            self.assertIn(f"          {line}\n", upload)
        paths = re.findall(r"(?m)^            (/\S+)$", upload)
        self.assertEqual(paths, [
            f"{ASSEMBLY_DIRECTORY}/ASSEMBLY.json",
            f"{ASSEMBLY_DIRECTORY}/cli/meerkat-connect-*-macos-arm64.tar.gz",
            f"{ASSEMBLY_DIRECTORY}/cli/meerkat-connect-*-macos-arm64.tar.gz.sha256",
            f"{ASSEMBLY_DIRECTORY}/gateway/meerkat-connect-gateway-*-macos-arm64.tar.gz",
            f"{ASSEMBLY_DIRECTORY}/gateway/meerkat-connect-gateway-*-macos-arm64.tar.gz.sha256",
        ])


class ConnectMacosProducerContractTests(unittest.TestCase):
    """What the three jobs of the complete macOS installation assume of the producer they call
    (connect/scripts/connect-candidate.py), read from its source."""

    SOURCE = ROOT / "connect/scripts/connect-candidate.py"
    JOBS = (*STAGE_JOBS, "assemble_macos")

    def test_the_stage_jobs_call_every_stage_one_step_of_the_producer_in_its_order(self):
        source = self.SOURCE.read_text()
        declared = re.search(r"(?m)^MACOS_STEPS = \(([^)]*)\)", source).group(1)
        steps = re.findall(r'"([a-z-]+)"', declared)
        called = re.findall(r"connect-candidate\.py macos ([a-z-]+)", "".join(job_block(job) for job in STAGE_JOBS))
        in_order = [step for index, step in enumerate(called) if index == 0 or called[index - 1] != step]
        self.assertEqual(in_order, steps)
        for step in ("accept-review", "assemble", "release-check"):
            self.assertIn(f'"{step}"', source)

    def test_the_cargo_output_the_stage_frees_is_the_producers_lane(self):
        lane = re.search(r'(?m)^MACOS_LANE = "([^"]+)"$', self.SOURCE.read_text()).group(1)
        self.assertIn(f"          RUST_LANE_ID: {lane}\n", step_block("Free the Cargo output", "stage_macos_build") + "\n")

    def test_a_failed_step_fails_its_job(self):
        # A failed check (the official Playwright install is one) must stop the job before the stage
        # record is written: no step is conditional and none tolerates a failure. The producer's own
        # tests hold that a failed official install fails its step and that a check that did not run is
        # stated and never assembled (connect/scripts/test_connect_candidate_cli.py:
        # test_a_failed_official_install_fails_the_stage,
        # test_an_official_install_that_could_not_run_is_stated_and_never_assembles).
        for job in self.JOBS:
            block = job_block(job)
            with self.subTest(job=job):
                self.assertNotIn("continue-on-error", block)
                self.assertEqual(re.findall(r"(?m)^        if:", block), [])
                for step in re.findall(r"(?m)^      - name: (.+)$", block):
                    body = step_block(step, job)
                    if "        run: |\n" in body + "\n":
                        self.assertIn("          set -euo pipefail\n", body + "\n", step)
        producer_tests = (ROOT / "connect/scripts/test_connect_candidate_cli.py").read_text()
        for name in ("test_a_failed_official_install_fails_the_stage",
                     "test_an_official_install_that_could_not_run_is_stated_and_never_assembles"):
            self.assertIn(f"def {name}(", producer_tests)


if __name__ == "__main__":
    unittest.main()
