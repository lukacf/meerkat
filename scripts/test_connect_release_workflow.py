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

import hashlib
import json
import os
import re
import shlex
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


class ConnectUploadReceiptTests(unittest.TestCase):
    """No path of this workflow adds a file to a release without the upload receipt check
    (connect/scripts/release-upload-receipt-check.py, run by accept-publication and publish-accepted)."""

    # Every dispatch input of the workflow. None of them is a pin of the upload receipt check, and
    # none is a full scan report: accept takes the upload receipt.
    INPUTS = ("mode", "target", "source_sha", "inputs_sha256", "candidate_run_id", "publication_request",
              "acceptance_run_id", "acceptance_sha256", "upload_receipt")
    # Words under which a pin could come back as an input, a variable or an option.
    PIN_WORDS = ("name_list", "name-list", "rules", "detector", "non_published", "non-published", "pins",
                 "coverage", "exceptions", "max-age", "max_age")
    PINS_FILE = ROOT / "connect/release/upload-receipt-pins.json"

    def code(self) -> str:
        """The workflow without its comment lines."""
        return "\n".join(line for line in WORKFLOW.read_text().splitlines() if not line.lstrip().startswith("#"))

    def inputs(self) -> list[str]:
        return re.findall(r"^      ([a-z_0-9]+):$", self.code().split("permissions:")[0], flags=re.M)

    def test_no_hosted_step_scans_or_holds_a_private_list(self):
        # A party that cannot hold the private inputs never mints a receipt: this workflow only consumes
        # the upload receipt. No step runs a scan or writes a receipt, and no input, secret or file of it
        # carries a name list, rules or the list of left-out members, nor a digest of one.
        code = self.code()
        for forbidden in ("privacy-scan", "credential-scan", "assemble-cli", "release-payload-privacy-scan",
                          "release-payload-credential-scan", "privacy-rules-from-names", "release-upload-receipt-write",
                          "--rules", "--exemptions",
                          "--non-published-list", "--name-list", "--accepted-findings", "secrets."):
            self.assertNotIn(forbidden, code, forbidden)
        private = [name for name in self.inputs() if ("name" in name or "non_published" in name or "rules" in name)]
        self.assertEqual(private, [])

    def test_no_pin_of_the_receipt_check_is_an_input_a_secret_or_a_variable(self):
        # What the reports are held to comes from the commit of the checkout, never from the run that
        # supplies the reports: the workflow has exactly these inputs, uses no secret and no repository
        # or environment variable, and no line of it names a pin, the pins file or the age bound.
        self.assertEqual(tuple(self.inputs()), self.INPUTS)
        code = self.code()
        for forbidden in ("secrets.", "vars.", "${{ env."):
            self.assertNotIn(forbidden, code, forbidden)
        for word in self.PIN_WORDS:
            self.assertNotIn(word, code.lower(), word)
        expressions = set(re.findall(r"\$\{\{\s*inputs\.([a-z_0-9]+)\s*\}\}", code))
        self.assertEqual(expressions - set(self.INPUTS), set())
        # The pins are a committed file of the checkout that accept and publish run from: both jobs
        # check the repository out (no other ref, no path filter) before they run its candidate tool.
        self.assertTrue(self.PINS_FILE.is_file())
        for job, name in (("accept", "accept-publication"), ("publish", "publish-accepted")):
            block = job_block(job)
            checkout = step_block("Checkout", job)
            self.assertIn("uses: actions/checkout@", checkout)
            for forbidden in ("ref:", "repository:", "sparse-checkout", "path:"):
                self.assertNotIn(forbidden, checkout, (job, forbidden))
            self.assertLess(block.index("- name: Checkout"), block.index("connect-candidate.py " + name))
            self.assertIn("working-directory: connect", block)
            self.assertEqual(block.count("python3 scripts/connect-candidate.py " + name), 1)

    def command(self, step: str, job: str, name: str) -> str:
        """The one invocation of a connect-candidate.py command in a step, as one line."""
        body = indented_body(step, job, "run", 10).replace("\\\n", " ")
        lines = [line for line in body.splitlines() if "connect-candidate.py " + name in line]
        self.assertEqual(len(lines), 1, body)
        return " ".join(lines[0].split())

    def options(self, command: str) -> list[str]:
        return [word for word in command.split() if word.startswith("--")]

    def test_publish_gives_the_receipt_check_no_pin(self):
        step = "Add exactly the accepted assets (never replacing one)"
        command = self.command(step, "publish", "publish-accepted")
        self.assertEqual(self.options(command), ["--accepted", "--acceptance-sha256", "--output"])
        variables = re.findall(r"^          ([A-Z_0-9]+): ", step_block(step, "publish"), flags=re.M)
        self.assertEqual(variables, ["GH_TOKEN", "ACCEPTANCE_SHA256"])

    def test_accept_gives_the_receipt_check_the_upload_receipt_and_no_pin(self):
        step = "Accept the candidate"
        command = self.command(step, "accept", "accept-publication")
        self.assertEqual(self.options(command), ["--request", "--candidate", "--meta-source", "--upload-receipt", "--output"])
        variables = re.findall(r"^          ([A-Z_0-9]+): ", step_block(step, "accept"), flags=re.M)
        self.assertEqual(variables, ["GH_TOKEN", "REQUEST", "UPLOAD_RECEIPT"])
        body = indented_body(step, "accept", "run", 10)
        self.assertIn('--upload-receipt "${RUNNER_TEMP}/upload-receipt.json"', command)
        self.assertIn("printf '%s' \"${UPLOAD_RECEIPT}\" > \"${RUNNER_TEMP}/upload-receipt.json\"", body)
        self.assertIn("          UPLOAD_RECEIPT: ${{ inputs.upload_receipt }}", step_block(step, "accept"))
        # The acceptance record is uploaded only after that command, in the same job.
        block = job_block("accept")
        self.assertLess(block.index("accept-publication"), block.index("- name: Upload the acceptance record"))

    def test_no_full_scan_report_is_an_input_or_an_uploaded_artifact(self):
        # A full scan report holds category names and counts. It stays where the scans ran: this
        # workflow takes the upload receipt, and no line of it names a report.
        self.assertEqual([name for name in self.inputs() if "report" in name], [])
        code = self.code()
        for word in ("report", "REPORT", "scan-reports"):
            self.assertNotIn(word, code, word)
        # What the workflow uploads: the candidate closure (twice, before and after attestation) and
        # the acceptance directory. Nothing else, and no file from a runner's temporary directory
        # other than that directory, which the candidate tool fills with the receipt alone.
        uploads = []
        for job in JOBS:
            lines = job_block(job).splitlines()
            for at, line in enumerate(lines):
                if "uses: actions/upload-artifact@" in line:
                    paths = [text.split("path:", 1)[1].strip() for text in lines[at:at + 8] if text.strip().startswith("path:")]
                    uploads.append((job, paths))
        self.assertEqual(uploads, [("build", ["connect/dist/*"]), ("attest", ["connect/dist/*"]),
                                   ("accept", ["${{ runner.temp }}/acceptance"])])
        accept = indented_body("Accept the candidate", "accept", "run", 10)
        written = re.findall(r'> "\$\{RUNNER_TEMP\}/([^"]+)"', accept)
        self.assertEqual(written, ["request.json", "upload-receipt.json"])
        self.assertIn('--output "${RUNNER_TEMP}/acceptance"', accept)
        # The candidate tool has no option that takes a full report into an acceptance record.
        candidate = (ROOT / "connect/scripts/connect-candidate.py").read_text()
        for option in ('"privacy-report"', '"credential-report"', "scan-reports"):
            self.assertNotIn(option, candidate, option)

    def test_release_assets_are_written_by_publish_accepted_alone(self):
        text = WORKFLOW.read_text()
        code = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))
        for forbidden in ("gh release upload", "upload-release-asset", "softprops/action-gh-release", "uploads.github.com"):
            self.assertNotIn(forbidden, code)
        # The one release command creates the release object and names no file.
        creates = [line for line in code.splitlines() if "gh release create" in line]
        self.assertEqual(len(creates), 1)
        create = indented_body("Create the release object for the tag", "create_release", "run", 10)
        words = shlex.split(create[create.index("gh release create"):].replace("\\\n", " "))[3:]
        positional, skip = [], False
        for word in words:
            if skip:
                skip = False
            elif word in ("--repo", "--title", "--notes"):
                skip = True
            elif not word.startswith("--"):
                positional.append(word)
        self.assertEqual(positional, ["${TAG}"])    # the tag alone: no file is given to the release
        # Only publish-accepted writes with the token, and only the publish job may write contents.
        writers = [job for job in JOBS if "contents: write" in job_block(job)]
        self.assertEqual(writers, ["create_release", "publish"])
        self.assertEqual(job_block("publish").count("connect-candidate.py publish-accepted"), 1)

    def test_the_rule_for_other_upload_paths_and_the_unbound_artifacts_is_stated(self):
        create = step_block("Create the release object for the tag", "create_release")
        # A lane's files reach the release through the upload tool, which runs the receipt check
        # itself; a script that only calls the check first and then uploads is not the rule any more.
        self.assertIn("connect/scripts/release-upload-attach.py", create)
        self.assertIn("is not to be run", create)
        self.assertNotIn("first statement", create)
        self.assertNotIn("release-upload-receipt-check.py", create)
        self.assertNotIn("the release owner uploads", create)
        # The two artifacts no report can exist for say so; the acceptance record's upload is bound.
        lines = WORKFLOW.read_text().splitlines()
        for step, word in (("Upload exact candidate closure", "Not bound to a scan report"),
                           ("Upload attested candidate without publication", "Not bound to a scan report"),
                           ("Upload the acceptance record", "Bound: accept-publication")):
            at = lines.index(f"      - name: {step}")
            comment = []
            while lines[at - 1].lstrip().startswith("#"):
                at -= 1
                comment.insert(0, lines[at])
            self.assertIn(word, "\n".join(comment), step)


