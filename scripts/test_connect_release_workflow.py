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
        "recover_macos", "build", "attest", "accept", "publish")


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


if __name__ == "__main__":
    unittest.main()