# Every form in which a script or a workflow can write a file to a GitHub release. The two `gh`
# commands are matched as a command line and as a list of arguments.
ATTACH_FORMS = {
    "gh release upload": re.compile(r"""gh["',\s]+release["',\s]+upload"""),
    "gh release create": re.compile(r"""gh["',\s]+release["',\s]+create"""),
    "the upload host": re.compile(r"uploads\.github\.com"),
    "an upload action": re.compile(r"action-gh-release|upload-release-asset|upload-release-action|ncipollo/release-action"),
    "an upload address of the API": re.compile(r"upload_url|upload_asset"),
    "the candidate tool's upload function": re.compile(r"release_asset_upload"),
}
ATTACH_ROOTS = ("connect", "scripts", ".github")
NOT_SOURCE = {".git", "node_modules", "target", "dist", "build", "__pycache__", ".venv", ".pytest_cache"}
CANDIDATE_TOOL = "connect/scripts/connect-candidate.py"
ATTACH_TOOL = "connect/scripts/release-upload-attach.py"
# The files below connect/, scripts/ and .github/ that name such a form, and why each is there.
ATTACH_SITES = {
    # The two sites that attach a file to a Connect release.
    CANDIDATE_TOOL: {"the upload host", "the candidate tool's upload function"},      # the one upload request; publish-accepted
    ATTACH_TOOL: {"the candidate tool's upload function"},                             # the upload tool for lane files
    # The Connect release object: created for the tag, with no file.
    ".github/workflows/connect-release.yml": {"gh release create"},
    # Rows that name the forms and run none (fakes and patterns).
    "connect/scripts/test_connect_candidate_publication.py": {"the upload host"},
    "connect/scripts/test_release_upload_attach.py": {"the upload host"},
    "scripts/test_connect_release_workflow.py": set(ATTACH_FORMS),
    # Core's release workflow, bound to its own tag, and the checker of that workflow with its fixtures.
    ".github/workflows/release.yml": {"gh release upload", "an upload action"},
    "scripts/check_release_workflow_contract.py": {"gh release upload", "an upload action"},
    "scripts/test-release-doctor-workflow-contract.sh": {"gh release upload"},
}


class ConnectReleaseAttachPathTests(unittest.TestCase):
    """A file reaches a Connect release in two ways, and both are held to a checked receipt:
    publish-accepted of the candidate tool (its three candidate files) and the upload tool (the
    files of a lane's staging directory). Nothing else below connect/, scripts/ and .github/ can."""
    maxDiff = None

    def sites(self) -> dict[str, set[str]]:
        found: dict[str, set[str]] = {}
        for root in ATTACH_ROOTS:
            for directory, names, files in os.walk(ROOT / root):
                names[:] = sorted(name for name in names if name not in NOT_SOURCE)
                for name in sorted(files):
                    path = Path(directory) / name
                    if path.is_symlink() or not path.is_file():
                        continue
                    try:
                        text = path.read_text()
                    except (UnicodeDecodeError, OSError):
                        continue
                    forms = {form for form, pattern in ATTACH_FORMS.items() if pattern.search(text)}
                    if forms:
                        found[path.relative_to(ROOT).as_posix()] = forms
        return found

    def test_release_assets_are_attached_by_publish_accepted_and_the_upload_tool_alone(self):
        self.assertEqual(self.sites(), ATTACH_SITES)
        # One request writes a file to a Connect release: the candidate tool's upload function.
        candidate = (ROOT / CANDIDATE_TOOL).read_text()
        self.assertEqual(candidate.count("uploads.github.com"), 1)
        function = candidate[candidate.index("def release_asset_upload("):]
        function = function[:function.index("\n\n\n")]
        self.assertIn("uploads.github.com", function)
        self.assertIn("/releases/{release_id}/assets?name={name}", function)
        # It is the only place that gives the API runner a file, and it has two callers: one in
        # publish_accepted, after the upload receipt check, and one in the upload tool's adapter.
        self.assertEqual(len(re.findall(r"publication_api\([^()]*upload=upload\)", candidate, flags=re.S)), 1)
        self.assertEqual(candidate.count("upload=upload"), 1)
        self.assertEqual(candidate.count("upload=upload"), function.count("upload=upload"))
        calls = [at.start() for at in re.finditer(r"(?<!def )release_asset_upload\(", candidate)]
        self.assertEqual(len(calls), 1)
        publish = candidate.index("def publish_accepted(")
        self.assertLess(publish, calls[0])
        self.assertLess(candidate.index("upload_receipt_check(", publish), calls[0])
        self.assertNotIn("\ndef ", candidate[publish + 1:calls[0]])
        tool = (ROOT / ATTACH_TOOL).read_text()
        self.assertEqual(len(re.findall(r"release_asset_upload\(", tool)), 1)
        self.assertIn("return self.candidate.release_asset_upload(release_id, name, stream)", tool)
        self.assertNotIn("upload=", tool)
        # No other tool below connect/ starts `gh` with a method that writes.
        for path in sorted((ROOT / "connect/scripts").glob("*.py")):
            if path.name.startswith("test_") or path.name == Path(CANDIDATE_TOOL).name:
                continue
            for word in ('"--method"', "'--method'", '"-X"', "'-X'"):
                self.assertNotIn(word, path.read_text(), (path.name, word))

    def test_the_other_release_workflow_of_the_tree_writes_to_its_own_tag(self):
        # Core's release workflow holds the tree's other upload calls. Its tag pattern does not match a
        # Connect tag, and a publishing run of it is bound to the tag of the workspace version.
        text = (ROOT / ".github/workflows/release.yml").read_text()
        head = text[:text.index("  workflow_dispatch:")]
        self.assertEqual(re.findall(r'^      - "([^"]+)"$', head, flags=re.M), ["v*"])
        self.assertIn('expected="v${version}"', text)
        self.assertNotIn("connect-v", text)
        connect = WORKFLOW.read_text()
        self.assertEqual(re.findall(r'^      - "([^"]+)"$', connect[:connect.index("  workflow_dispatch:")], flags=re.M), ["connect-v*"])


GATE = "Require a commit of the default branch"
GIT_TEST = ("-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
            "-c", "core.hooksPath=/dev/null", "-c", "init.defaultBranch=main", "-c", "protocol.file.allow=always")


class ConnectDefaultBranchGateTests(unittest.TestCase):
    """The pins of the upload receipt check are a commit's, and they count only when that commit is a
    reviewed one. accept and publish refuse, before the check, unless the commit they checked out is on
    the default branch. Candidates are still built on any branch."""

    def git(self, directory, *arguments, check=True):
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        done = subprocess.run(["git", "-C", str(directory), *GIT_TEST, *arguments], capture_output=True, text=True,
                              env=environment)
        if check:
            self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.strip()

    def commit(self, directory, text):
        (directory / "file.txt").write_text(text)
        self.git(directory, "add", "-A")
        self.git(directory, "commit", "-q", "-m", text)
        return self.git(directory, "rev-parse", "HEAD")

    def origin(self, root):
        """A made-up repository: main with three commits, a branch that left main at its first commit and
        was never merged, and a branch that names main's first commit."""
        seed = root / "seed"
        seed.mkdir()
        self.git(seed, "init", "-q")
        first = self.commit(seed, "first")
        self.git(seed, "branch", "old", first)
        self.git(seed, "checkout", "-q", "-b", "side")
        side = self.commit(seed, "side work")
        self.git(seed, "checkout", "-q", "main")
        second = self.commit(seed, "second")
        tip = self.commit(seed, "third")
        bare = root / "origin.git"
        self.git(root, "clone", "-q", "--bare", str(seed), str(bare))
        return bare, {"first": first, "side": side, "second": second, "tip": tip}

    def clone(self, root, bare, name, *options):
        self.git(root, "clone", "-q", *options, "file://" + str(bare), str(root / name))
        return root / name

    def gate(self, job, checkout, branch="main"):
        script = indented_body(GATE, job, "run", 10)
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        return subprocess.run(["bash", "-c", script], cwd=checkout, capture_output=True, text=True, env={
            **environment, "DEFAULT_BRANCH": branch, "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "protocol.file.allow",
            "GIT_CONFIG_VALUE_0": "always"})

    def test_accept_and_publish_run_the_gate_before_the_receipt_check(self):
        for job, name in (("accept", "accept-publication"), ("publish", "publish-accepted")):
            block = job_block(job)
            steps = re.findall(r"^      - name: (.+)$", block, flags=re.M)
            # The gate is the step right after the checkout, so nothing else of the job runs without it.
            self.assertEqual(steps[:2], ["Checkout", GATE], job)
            gate = step_block(GATE, job)
            self.assertIn("          DEFAULT_BRANCH: ${{ github.event.repository.default_branch }}", gate)
            self.assertLess(block.index("- name: " + GATE), block.index("connect-candidate.py " + name))
            # Nothing lets a later step run after the gate refused, and the gate is not conditional.
            for forbidden in ("continue-on-error", "always()", "failure()", "cancelled()"):
                self.assertNotIn(forbidden, block, (job, forbidden))
            self.assertNotIn("        if:", gate)
            self.assertEqual(block.count("- name: " + GATE), 1)
            body = indented_body(GATE, job, "run", 10)
            self.assertIn("set -euo pipefail", body)
            self.assertIn("git -c core.commitGraph=false merge-base --is-ancestor", body)
            self.assertEqual(body.count("merge-base"), 1)
            self.assertNotIn("|| true", body)
        # The same gate in both jobs, and in no job that builds or attests a candidate.
        self.assertEqual(indented_body(GATE, "accept", "run", 10), indented_body(GATE, "publish", "run", 10))
        for job in JOBS:
            if job not in ("accept", "publish"):
                self.assertNotIn(GATE, job_block(job), job)

    def test_a_commit_of_the_default_branch_passes_and_any_other_commit_refuses(self):
        for job in ("accept", "publish"):
            with self.subTest(job=job), TempDir() as root:
                bare, commits = self.origin(root)
                # The tip of the default branch, and an earlier commit of it.
                at_tip = self.clone(root, bare, "at-tip")
                done = self.gate(job, at_tip)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertIn(commits["tip"], done.stdout)
                earlier = self.clone(root, bare, "earlier")
                self.git(earlier, "checkout", "-q", "--detach", commits["first"])
                self.assertEqual(self.gate(job, earlier).returncode, 0)
                # A shallow checkout of an earlier commit: the history is fetched, not assumed.
                shallow = self.clone(root, bare, "shallow", "--depth", "1", "--branch", "old")
                self.assertEqual(self.git(shallow, "rev-parse", "--is-shallow-repository"), "true")
                done = self.gate(job, shallow)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertEqual(self.git(shallow, "rev-parse", "--is-shallow-repository"), "false")
                # A branch that was never merged, in a full and in a shallow checkout.
                for name, options in (("side", ("--branch", "side")), ("side-shallow", ("--depth", "1", "--branch", "side"))):
                    side = self.clone(root, bare, name, *options)
                    self.assertEqual(self.git(side, "rev-parse", "HEAD"), commits["side"])
                    done = self.gate(job, side)
                    self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
                    self.assertIn("is not on the default branch", done.stderr)
                # A commit that only this checkout has, on top of the default branch.
                local = self.clone(root, bare, "local")
                self.commit(local, "not pushed")
                self.assertEqual(self.gate(job, local).returncode, 1)

    def test_the_gate_refuses_when_it_cannot_fetch_or_does_not_know_the_branch(self):
        for job in ("accept", "publish"):
            with self.subTest(job=job), TempDir() as root:
                bare, _ = self.origin(root)
                checkout = self.clone(root, bare, "checkout")
                self.assertEqual(self.gate(job, checkout).returncode, 0)
                # The default branch is not named, is not a branch name, or does not exist.
                for branch in ("", "main other", "--upload-pack=x", "no-such-branch"):
                    done = self.gate(job, checkout, branch)
                    self.assertEqual(done.returncode, 1, (branch, done.stdout, done.stderr))
                # The remote cannot be reached: an earlier fetch of the branch is not trusted instead.
                self.git(checkout, "remote", "set-url", "origin", "file://" + str(root / "gone.git"))
                done = self.gate(job, checkout)
                self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
                self.assertIn("refusing", done.stderr)
                # Not a checkout at all.
                plain = root / "plain"
                plain.mkdir()
                self.assertNotEqual(self.gate(job, plain).returncode, 0)

    def test_a_grafts_file_states_no_ancestry_for_the_gate(self):
        for job in ("accept", "publish"):
            with self.subTest(job=job), TempDir() as root:
                bare, commits = self.origin(root)
                # A grafts line that names the unmerged commit as the parent of the default branch's tip:
                # read with it, the unmerged commit is an ancestor of the tip.
                side = self.clone(root, bare, "side", "--branch", "side")
                self.git(side, "fetch", "-q", "origin", "main")
                (side / ".git" / "info").mkdir(exist_ok=True)
                (side / ".git" / "info" / "grafts").write_text(f"{commits['tip']} {commits['side']}\n")
                self.git(side, "merge-base", "--is-ancestor", commits["side"], commits["tip"])    # fails unless it is one
                done = self.gate(job, side)
                self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
                self.assertIn("is not on the default branch", done.stderr)

    def test_a_commit_graph_file_states_no_ancestry_for_the_gate(self):
        for job in ("accept", "publish"):
            with self.subTest(job=job), TempDir() as root:
                bare, commits = self.origin(root)
                # git takes the parents of every commit below the ones it is asked about from a
                # commit-graph file when the checkout has one. Here the file's row for the parent of
                # the default branch's tip names the unmerged commit as its parent: read with it, the
                # unmerged commit is an ancestor of the tip.
                side = self.clone(root, bare, "side", "--branch", "side")
                self.git(side, "commit-graph", "write", "--reachable")
                rewrite_commit_graph_parent(side, commits["second"], commits["side"])
                self.git(side, "merge-base", "--is-ancestor", commits["side"], commits["tip"])    # fails unless it is one
                done = self.gate(job, side)
                self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
                self.assertIn("is not on the default branch", done.stderr)


def rewrite_commit_graph_parent(checkout, child, parent):
    """In the commit-graph file of a checkout, make parent the first parent of child and set the file's
    trailing checksum again. The commits themselves stay as they are."""
    path = checkout / ".git" / "objects" / "info" / "commit-graph"
    data = bytearray(path.read_bytes())
    assert data[:4] == b"CGPH", "not a commit-graph file"
    size = 20 if data[5] == 1 else 32                        # the length of a commit id in this file
    chunks = {}
    for index in range(data[6] + 1):                          # the table of chunks: a name and an offset each
        place = 8 + 12 * index
        chunks[bytes(data[place:place + 4])] = int.from_bytes(data[place + 4:place + 12], "big")
    count = int.from_bytes(data[chunks[b"OIDF"] + 255 * 4:chunks[b"OIDF"] + 256 * 4], "big")
    known = [bytes(data[chunks[b"OIDL"] + index * size:chunks[b"OIDL"] + (index + 1) * size]).hex() for index in range(count)]
    row = chunks[b"CDAT"] + known.index(child) * (size + 16)  # a commit's row: its tree, then its first parent
    data[row + size:row + size + 4] = known.index(parent).to_bytes(4, "big")
    data[-size:] = (hashlib.sha1 if size == 20 else hashlib.sha256)(bytes(data[:-size])).digest()
    rewritten = path.with_name("commit-graph.rewritten")
    rewritten.write_bytes(bytes(data))
    os.replace(rewritten, path)


if __name__ == "__main__":
    unittest.main()
