#!/usr/bin/env python3
"""Offline contracts for the Forge release workflow and scripts/forge-release.py.

The workflow is read as text and its job conditions are evaluated, as
test_release_workflow.py does for release.yml. The script runs against git
fixtures in the checkout and in-process doubles for GitHub; nothing here
reaches the network.
"""

import base64
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import urllib.error
import uuid
import zipfile


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/forge-release.yml"
FORGE_CI = ROOT / ".github/workflows/forge.yml"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


forge = load("forge_release", ROOT / "scripts/forge-release.py")
# The Guide's one exact selected release; the module has no package imports.
guide_sdk = load("forge_guide_sdk", ROOT / "installer/launchkit/guide_sdk.py")
release_tests = load("release_workflow_tests", ROOT / "scripts/test_release_workflow.py")
REPO = "lukacf/meerkat-meta"
SHA = "a" * 40
OTHER = "b" * 40
VERSION = "0.2.0"
TAG = "forge-v0.2.0"
RUN, ATTEMPT, CI_RUN, FORGE_RUN = 501, 1, 77, 610
JOBS = ("resolve", "build_package", "require_ci_green", "collect", "publish")
# forge.yml's pull_request paths that a release does not bind: the repository's
# root README and license files. No Forge test or package build reads them; the
# installer ships and tests its own copies under installer/.
FORGE_PATHS_NOT_BOUND = ("README.md", "LICENSE-MIT", "LICENSE-APACHE")


def text(path=WORKFLOW):
    return path.read_text()


def job_block(name, path=WORKFLOW):
    lines = text(path).splitlines()
    start = lines.index(f"  {name}:")
    for offset, line in enumerate(lines[start + 1:], start=start + 1):
        if re.fullmatch(r"  [A-Za-z_][A-Za-z0-9_-]*:", line):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


def job_condition(name):
    block = job_block(name).splitlines()
    matches = [i for i, line in enumerate(block) if line.startswith("    if:")]
    if not matches:
        return "true"
    body = []
    for line in block[matches[0] + 1:]:
        if not line.startswith("      "):
            break
        body.append(line.strip())
    return " ".join(body)


def job_needs(name):
    lines = [line for line in job_block(name).splitlines() if line.startswith("    needs:")]
    return [] if not lines else [item.strip() for item in lines[0].split("[", 1)[1].rstrip("]").split(",")]


def run_blocks(block):
    """Every `run:` script of a block, continuation lines joined."""
    lines, scripts = block.splitlines(), []
    for index, line in enumerate(lines):
        match = re.match(r"(\s*)(?:- )?run: (.*)$", line)
        if not match:
            continue
        if match.group(2).strip() != "|":
            scripts.append(match.group(2))
            continue
        indent, body = None, []
        for following in lines[index + 1:]:
            if following.strip() and indent is None:
                indent = len(following) - len(following.lstrip())
            if following.strip() and len(following) - len(following.lstrip()) < (indent or 0):
                break
            body.append(following.strip())
        scripts.append("\n".join(body))
    return scripts


def context(mode="candidate", publishable="true", **results):
    value = {"needs_resolve_outputs_mode": mode, "needs_resolve_outputs_publishable": publishable}
    for job in JOBS:
        value[f"needs_{job}_result"] = results.get(job, "success")
    return value


def outcomes(mode="candidate", publishable="true", failures=None, run_cancelled=False):
    """Walk the job graph: a job runs when its condition holds; it then
    succeeds unless listed in failures."""
    current, results = context(mode, publishable), {}
    for job in JOBS:
        runs = release_tests.evaluate_condition(job_condition(job), run_cancelled=run_cancelled, **current)
        result = (failures or {}).get(job, "success") if runs else "skipped"
        results[job] = result
        current[f"needs_{job}_result"] = result
    return results


@contextlib.contextmanager
def fixture_directory():
    directory = ROOT / f".forge-release-fixture-{uuid.uuid4().hex}"
    directory.mkdir()
    try:
        yield directory
    finally:
        shutil.rmtree(directory)


@contextlib.contextmanager
def working_directory(path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def gateway_archive(member=b"guide gateway bytes", name="rpc_gateway"):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        info = tarfile.TarInfo(name)
        info.size = len(member)
        archive.addfile(info, io.BytesIO(member))
    return buffer.getvalue()


ARCHIVE = gateway_archive()
GATEWAY = b"guide gateway bytes"
PRODUCT = {
    "product/assembly.json": b'{"assembly": true}',
    "product/build-record.json": b'{"build": true}',
    "product/meerkat-connect-gateway.tar.gz": b"connect product bytes",
    "product/meta.whl": b"meta wheel",
    "product/connect.whl": b"connect wheel",
    "product/requirements.txt": b"dependency==1\n",
    "product/dependency.whl": b"dependency wheel",
    "product/catalog.json": b'{"models": []}',
    "product/source-manifest.json": b'{"source": true}',
}
URL = "https://github.com/lukacf/meerkat-connect/releases/download/v0.9.0/"
GATEWAY_URL = "https://github.com/lukacf/meerkat-meta/releases/download/v0.9.0/meta-rpc-gateway.tar.gz"


def pin(data, **source):
    return {**source, "size": len(data), "sha256": sha256(data)}


def artifact(name):
    data = PRODUCT[name]
    return {"path": "/producer/" + name, "size": len(data), "sha256": sha256(data)}


def target_inputs(target="linux-x86_64"):
    files = {name: pin(data, url=URL + name.rsplit("/", 1)[1]) for name, data in PRODUCT.items()}
    files["product/assembly.json"] = pin(PRODUCT["product/assembly.json"],
                                         path="installer/release/inputs/linux-x86_64/assembly.json")
    files["guide/rpc_gateway"] = {"url": GATEWAY_URL, "archive_size": len(ARCHIVE),
                                  "archive_sha256": sha256(ARCHIVE), "member": "rpc_gateway",
                                  **pin(GATEWAY)}
    selection = {"format": forge.PRODUCT_SELECTION_FORMAT, "artifact_root": "/producer/product",
                 "target": target, "meta": {}, "toolkit": {},
                 "assembly": artifact("product/assembly.json"),
                 "meta_wheel": artifact("product/meta.whl"),
                 "toolkit_wheel": artifact("product/connect.whl"),
                 "dependency_requirements": artifact("product/requirements.txt"),
                 "dependency_wheels": [artifact("product/dependency.whl")],
                 "model_catalog": artifact("product/catalog.json"),
                 "source_manifest": artifact("product/source-manifest.json")}
    paths = {"artifact_root": "product", "assembly": "product/assembly.json", "meta_wheel": "product/meta.whl",
             "toolkit_wheel": "product/connect.whl", "dependency_requirements": "product/requirements.txt",
             "dependency_wheels": ["product/dependency.whl"], "model_catalog": "product/catalog.json",
             "source_manifest": "product/source-manifest.json"}
    return {"files": files, "gateway": "guide/rpc_gateway", "selection": selection, "paths": paths}


ORDINARY = b"ordinary meta gateway bytes"
ORDINARY_ARCHIVE = gateway_archive(ORDINARY)


def ordinary_url(version="0.9.4", triple="x86_64-unknown-linux-gnu"):
    return (f"https://github.com/lukacf/meerkat-meta/releases/download/v{version}/"
            f"meta-rpc-gateway-{version}-{triple}.tar.gz")


def with_ordinary(entry, version="0.9.4", name="ordinary/rpc_gateway", triple=None):
    """Select the application's ordinary gateway: the selected Meta's rpc_gateway release."""
    entry["selection"]["meta"] = {"version": "0.9.4"}
    triple = triple or {"linux-x86_64": "x86_64-unknown-linux-gnu",
                        "macos-arm64": "aarch64-apple-darwin"}[entry["selection"]["target"]]
    entry["files"][name] = {"url": ordinary_url(version, triple), "archive_size": len(ORDINARY_ARCHIVE),
                            "archive_sha256": sha256(ORDINARY_ARCHIVE), "member": "rpc_gateway", **pin(ORDINARY)}
    entry["ordinary_gateway"] = name
    return entry


def inputs(targets=("linux-x86_64",)):
    return {"schema_version": 1, "kind": "forge-release-inputs",
            "guide": {"version": "0.9.0", "core_version": "0.9.0", "meta_source": "c" * 40, "core_source": "d" * 40},
            "targets": {target: target_inputs(target) for target in targets}}


def git(directory, *args):
    return subprocess.run(["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
                           "-c", "commit.gpgsign=false", *args], cwd=directory, check=True,
                          capture_output=True, text=True).stdout.strip()


def git_fixture(directory, value=None, version=VERSION):
    """A repository whose HEAD holds the Forge version and inputs files."""
    (directory / "installer/release/inputs/linux-x86_64").mkdir(parents=True)
    (directory / "installer/pyproject.toml").write_text(f'[project]\nname = "meta-forge"\nversion = "{version}"\n')
    (directory / forge.INPUTS).write_text(json.dumps(inputs() if value is None else value, indent=2))
    (directory / "installer/release/inputs/linux-x86_64/assembly.json").write_bytes(PRODUCT["product/assembly.json"])
    git(directory, "init", "-q")
    git(directory, "add", "-A")
    git(directory, "commit", "-q", "-m", "fixture")
    return git(directory, "rev-parse", "HEAD")


def environment(**values):
    base = {"GITHUB_REPOSITORY": REPO, "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": forge.MAIN}
    base.update(values)
    return mock.patch.dict(os.environ, base, clear=False)


def read_outputs(path):
    return dict(line.split("=", 1) for line in path.read_text().splitlines())


@contextlib.contextmanager
def settling_clock():
    """Re-reads without waiting: the re-read sleep only advances a fake clock.
    Yields the sleeps and the captured stderr, where each miss logs a line."""
    sleeps, now = [], [0.0]

    def sleep(seconds):
        sleeps.append(seconds)
        now[0] += seconds

    with mock.patch.object(forge.time, "sleep", sleep), mock.patch.object(forge.time, "monotonic", lambda: now[0]), \
            contextlib.redirect_stderr(io.StringIO()) as log:
        yield sleeps, log


class WorkflowShapeTests(unittest.TestCase):
    def test_only_a_dispatch_starts_it_with_exactly_three_inputs(self):
        source = text()
        on_block = source.split("\non:\n", 1)[1].split("\npermissions:\n", 1)[0]
        self.assertIn("  workflow_dispatch:\n", on_block)
        for trigger in ("push:", "pull_request", "schedule:", "tags:", "workflow_run", "release:"):
            self.assertNotIn(trigger, on_block)
        self.assertEqual(re.findall(r"^      ([a-z_]+):$", on_block, re.M),
                         ["release_mode", "source_sha", "artifact_selection"])
        self.assertIn("        options: [candidate, publish]\n", on_block)

    def test_no_job_reads_a_secret_and_tokens_are_the_run_token(self):
        source = text()
        self.assertNotIn("secrets.", source)
        tokens = re.findall(r"GH_TOKEN: (.*)", source)
        self.assertTrue(tokens)
        self.assertEqual(set(tokens), {"${{ github.token }}"})

    def test_every_action_is_pinned_like_the_rest_of_the_repository(self):
        pins = {}
        for other in (".github/workflows/release.yml", ".github/workflows/ci.yml"):
            for action, sha in re.findall(r"uses: ([\w./-]+)@([0-9a-f]{40}) #", (ROOT / other).read_text()):
                pins.setdefault(action, set()).add(sha)
        uses = re.findall(r"^\s*(?:- )?uses: (\S+)(.*)$", text(), re.M)
        self.assertTrue(uses)
        for reference, comment in uses:
            action, _, sha = reference.partition("@")
            self.assertRegex(sha, r"^[0-9a-f]{40}$", reference)
            self.assertRegex(comment, r"^ # v\d+$", reference)
            self.assertIn(sha, pins.get(action, set()), f"{action} differs from the repository's pin")

    def test_permissions_grant_writes_only_where_they_are_used(self):
        top = text().split("\npermissions:\n", 1)[1].split("\n\n", 1)[0]
        self.assertEqual([line for line in top.splitlines() if not line.strip().startswith("#")],
                         ["  actions: read", "  contents: read"])
        expected = {
            "resolve": None, "require_ci_green": None,
            "build_package": ["contents: read", "id-token: write", "attestations: write"],
            "collect": ["actions: read", "contents: read", "id-token: write", "attestations: write"],
            "publish": ["actions: read", "contents: write"],
        }
        for job, grants in expected.items():
            block = job_block(job)
            if grants is None:
                self.assertNotIn("    permissions:", block, job)
                continue
            listed = block.split("    permissions:\n", 1)[1].splitlines()
            self.assertEqual([line.strip() for line in listed[:len(grants)]], grants, job)
            self.assertFalse(listed[len(grants)].startswith("      "), job)
        self.assertEqual(text().count("contents: write"), 1)
        self.assertEqual(text().count("id-token: write"), 2)

    def test_checkouts_keep_no_credentials_and_scripts_get_inputs_through_env(self):
        for job in JOBS:
            block = job_block(job)
            self.assertEqual(block.count("uses: actions/checkout@"), block.count("persist-credentials: false"), job)
            for script in run_blocks(block):
                self.assertNotIn("${{", script, f"{job}: expressions reach scripts only through env")

    def test_the_package_build_runs_no_dependency_lifecycle_script(self):
        installs = [line for script in run_blocks(job_block("build_package"))
                    for line in script.splitlines() if "npm" in line and " ci" in line]
        self.assertEqual(len(installs), 2)
        for line in installs:
            self.assertIn("--ignore-scripts", line)
        self.assertIn("umask 077", job_block("build_package"))

    def test_every_script_command_parses(self):
        values = {"$FORGE_TARGET": "linux-x86_64", "$FORGE_VERSION": VERSION, "${FORGE_VERSION}": VERSION,
                  "${FORGE_TARGET}": "linux-x86_64", "$RUNNER_TEMP": "/runner", "$ARTIFACT_ID": "9",
                  "$ARTIFACT_DIGEST": "e" * 64}
        commands = []
        for job in JOBS:
            for script in run_blocks(job_block(job)):
                joined = script.replace("\\\n", " ")
                commands += [line for line in joined.splitlines() if "scripts/forge-release.py" in line]
        self.assertEqual(len(commands), 8)
        for command in commands:
            for name, value in values.items():
                command = command.replace(name, value)
            argv = shlex.split(command)
            self.assertEqual(argv[:2], ["python3", "scripts/forge-release.py"])
            forge.parse_args(argv[2:])

    def test_artifact_attestation_and_bundle_names_match_the_script(self):
        build, collect = job_block("build_package"), job_block("collect")
        self.assertIn("name: forge-package-${{ matrix.target }}-${{ github.run_attempt }}", build)
        self.assertEqual(forge.package_artifact_name("linux-x86_64", 3), "forge-package-linux-x86_64-3")
        self.assertIn("subject-path: dist/meta-forge-*.zip", build)
        self.assertEqual(forge.package_name(VERSION, "linux-x86_64"), "meta-forge-0.2.0-linux-x86_64.zip")
        self.assertIn('"dist/meta-forge-${FORGE_VERSION}-${FORGE_TARGET}-provenance.jsonl"', build)
        self.assertEqual(forge.provenance_name(VERSION, "linux-x86_64"),
                         "meta-forge-0.2.0-linux-x86_64-provenance.jsonl")
        self.assertIn("name: forge-candidate-manifest-${{ github.run_attempt }}", collect)
        self.assertEqual(forge.manifest_artifact_name(3), "forge-candidate-manifest-3")
        self.assertIn(f"subject-path: candidate/{forge.MANIFEST}", collect)
        self.assertIn(f"candidate/{forge.MANIFEST_PROVENANCE}", collect)
        self.assertIn("        id: manifest\n", collect)
        self.assertIn("ARTIFACT_ID: ${{ steps.manifest.outputs.artifact-id }}", collect)
        self.assertIn("ARTIFACT_DIGEST: ${{ steps.manifest.outputs.artifact-digest }}", collect)
        for block in (build, collect):
            self.assertIn("overwrite: false", block)
            self.assertIn("if-no-files-found: error", block)
            self.assertIn("retention-days: ${{ needs.resolve.outputs.publishable == 'true' && 90 || 7 }}", block)

    def test_the_release_build_and_forge_ci_declare_one_toolchain(self):
        build = job_block("build_package")
        self.assertIn("python-version: '3.12'", build)
        self.assertIn("node-version: '22'", build)
        source = text(FORGE_CI)
        self.assertEqual(set(re.findall(r"python-version: '([0-9.]+)'", source)), {"3.12"})
        self.assertEqual(set(re.findall(r"node-version: '([0-9.]+)'", source)), {"22"})

    def test_forge_ci_can_be_dispatched_and_records_each_jobs_toolchain(self):
        source = text(FORGE_CI)
        self.assertIn("\n  workflow_dispatch:\n", source.split("\npermissions:", 1)[0])
        self.assertIn("group: forge-${{ github.event.pull_request.number || github.run_id }}", source)
        for job in forge.EVIDENCE_JOBS:
            block = job_block(job, FORGE_CI)
            self.assertIn("name: forge-toolchain-${{ github.job }}-${{ github.run_attempt }}", block)
            self.assertIn(f"> {forge.TOOLCHAIN_FILE}", block)
            self.assertIn(f"path: {forge.TOOLCHAIN_FILE}", block)
        self.assertEqual(forge.toolchain_artifact_name("backend", 2), "forge-toolchain-backend-2")

    def test_every_forge_ci_artifact_name_carries_the_run_attempt(self):
        """Artifact names are unique within a run, so a rerun attempt that
        uploads a fixed name fails with a conflict; a shard's names also
        carry its shard."""
        names = {job: re.findall(r"^ {10}name: (.+)$", job_block(job, FORGE_CI), re.M)
                 for job in ("frontend", "backend-shard", "backend")}
        self.assertEqual(names, {
            "frontend": ["forge-toolchain-${{ github.job }}-${{ github.run_attempt }}"],
            "backend-shard": ["forge-shard-plan-${{ matrix.shard }}-${{ github.run_attempt }}",
                              "forge-toolchain-${{ github.job }}-${{ matrix.shard }}-${{ github.run_attempt }}",
                              "forge-pytest-${{ matrix.shard }}-${{ github.run_attempt }}"],
            "backend": ["forge-toolchain-${{ github.job }}-${{ github.run_attempt }}"]})
        shard = job_block("backend-shard", FORGE_CI)
        self.assertIn("      - name: Retain test results\n        if: always()\n", shard)
        self.assertIn("          path: forge-pytest.xml\n", shard)
        self.assertNotIn("overwrite:", text(FORGE_CI))

    def test_the_backend_job_runs_only_after_every_shard_passed_and_binds_their_toolchain(self):
        """forge-release.py binds the job named backend. It needs the shard
        matrix with the default success() condition, so a failed, cancelled or
        skipped shard leaves it unrun, and it records the one toolchain every
        shard recorded."""
        source = text(FORGE_CI)
        self.assertEqual(re.findall(r"(?m)^  ([A-Za-z_][A-Za-z0-9_-]*):$", source.split("\njobs:\n", 1)[1]),
                         ["frontend", "agent-team-live", "backend-shard", "backend"])
        shard, backend = job_block("backend-shard", FORGE_CI), job_block("backend", FORGE_CI)
        self.assertIn("\n    needs: [backend-shard, agent-team-live]\n", backend)
        self.assertNotRegex(backend, r"(?m)^    if:")
        legs = [int(leg) for leg in re.search(r"(?m)^        shard: \[([0-9, ]+)\]$", shard).group(1).split(", ")]
        self.assertEqual(legs, list(range(len(legs))))
        self.assertIn("\n      fail-fast: false\n", shard)
        self.assertIn(f"        run: python installer/tests/forge_shards.py ${{{{ matrix.shard }}}} {len(legs)} "
                      "forge-shard-files.txt forge-shard-startup.txt\n", shard)
        self.assertIn("        run: python -m pytest -vv --tb=short --durations=20 --junitxml=forge-pytest.xml "
                      "$(cat forge-shard-files.txt)\n", shard)
        startup = shard.split("      - name: Application startup checks with this checkout's Meta SDK\n", 1)[1]
        startup = startup.split("\n      - ", 1)[0]
        self.assertIn("          PYTHONPATH: sdk/python:installer\n", startup)
        self.assertIn("        if: ${{ !cancelled() }}\n", startup)
        self.assertIn("python -m pytest -q --tb=short $(cat forge-shard-startup.txt)", startup)
        self.assertIn("      - name: Isolated controller subprocess checks\n        if: matrix.shard == 0\n", shard)
        self.assertNotIn("--maxfail", source)
        # Each shard retains its run lists; backend fails unless together they
        # ran every test file once and reran every listed file once.
        self.assertIn("          name: forge-shard-plan-${{ matrix.shard }}-${{ github.run_attempt }}\n"
                      "          path: |\n            forge-shard-files.txt\n            forge-shard-startup.txt\n"
                      "          if-no-files-found: error\n", shard)
        steps = [line.strip() for line in backend.splitlines() if line.startswith("      - ")]
        self.assertEqual(steps[:4], ["- uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4",
            "- name: Collect the shards' plans", "- name: Check that the shards ran every test file once",
            "- name: Collect the shards' toolchains"])
        self.assertIn("          pattern: forge-shard-plan-*-${{ github.run_attempt }}\n          path: plans\n", backend)
        self.assertIn(f"        run: python3 installer/tests/forge_shards.py verify {len(legs)} plans\n", backend)
        self.assertIn("          pattern: forge-toolchain-backend-shard-*-${{ github.run_attempt }}\n"
                      "          path: shards\n", backend)
        record = backend.split("          python3 - > forge-toolchain.json <<'PY'\n", 1)[1].split("          PY\n", 1)[0]
        script = "".join(line.removeprefix("          ") + "\n" for line in record.splitlines())
        self.assertIn(f"if len(records) != {len(legs)} or len(values) != 1:", script)
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        value = {"python": "Python 3.12.13", "node": "v22.22.2"}
        for index in range(len(legs)):
            leg = folder / "shards" / f"forge-toolchain-backend-shard-{index}-1"
            leg.mkdir(parents=True)
            (leg / "forge-toolchain.json").write_text(json.dumps(value) + "\n")
        recorded = subprocess.run([sys.executable, "-c", script], cwd=folder, capture_output=True, text=True,
                                  check=True).stdout
        self.assertEqual(json.loads(recorded), value)
        (folder / "shards" / "forge-toolchain-backend-shard-3-1" / "forge-toolchain.json").write_text(
            json.dumps({**value, "python": "Python 3.12.12"}) + "\n")
        self.assertNotEqual(subprocess.run([sys.executable, "-c", script], cwd=folder, capture_output=True,
                                           text=True).returncode, 0)
        (folder / "shards" / "forge-toolchain-backend-shard-3-1" / "forge-toolchain.json").unlink()
        self.assertNotEqual(subprocess.run([sys.executable, "-c", script], cwd=folder, capture_output=True,
                                           text=True).returncode, 0)

    def test_the_shard_plan_runs_every_file_and_every_rerun_exactly_once_and_balances_them(self):
        shards = load("forge_shards", ROOT / "installer/tests/forge_shards.py")
        files = shards.test_files()
        self.assertGreater(len(files), 100)
        weighted = {name: float((index * 37) % 101) for index, name in enumerate(files)}
        reruns = set(files[::5])
        plan = shards.plan(weighted, 12, reruns)
        ran = [name for mine, _ in plan for name in mine]
        again = [name for _, rerun in plan for name in rerun]
        self.assertEqual(sorted(ran), files)
        self.assertEqual(sorted(again), sorted(reruns))
        loads = [sum(weighted[name] for name in mine + rerun) for mine, rerun in plan]
        self.assertLessEqual(max(loads) - min(loads), max(weighted.values()))
        self.assertEqual(shards.plan(weighted, 12, reruns), plan)

    def test_weights_use_recorded_seconds_and_fall_back_to_items_at_the_median_rate(self):
        shards = load("forge_shards", ROOT / "installer/tests/forge_shards.py")
        counts = {"test_a.py": 10, "test_b.py": 4, "test_c.py": 6, "test_new.py": 3, "test_empty.py": 0}
        recorded = {"test_a.py": 20.0, "test_b.py": 40.0, "test_c.py": 6.0, "test_gone.py": 99.0}
        # Median recorded rate: 2 s per item, from 2, 10 and 1 s per item.
        weighted = shards.weights(counts, recorded)
        self.assertEqual(weighted, {"test_a.py": 20.0, "test_b.py": 40.0, "test_c.py": 6.0, "test_new.py": 6.0,
                                    "test_empty.py": 0.0})
        self.assertEqual(shards.weights({"test_x.py": 5}, {}), {"test_x.py": 5.0})
        # The rerun of b is its own run of 40 s, so b's two runs land on different shards.
        self.assertEqual(shards.plan(weighted, 2, {"test_b.py"}),
                         [(["test_a.py", "test_b.py"], []),
                          (["test_c.py", "test_empty.py", "test_new.py"], ["test_b.py"])])
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        (folder / "seconds.json").write_text('{"test_a.py": "slow"}')
        with self.assertRaises(SystemExit):
            shards.recorded_seconds(folder / "seconds.json")
        junit = folder / "junit.xml"
        junit.write_text('<testsuites><testsuite>'
                         '<testcase classname="tests.test_a" name="one" time="1.25"/>'
                         '<testcase classname="tests.test_a" name="two" time="2"/>'
                         '<testcase classname="tests.test_b" name="one" time="0.5"/>'
                         '<testcase classname="tests.test_b.ContractTests" name="two" time="1"/>'
                         '</testsuite></testsuites>')
        shards.record([junit], folder / "seconds.json")
        self.assertEqual(json.loads((folder / "seconds.json").read_text()), {"test_a.py": 3.2, "test_b.py": 1.5})
        committed = shards.recorded_seconds()
        self.assertGreater(len(committed), 100)
        self.assertTrue(all(re.fullmatch(r"test_[A-Za-z0-9_]+\.py", name) for name in committed))

    def test_the_gate_check_fails_unless_the_shards_ran_every_file_once_and_reran_every_listed_file_once(self):
        shards = load("forge_shards", ROOT / "installer/tests/forge_shards.py")
        files = shards.test_files()
        output = "".join(f"tests/{name}: {index % 7 + 1}\n" for index, name in enumerate(files))
        plans = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, plans)
        for shard in range(12):
            folder = plans / f"forge-shard-plan-{shard}-1"
            folder.mkdir()
            with mock.patch.object(shards.subprocess, "run",
                                   return_value=subprocess.CompletedProcess([], 0, output, "")):
                shards.main([str(shard), "12", str(folder / "forge-shard-files.txt"),
                             str(folder / "forge-shard-startup.txt")])
        with contextlib.redirect_stdout(io.StringIO()) as printed:
            shards.main(["verify", "12", str(plans)])
        self.assertIn(f"12 shards ran {len(shards.tracked_test_files())} test files once", printed.getvalue())
        self.assertEqual(shards.tracked_test_files(), files)

        def refused(change):
            copy = Path(tempfile.mkdtemp())
            self.addCleanup(shutil.rmtree, copy)
            shutil.copytree(plans, copy, dirs_exist_ok=True)
            change(copy)
            with self.assertRaises(SystemExit) as refusal:
                shards.verify(12, copy)
            return str(refusal.exception)

        def edit(name, shard, transform):
            def change(copy):
                path = copy / f"forge-shard-plan-{shard}-1" / name
                path.write_text(transform(path.read_text()))
            return change

        first = lambda text: text.splitlines(keepends=True)[0]
        dropped = refused(edit("forge-shard-files.txt", 3, lambda text: "".join(text.splitlines(keepends=True)[1:])))
        self.assertIn("ran nowhere", dropped)
        self.assertIn(first((plans / "forge-shard-plan-3-1/forge-shard-files.txt").read_text()).strip()
                      .removeprefix("installer/tests/"), dropped)
        twice = (plans / "forge-shard-plan-4-1/forge-shard-files.txt").read_text()
        self.assertIn("ran more than once", refused(edit("forge-shard-files.txt", 5, lambda text: text + first(twice))))
        self.assertIn("reran nowhere", refused(edit("forge-shard-startup.txt", next(
            shard for shard in range(12) if (plans / f"forge-shard-plan-{shard}-1/forge-shard-startup.txt").read_text()),
            lambda text: "".join(text.splitlines(keepends=True)[1:]))))
        self.assertIn("11 shard plans instead of 12", refused(lambda copy: shutil.rmtree(copy / "forge-shard-plan-7-1")))
        # A test file at any depth counts: one in a subdirectory ran nowhere.
        with self.assertRaises(SystemExit) as hidden:
            shards.verify(12, plans, files + ["test_nested/test_hidden.py"])
        self.assertIn("ran nowhere: test_nested/test_hidden.py", str(hidden.exception))

    def test_a_collected_file_no_shard_can_place_fails_every_shard(self):
        shards = load("forge_shards", ROOT / "installer/tests/forge_shards.py")
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        files = shards.test_files()
        nested = subprocess.CompletedProcess([], 0, f"tests/{files[0]}: 3\ntests/sub/test_nested.py: 2\n", "")
        with mock.patch.object(shards.subprocess, "run", return_value=nested), self.assertRaises(SystemExit) as refused:
            shards.main(["0", "12", str(folder / "files.txt"), str(folder / "startup.txt")])
        self.assertIn("tests/sub/test_nested.py: 2", str(refused.exception))
        self.assertFalse((folder / "files.txt").exists())

    def test_a_failed_collection_runs_no_shard(self):
        shards = load("forge_shards", ROOT / "installer/tests/forge_shards.py")
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        failed = subprocess.CompletedProcess([], 2, "ERROR tests/test_broken.py\n", "")
        with mock.patch.object(shards.subprocess, "run", return_value=failed), \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as refused:
            shards.main(["0", "12", str(folder / "files.txt"), str(folder / "startup.txt")])
        self.assertIn("collection failed (exit 2)", str(refused.exception))
        self.assertFalse((folder / "files.txt").exists() or (folder / "startup.txt").exists())
        listed = subprocess.CompletedProcess([], 0, "tests/test_unknown_elsewhere.py: 3\n", "")
        with mock.patch.object(shards.subprocess, "run", return_value=listed), self.assertRaises(SystemExit):
            shards.main(["0", "12", str(folder / "files.txt"), str(folder / "startup.txt")])

    def test_the_shards_rerun_every_listed_startup_file_once_with_the_checkout_sdk(self):
        shards = load("forge_shards", ROOT / "installer/tests/forge_shards.py")
        files = shards.test_files()
        output = "".join(f"tests/{name}: {index % 7 + 1}\n" for index, name in enumerate(files))
        folder = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, folder)
        startup = shards.startup_files()
        seen, rerun = [], []
        for shard in range(12):
            with mock.patch.object(shards.subprocess, "run",
                                   return_value=subprocess.CompletedProcess([], 0, output, "")):
                shards.main([str(shard), "12", str(folder / "files.txt"), str(folder / "startup.txt")])
            mine = (folder / "files.txt").read_text().splitlines()
            again = (folder / "startup.txt").read_text().splitlines()
            self.assertTrue(all(line.startswith("installer/tests/test_") for line in mine + again))
            seen += mine
            rerun += again
        self.assertEqual(sorted(seen), [f"installer/tests/{name}" for name in files])
        self.assertEqual(sorted(rerun), sorted(f"installer/tests/{name}" for name in startup))

    def test_the_checkout_sdk_startup_list_names_every_startup_test(self):
        """The list holds exactly the test files matching the startup pattern,
        then five named extras the pattern misses, each with its reason."""
        pattern = re.compile(r"templates_v2/app\.py|launch_host|application_startup")
        tests = ROOT / "installer/tests"
        matching = sorted(path.name for path in tests.glob("test_*.py") if pattern.search(path.read_text()))
        listed = (tests / "checkout_sdk_startup_tests.txt").read_text().splitlines()
        marker = listed.index("# Extras.")
        main = [line for line in listed[:marker] if line and not line.startswith("#")]
        extras = {}
        for line in listed[marker + 1:]:
            name, _, reason = line.partition("  # ")
            extras[name] = reason
        self.assertEqual(main, matching, "a test file matching the startup pattern is missing from the list, "
                                         "or a listed one no longer matches")
        self.assertEqual(sorted(extras), ["test_helperless_application_source.py", "test_implementation_fresh_build.py", "test_native_callback_member_contract.py", "test_native_console_login.py",
                                          "test_product_start_entry.py"])
        for name, reason in extras.items():
            self.assertTrue((tests / name).is_file(), name)
            self.assertGreater(len(reason.split()), 3, name)
            self.assertIsNone(pattern.search((tests / name).read_text()), f"{name} matches; list it with the others")

    def test_single_line_run_values_stay_plain_yaml_scalars(self):
        """A plain scalar cannot hold ": " or " #", nor start with a YAML
        indicator: forge.yml once carried a printf of JSON that way and no
        longer parsed. Multi-line scripts use a block scalar."""
        for path in (WORKFLOW, FORGE_CI):
            for line in text(path).splitlines():
                match = re.match(r"\s*(?:- )?run: (.*)$", line)
                if not match or match.group(1) in ("|", "|-", ">", ">-"):
                    continue
                value = match.group(1)
                self.assertNotIn(": ", value, f"{path.name}: {line}")
                self.assertNotIn(" #", value, f"{path.name}: {line}")
                self.assertFalse(value[:1] in "'\"{[&*!|>%@`", f"{path.name}: {line}")

    @unittest.skipUnless(importlib.util.find_spec("yaml"), "PyYAML is not installed")
    def test_the_forge_workflows_parse_as_yaml(self):
        import yaml

        release = yaml.safe_load(text(WORKFLOW))
        self.assertEqual(sorted(release["jobs"]), sorted(JOBS))
        forge_ci = yaml.safe_load(text(FORGE_CI))
        # The jobs that ran the tests record their own versions; the backend
        # job binds the shards' (see the shard test above).
        for job in ("frontend", "backend-shard"):
            steps = {step.get("name"): step for step in forge_ci["jobs"][job]["steps"]}
            self.assertEqual(steps["Record the toolchain"]["run"],
                             "printf '{\"python\": \"%s\", \"node\": \"%s\"}\\n' \"$(python3 --version 2>&1)\" "
                             "\"$(node --version)\" > forge-toolchain.json\n")

    @unittest.skipUnless(importlib.util.find_spec("yaml"), "PyYAML is not installed")
    def test_forge_pull_request_paths_and_the_release_binding_agree(self):
        """A path forge.yml's filters select starts Forge's suite on a pull
        request, and a release binds a run that tested the same inputs. Each
        path is an evidence tree, an evidence file or a root file named as
        unbound, and each evidence tree and file is a path the filters
        select."""
        import yaml

        workflow = yaml.safe_load(text(FORGE_CI))
        paths = workflow.get("on", workflow.get(True))["pull_request"]["paths"]
        trees = {path[:-3] for path in paths if path.endswith("/**")}
        unbound = [path for path in paths if not (path.endswith("/**") and path[:-3] in forge.EVIDENCE_TREES)
                   and path not in forge.EVIDENCE_FILES and path not in FORGE_PATHS_NOT_BOUND]
        self.assertEqual(unbound, [])
        self.assertLessEqual(set(forge.EVIDENCE_TREES), trees)
        unselected = [path for path in forge.EVIDENCE_FILES
                      if path not in paths and not any(path.startswith(f"{tree}/") for tree in trees)]
        self.assertEqual(unselected, [])

    def test_the_guide_says_a_partial_rerun_is_not_evidence(self):
        guide = (ROOT / "installer/RELEASING.md").read_text()
        self.assertIn("A rerun of only the\nfailed jobs", guide)
        self.assertIn("rerun all jobs, when the run must count", guide)

    def test_ci_runs_these_tests_and_meta_release_stays_separate(self):
        self.assertIn("      - run: python3 scripts/test_forge_release.py\n", (ROOT / ".github/workflows/ci.yml").read_text())
        self.assertNotIn("forge", (ROOT / ".github/workflows/release.yml").read_text().lower())
        self.assertNotIn("forge", (ROOT / "scripts/release-candidate.py").read_text().lower())

    def test_the_ci_gate_and_collection_start_after_every_package_leg(self):
        """GUARD: a job that needs the build_package matrix starts only after
        every leg finished, and sees success only when every leg succeeded.
        collect re-reads the artifact listing while a package name is missing,
        which is a listing that has not caught up only if no leg still runs."""
        self.assertIn("      matrix: ${{ fromJSON(needs.resolve.outputs.matrix) }}\n", job_block("build_package"))
        self.assertIn("build_package", job_needs("require_ci_green"))
        self.assertIn("needs.build_package.result == 'success'", job_condition("require_ci_green"))
        self.assertTrue({"build_package", "require_ci_green"} <= set(job_needs("collect")))
        self.assertIn("needs.build_package.result == 'success'", job_condition("collect"))
        self.assertIn("needs.require_ci_green.result == 'success'", job_condition("collect"))
        for publishable in ("true", "false"):
            for result in ("failure", "cancelled", "skipped"):
                current = context(publishable=publishable, build_package=result)
                for job in ("require_ci_green", "collect"):
                    self.assertFalse(release_tests.evaluate_condition(job_condition(job), **current),
                                     (job, publishable, result))


class WorkflowGraphTests(unittest.TestCase):
    def test_every_job_after_resolve_sets_its_own_status_check(self):
        for job in JOBS[1:]:
            self.assertTrue(job_condition(job).startswith("!cancelled() &&"), job)
        self.assertEqual(job_needs("build_package"), ["resolve"])
        self.assertEqual(job_needs("require_ci_green"), ["resolve", "build_package"])
        self.assertEqual(job_needs("collect"), ["resolve", "build_package", "require_ci_green"])
        self.assertEqual(job_needs("publish"), ["resolve"])

    def test_a_main_candidate_binds_only_after_exact_main_ci(self):
        self.assertEqual(outcomes(), {"resolve": "success", "build_package": "success",
                                      "require_ci_green": "success", "collect": "success", "publish": "skipped"})
        refused = outcomes(failures={"require_ci_green": "failure"})
        self.assertEqual(refused["collect"], "skipped")

    def test_a_branch_dry_run_binds_without_the_main_ci_gate(self):
        self.assertEqual(outcomes(publishable="false"), {
            "resolve": "success", "build_package": "success", "require_ci_green": "skipped",
            "collect": "success", "publish": "skipped"})

    def test_a_failed_build_binds_nothing(self):
        for publishable in ("true", "false"):
            results = outcomes(publishable=publishable, failures={"build_package": "failure"})
            self.assertEqual((results["require_ci_green"], results["collect"]), ("skipped", "skipped"))

    def test_publish_mode_builds_nothing(self):
        self.assertEqual(outcomes(mode="publish", publishable="false"), {
            "resolve": "success", "build_package": "skipped", "require_ci_green": "skipped",
            "collect": "skipped", "publish": "success"})

    def test_a_cancelled_run_starts_nothing_after_resolve(self):
        results = outcomes(run_cancelled=True)
        self.assertEqual({job for job, result in results.items() if result != "skipped"}, {"resolve"})

    def test_a_main_candidate_never_binds_with_a_skipped_gate(self):
        current = context(require_ci_green="skipped")
        self.assertFalse(release_tests.evaluate_condition(job_condition("collect"), **current))


class InputsTests(unittest.TestCase):
    def test_the_committed_inputs_file_is_valid(self):
        value = forge.validate_inputs(json.loads((ROOT / forge.INPUTS).read_text()))
        skeleton = json.loads((ROOT / forge.INPUTS_DIR / "forge-release-inputs.skeleton.json").read_text())
        # The Guide is the skeleton's published release, and every target is pinned.
        self.assertEqual(value["guide"], skeleton["guide"])
        self.assertEqual(sorted(value["targets"]), sorted(forge.TARGETS))

    def test_the_guide_skeleton_is_refused_until_every_pending_value_is_filled(self):
        selected = guide_sdk.SELECTED_VERSION
        skeleton = json.loads((ROOT / forge.INPUTS_DIR / "forge-release-inputs.skeleton.json").read_text())
        guide = skeleton["guide"]
        # The gateway and the Guide's client come from the same Meta release commit.
        self.assertEqual((guide["version"], guide["core_version"], guide["meta_source"]),
                         (selected, selected, guide_sdk.SOURCE))
        self.assertTrue(re.fullmatch("[0-9a-f]{40}", guide["core_source"]))
        self.assertEqual(list(skeleton["targets"]), ["linux-x86_64"])
        entry = skeleton["targets"]["linux-x86_64"]
        self.assertEqual((list(entry["files"]), entry["selection"], entry["paths"]), ([entry["gateway"]], None, None))
        self.assertEqual(entry["files"][entry["gateway"]]["url"],
                         f"https://github.com/lukacf/meerkat-meta/releases/download/v{selected}/"
                         f"meta-rpc-gateway-{selected}-x86_64-unknown-linux-gnu.tar.gz")
        # Only the product owner's selection, its paths and their files are pending.
        with self.assertRaisesRegex(forge.ProtocolError, "owner's product selection"):
            forge.validate_inputs(skeleton)
        # With a fixture owner selection, every value the skeleton already holds is valid.
        product = target_inputs()
        filled = copy.deepcopy(skeleton)
        target = filled["targets"]["linux-x86_64"]
        target["files"].update({name: pin for name, pin in product["files"].items() if name != product["gateway"]})
        target.update(selection=product["selection"], paths=product["paths"])
        forge.validate_inputs(filled)

        def gateway_value(key):
            return lambda value: value["targets"]["linux-x86_64"]["files"]["guide/rpc_gateway"].__setitem__(key, None)

        pending = {
            "meta source": lambda value: value["guide"].__setitem__("meta_source", None),
            "core source": lambda value: value["guide"].__setitem__("core_source", None),
            "selection": lambda value: value["targets"]["linux-x86_64"].__setitem__("selection", None),
            "paths": lambda value: value["targets"]["linux-x86_64"].__setitem__("paths", None),
            "product files": lambda value: value["targets"]["linux-x86_64"].__setitem__(
                "files", {"guide/rpc_gateway": value["targets"]["linux-x86_64"]["files"]["guide/rpc_gateway"]}),
            **{f"gateway {key}": gateway_value(key) for key in ("archive_size", "archive_sha256", "size", "sha256")},
        }
        for label, reset in pending.items():
            with self.subTest(label):
                value = copy.deepcopy(filled)
                reset(value)
                with self.assertRaises(forge.ProtocolError):
                    forge.validate_inputs(value)

    def test_a_complete_target_is_valid(self):
        forge.validate_inputs(inputs(("linux-x86_64", "macos-arm64")))

    def refused(self, mutate, message):
        value = inputs()
        mutate(value["targets"]["linux-x86_64"], value)
        with self.assertRaisesRegex(forge.ProtocolError, message):
            forge.validate_inputs(value)

    def test_unsafe_names_and_sources_are_refused(self):
        def rename(name):
            return lambda entry, _: entry["files"].__setitem__(name, entry["files"].pop("product/catalog.json"))
        self.refused(rename("../catalog.json"), "unsafe")
        self.refused(rename("/catalog.json"), "unsafe")
        self.refused(rename("product/.catalog.json"), "unsafe")
        for url in ("https://example.com/x", "http://github.com/lukacf/meerkat-meta/releases/download/x",
                    URL + "x?y", URL + "x#sha256=1", URL + "../x", "https://github.com/lukacf/meerkat-toolkit/x"):
            self.refused(lambda entry, _, url=url: entry["files"]["product/catalog.json"].__setitem__("url", url),
                         "allowlisted")
        self.refused(lambda entry, _: entry["files"]["product/assembly.json"].__setitem__("path", "installer/x.json"),
                     "must live under")
        self.refused(lambda entry, _: entry["files"]["guide/rpc_gateway"].__setitem__("url", URL + "x.zip"),
                     "tar.gz")

    def test_pins_must_be_exact(self):
        self.refused(lambda entry, _: entry["files"]["product/meta.whl"].__setitem__("size", 0), "positive")
        self.refused(lambda entry, _: entry["files"]["product/meta.whl"].__setitem__("sha256", "A" * 64), "SHA-256")
        self.refused(lambda entry, _: entry["files"]["product/meta.whl"].__setitem__("extra", 1), "fields")
        self.refused(lambda entry, _: entry.__setitem__("gateway", "guide/missing"), "gateway")
        self.refused(lambda entry, _: entry.__setitem__("gateway", "product/meta.whl"), "outside the product root")

    def test_the_selection_and_its_pins_must_agree(self):
        self.refused(lambda entry, _: entry["selection"].__setitem__("target", "macos-arm64"), "selection")
        self.refused(lambda entry, _: entry["selection"]["model_catalog"].__setitem__("sha256", "f" * 64),
                     "model_catalog differs")
        self.refused(lambda entry, _: entry["selection"]["dependency_wheels"][0].__setitem__("size", 1),
                     "dependency wheel differs")
        self.refused(lambda entry, _: entry["paths"].__setitem__("model_catalog", "guide/rpc_gateway"),
                     "not a pinned input under its artifact root")
        self.refused(lambda entry, _: entry["paths"].__setitem__("dependency_wheels", []), "dependency wheels")
        self.refused(lambda entry, _: entry["paths"].pop("source_manifest"), "paths fields")
        self.refused(lambda _, value: value["targets"].__setitem__("windows-x86_64", {}), "unknown Forge target")
        self.refused(lambda _, value: value["guide"].__setitem__("meta_source", "c" * 39), "guide")


    def test_an_ordinary_gateway_is_the_selected_meta_rpc_gateway_release(self):
        value = inputs(("linux-x86_64", "macos-arm64"))
        for entry in value["targets"].values():
            with_ordinary(entry)
        forge.validate_inputs(value)
        message = "ordinary gateway must be the Meta 0.9.4 rpc_gateway release asset for linux-x86_64"
        self.refused(lambda entry, _: with_ordinary(entry, version="0.9.3"), message)
        self.refused(lambda entry, _: with_ordinary(entry, triple="aarch64-apple-darwin"), message)
        self.refused(lambda entry, _: with_ordinary(entry)["files"]["ordinary/rpc_gateway"].__setitem__(
            "member", "meta_gateway"), message)
        self.refused(lambda entry, _: with_ordinary(entry)["selection"].__setitem__("meta", {}), "ordinary gateway")
        separate = "separate pinned input outside the product root"
        self.refused(lambda entry, _: with_ordinary(entry).__setitem__("ordinary_gateway", "guide/rpc_gateway"),
                     separate)
        self.refused(lambda entry, _: with_ordinary(entry).__setitem__("ordinary_gateway", "ordinary/missing"),
                     separate)
        self.refused(lambda entry, _: with_ordinary(entry, name="product/rpc_gateway"), separate)


class ResolveTests(unittest.TestCase):
    def resolve(self, directory, head, **env):
        output = directory / f"output-{uuid.uuid4().hex}"
        values = {"GITHUB_SHA": head, "GITHUB_WORKFLOW_SHA": head, "RELEASE_MODE": "candidate", "SOURCE_SHA": head,
                  "ARTIFACT_SELECTION": "", "GITHUB_OUTPUT": str(output)}
        values.update(env)
        with environment(**values), working_directory(directory), contextlib.redirect_stdout(io.StringIO()):
            forge.resolve()
        return read_outputs(output)

    def test_a_main_candidate_is_publishable_and_a_branch_one_is_not(self):
        with fixture_directory() as directory:
            head = git_fixture(directory)
            main = self.resolve(directory, head)
            self.assertEqual((main["mode"], main["sha"], main["version"], main["tag"], main["publishable"]),
                             ("candidate", head, VERSION, TAG, "true"))
            self.assertEqual(json.loads(main["matrix"]), {"include": [{"target": "linux-x86_64"}]})
            branch = self.resolve(directory, head, GITHUB_REF="refs/heads/ci/forge-dry-run")
            self.assertEqual(branch["publishable"], "false")

    def test_candidate_refusals(self):
        with fixture_directory() as directory:
            head = git_fixture(directory)
            for env, message in (({"GITHUB_EVENT_NAME": "push"}, "dispatched"),
                                 ({"SOURCE_SHA": OTHER}, "SOURCE_SHA"),
                                 ({"GITHUB_SHA": OTHER}, "checkout/event/workflow"),
                                 ({"ARTIFACT_SELECTION": "{}"}, "selection"),
                                 ({"GITHUB_REF": "refs/tags/forge-v0.2.0"}, "branch"),
                                 ({"RELEASE_MODE": "promote"}, "mode")):
                with self.subTest(env=env), self.assertRaisesRegex(forge.ProtocolError, message):
                    self.resolve(directory, head, **env)

    def test_a_candidate_needs_a_pinned_target_and_a_valid_version(self):
        with fixture_directory() as directory:
            head = git_fixture(directory, inputs(()))
            with self.assertRaisesRegex(forge.ProtocolError, "no Forge target"):
                self.resolve(directory, head)
        with fixture_directory() as directory:
            head = git_fixture(directory, version="0.2")
            with self.assertRaisesRegex(forge.ProtocolError, "invalid Forge version"):
                self.resolve(directory, head)

    def test_publish_reads_only_the_selection_from_main(self):
        selection = json.dumps(selection_value())
        with fixture_directory() as directory:
            head = git_fixture(directory, {"broken": True})
            outputs = self.resolve(directory, head, RELEASE_MODE="publish", SOURCE_SHA="",
                                   ARTIFACT_SELECTION=selection)
            self.assertEqual((outputs["mode"], outputs["version"], outputs["tag"], outputs["publishable"]),
                             ("publish", "", "", "false"))
            for env, message in (({"GITHUB_REF": "refs/heads/ci/x"}, "from main"),
                                 ({"SOURCE_SHA": head}, "SOURCE_SHA"),
                                 ({"ARTIFACT_SELECTION": "{}"}, "selection")):
                with self.subTest(env=env), self.assertRaisesRegex(forge.ProtocolError, message):
                    self.resolve(directory, head, RELEASE_MODE="publish",
                                 **{"SOURCE_SHA": "", "ARTIFACT_SELECTION": selection, **env})


class FetchTests(unittest.TestCase):
    def fetch(self, directory, served, target="linux-x86_64"):
        def download(url, destination, limit):
            data = served[url]
            forge.require(len(data) <= limit, f"{url} exceeds its pinned size")
            destination.write_bytes(data)

        output = directory / "inputs"
        with working_directory(directory), mock.patch.object(forge, "ROOT", directory), \
                mock.patch.object(forge, "download", download), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(forge.main(["fetch-inputs", "--target", target, "--output-dir", str(output)]), 0)
        return output

    def served(self):
        value = {URL + name.rsplit("/", 1)[1]: data for name, data in PRODUCT.items()}
        value[GATEWAY_URL] = ARCHIVE
        return value

    def test_inputs_land_pinned_with_private_modes(self):
        with fixture_directory() as directory:
            git_fixture(directory)
            output = self.fetch(directory, self.served())
            for name, data in PRODUCT.items():
                self.assertEqual((output / name).read_bytes(), data)
                self.assertEqual((output / name).stat().st_mode & 0o777, 0o600)
            self.assertEqual((output / "guide/rpc_gateway").read_bytes(), GATEWAY)
            self.assertEqual((output / "guide/rpc_gateway").stat().st_mode & 0o777, 0o700)
            self.assertFalse((directory / "inputs-downloads").exists())

    def test_the_ordinary_gateway_lands_pinned_and_executable(self):
        value = inputs()
        with_ordinary(value["targets"]["linux-x86_64"])
        with fixture_directory() as directory:
            git_fixture(directory, value)
            served = {**self.served(), ordinary_url(): ORDINARY_ARCHIVE}
            output = self.fetch(directory, served)
            self.assertEqual((output / "ordinary/rpc_gateway").read_bytes(), ORDINARY)
            self.assertEqual((output / "ordinary/rpc_gateway").stat().st_mode & 0o777, 0o700)
            self.assertEqual((output / "product/meta.whl").stat().st_mode & 0o777, 0o600)

    def test_changed_or_missing_bytes_are_refused(self):
        cases = {"changed product bytes": (URL + "catalog.json", b'{"models": [1]}'),
                 "changed archive": (GATEWAY_URL, gateway_archive(b"other gateway bytes")),
                 "missing member": (GATEWAY_URL, gateway_archive(name="other"))}
        for label, (url, data) in cases.items():
            with self.subTest(label), fixture_directory() as directory:
                git_fixture(directory)
                served = self.served()
                served[url] = data
                with working_directory(directory), mock.patch.object(forge, "ROOT", directory), \
                        mock.patch.object(forge, "download", lambda u, d, _l, s=served: d.write_bytes(s[u])), \
                        self.assertRaises((forge.ProtocolError, tarfile.TarError)):
                    forge.fetch_inputs(forge.parse_args(["fetch-inputs", "--target", "linux-x86_64",
                                                         "--output-dir", str(directory / "inputs")]))

    def test_an_existing_inputs_directory_is_refused(self):
        with fixture_directory() as directory:
            git_fixture(directory)
            (directory / "inputs").mkdir()
            with working_directory(directory), self.assertRaisesRegex(forge.ProtocolError, "must be new"):
                forge.fetch_inputs(forge.parse_args(["fetch-inputs", "--target", "linux-x86_64",
                                                     "--output-dir", str(directory / "inputs")]))


class DownloadTests(unittest.TestCase):
    class Response(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *_):
            self.close()

    def test_download_stops_at_the_pinned_size_and_retries_only_network_errors(self):
        with fixture_directory() as directory:
            with mock.patch.object(forge.urllib.request, "urlopen", return_value=self.Response(b"x" * 11)), \
                    self.assertRaisesRegex(forge.ProtocolError, "exceeds its pinned size"):
                forge.download(URL + "a", directory / "a", 10)
            calls = []

            def flaky(request, timeout):
                calls.append(request.full_url)
                if len(calls) < 3:
                    raise urllib.error.URLError("reset")
                return self.Response(b"ok")

            with mock.patch.object(forge.urllib.request, "urlopen", flaky), mock.patch.object(forge.time, "sleep"):
                forge.download(URL + "b", directory / "b", 10)
            self.assertEqual(((directory / "b").read_bytes(), len(calls)), (b"ok", 3))
            with mock.patch.object(forge.urllib.request, "urlopen", side_effect=urllib.error.URLError("down")), \
                    mock.patch.object(forge.time, "sleep"), self.assertRaisesRegex(forge.ProtocolError, "3 attempts"):
                forge.download(URL + "c", directory / "c", 10)
            self.assertFalse((directory / "c").exists())
            with self.assertRaisesRegex(forge.ProtocolError, "allowlisted"):
                forge.download("https://example.com/d", directory / "d", 10)


def package_zip(path, members=None, *, mode=0o644, link=False):
    members = members or {"start.sh": b"#!/bin/sh\n", "checker-dependencies/node_modules/x/.npmignore": b"x"}
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in members.items():
            info = zipfile.ZipInfo(name)
            info.external_attr = ((0o120000 if link else 0o100000) | mode) << 16
            archive.writestr(info, data)
    return path


class CheckPackageTests(unittest.TestCase):
    def check(self, archive, target="linux-x86_64", native="linux-x86_64"):
        seen = []

        class Distribution:
            @staticmethod
            def verify(root):
                seen.append(sorted(p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()))
                return {"support": {"native_target": native}}

        with mock.patch.object(forge, "launchkit_module", lambda name: Distribution), \
                contextlib.redirect_stdout(io.StringIO()):
            forge.check_package(forge.parse_args(["check-package", "--target", target, "--version", VERSION,
                                                  str(archive)]))
        return seen

    def test_a_verified_package_gets_its_checksum_file(self):
        with fixture_directory() as directory:
            archive = package_zip(directory / forge.package_name(VERSION, "linux-x86_64"))
            seen = self.check(archive)
            self.assertEqual(seen, [["checker-dependencies/node_modules/x/.npmignore", "start.sh"]])
            checksum = archive.with_name(archive.name + ".sha256").read_text()
            self.assertEqual(checksum, f"{sha256(archive.read_bytes())}  {archive.name}\n")
            self.assertFalse(archive.with_name(archive.name + ".verify").exists())

    def test_unsafe_or_foreign_packages_are_refused(self):
        name = forge.package_name(VERSION, "linux-x86_64")
        cases = {
            "parent member": ({"../escape": b"x"}, {}, "unsafe package member"),
            "absolute member": ({"/etc/x": b"x"}, {}, "unsafe package member"),
            "link member": (None, {"link": True}, "plain file"),
            "other target": (None, {}, "another target"),
        }
        for label, (members, options, message) in cases.items():
            with self.subTest(label), fixture_directory() as directory:
                archive = package_zip(directory / name, members, **options)
                native = "macos-arm64" if label == "other target" else "linux-x86_64"
                with self.assertRaisesRegex(forge.ProtocolError, message):
                    self.check(archive, native=native)
                self.assertFalse(archive.with_name(name + ".sha256").exists())
        with fixture_directory() as directory:
            archive = package_zip(directory / "meta-forge-0.2.0-macos-arm64.zip")
            with self.assertRaisesRegex(forge.ProtocolError, "unexpected package name"):
                self.check(archive)
            archive = package_zip(directory / name)
            with mock.patch.object(forge, "MAX_ASSET", 10), self.assertRaisesRegex(forge.ProtocolError, "2 GiB"):
                self.check(archive)


def evidence_record(head=SHA, run_id=FORGE_RUN):
    return {"run_id": run_id, "run_attempt": 1, "head_sha": head, "event": "workflow_dispatch",
            "tuple": {"trees": {path: "1" * 40 for path in forge.EVIDENCE_TREES},
                      "files": {path: "2" * 40 for path in forge.EVIDENCE_FILES}},
            "toolchain": {"declared": {"python": "3.12", "node": "22"},
                          "backend": {"python": "Python 3.12.12", "node": "v22.21.1"},
                          "frontend": {"python": "Python 3.12.3", "node": "v22.21.1"}}}


def package_record(target="linux-x86_64", data=b"package bytes", artifact_id=11, attempt=ATTEMPT):
    return {"target": target, "name": forge.package_name(VERSION, target), "size": len(data),
            "sha256": sha256(data), "artifact": {"id": artifact_id, "name": forge.package_artifact_name(target, attempt),
                                                 "digest": "e" * 64}}


def manifest_value(ref=forge.MAIN, ci=None, packages=None):
    return {"schema_version": 1, "kind": "forge-candidate-manifest", "repository": REPO, "source_sha": SHA,
            "version": VERSION, "tag": TAG, "run_id": RUN, "run_attempt": ATTEMPT,
            "workflow": {"path": forge.WORKFLOW, "sha": SHA, "ref": ref},
            "ci": ({"run_id": CI_RUN, "run_attempt": 1} if ref == forge.MAIN else None) if ci is None else ci,
            "forge_ci": evidence_record(), "inputs_sha256": "f" * 64,
            "packages": packages or [package_record()]}


def selection_value(manifest_sha="9" * 64):
    return {"schema_version": 1, "kind": "forge-accepted-candidate", "repository": REPO, "run_id": RUN,
            "run_attempt": ATTEMPT, "manifest_artifact": {"id": 90, "digest": "8" * 64, "manifest_sha256": manifest_sha}}


class ManifestTests(unittest.TestCase):
    def test_valid_manifests_and_selections(self):
        with environment():
            forge.validate_manifest(manifest_value())
            forge.validate_manifest(manifest_value(ref="refs/heads/ci/dry-run"))
            forge.validate_selection(selection_value())

    def test_manifest_refusals(self):
        def mutated(change):
            value = manifest_value()
            change(value)
            return value

        cases = {
            "tag": (lambda v: v.__setitem__("tag", "v0.2.0"), "version/tag"),
            "workflow": (lambda v: v["workflow"].__setitem__("path", ".github/workflows/release.yml"), "workflow identity"),
            "main without CI": (lambda v: v.__setitem__("ci", None), "green CI"),
            "branch with CI": (lambda v: v["workflow"].__setitem__("ref", "refs/heads/x"), "green CI"),
            "package name": (lambda v: v["packages"][0].__setitem__("name", "forge.zip"), "name/target"),
            "artifact name": (lambda v: v["packages"][0]["artifact"].__setitem__("name", "forge-package-linux-x86_64-9"),
                              "artifact name"),
            "duplicate target": (lambda v: v["packages"].append(copy.deepcopy(v["packages"][0])), "unique"),
            "oversize": (lambda v: v["packages"][0].__setitem__("size", 2 * 1024**3), "release asset limit"),
            "tuple": (lambda v: v["forge_ci"]["tuple"]["trees"].pop("sdk/python"), "incomplete"),
            "pull request evidence": (lambda v: v["forge_ci"].__setitem__("event", "pull_request"),
                                      "dispatched forge.yml run"),
            "toolchain": (lambda v: v["forge_ci"]["toolchain"]["backend"].__setitem__("python", "Python 3.11.9"),
                          "toolchain differs"),
            "unknown key": (lambda v: v.__setitem__("extra", 1), "fields"),
        }
        for label, (change, message) in cases.items():
            with self.subTest(label), environment(), self.assertRaisesRegex(forge.ProtocolError, message):
                forge.validate_manifest(mutated(change))

    def test_selection_refusals(self):
        for change, message in ((lambda v: v.__setitem__("kind", "accepted-candidate"), "forge-accepted-candidate"),
                                (lambda v: v.__setitem__("repository", "lukacf/meerkat"), "repository"),
                                (lambda v: v["manifest_artifact"].__setitem__("id", 0), "positive"),
                                (lambda v: v.__setitem__("tier", "core"), "fields")):
            value = selection_value()
            change(value)
            with environment(), self.assertRaisesRegex(forge.ProtocolError, message):
                forge.validate_selection(value)


BASE_TREE = {
    ".github/workflows/forge.yml": "python-version: '3.12'\nnode-version: '22'\n",
    ".github/workflows/ci.yml": "ci",
    "console/package-lock.json": "console lock",
    "console/src/app.ts": "console",
    "crates/meerkat-meta/src/lib.rs": "rust",
    "crates/meerkat-meta/src/rpc_gateway.rs": "gateway",
    "installer/launchkit/server.py": "server",
    "installer/requirements-guide-sdk.txt": "guide requirements",
    "installer/requirements-standalone.txt": "standalone requirements",
    "installer/vendor/guide/meerkat_meta-0.9.2-py3-none-any.whl": "guide wheel",
    "installer/web/package-lock.json": "web lock",
    "packages/console-components/index.ts": "components",
    "packages/console-core/index.ts": "core",
    "sdk/python/meerkat_meta/__init__.py": "sdk",
}


class FakeGitHub:
    """Git objects, Forge runs, jobs and artifacts behind rc.api."""

    def __init__(self):
        self.trees, self.blobs, self.commits = {}, {}, {}
        self.runs, self.jobs, self.artifacts, self.downloads = [], {}, {}, {}
        self.listings = []
        self.extra = {}
        # Run ID -> how many more forge.yml listings omit it, as GitHub's can.
        self.hidden = {}
        self.calls = []

    def tree(self, node):
        entries = []
        for name, child in sorted(node.items()):
            if isinstance(child, dict):
                entries.append({"path": name, "type": "tree", "sha": self.tree(child)})
            else:
                sha = hashlib.sha1(b"blob:" + child.encode()).hexdigest()
                self.blobs[sha] = child
                entries.append({"path": name, "type": "blob", "sha": sha})
        sha = hashlib.sha1(json.dumps(entries, sort_keys=True).encode()).hexdigest()
        self.trees[sha] = entries
        return sha

    def commit(self, sha, files):
        root = {}
        for path, content in files.items():
            node = root
            parts = path.split("/")
            for part in parts[:-1]:
                node = node.setdefault(part, {})
            node[parts[-1]] = content
        self.commits[sha] = self.tree(root)

    def forge_run(self, run_id, head, *, jobs=None, toolchain=None, attempt=1, event="workflow_dispatch"):
        self.runs.append({"id": run_id, "run_attempt": attempt, "head_sha": head, "path": forge.FORGE_CI,
                          "status": "completed", "conclusion": "success", "event": event,
                          "repository": {"full_name": REPO, "id": 1}, "head_repository": {"full_name": REPO, "id": 1}})
        self.jobs[(run_id, attempt)] = [{"name": name, "conclusion": conclusion}
                                        for name, conclusion in (jobs or {"backend": "success",
                                                                          "frontend": "success"}).items()]
        recorded = toolchain if toolchain is not None else {
            "backend": {"python": "Python 3.12.12", "node": "v22.21.1"},
            "frontend": {"python": "Python 3.12.3", "node": "v22.21.1"}}
        items = []
        for job, value in recorded.items():
            artifact_id = run_id * 10 + len(items)
            items.append({"id": artifact_id, "name": forge.toolchain_artifact_name(job, attempt),
                          "digest": "sha256:" + "7" * 64})
            self.downloads[artifact_id] = json.dumps(value).encode()
        self.artifacts[run_id] = items

    def api(self, endpoint, *, pages=False):
        prefix = f"repos/{REPO}/"
        assert endpoint.startswith(prefix), endpoint
        path = endpoint[len(prefix):]
        self.calls.append(path)
        if path in self.extra:
            return copy.deepcopy(self.extra[path])
        if match := re.fullmatch(r"git/commits/([0-9a-f]{40})", path):
            return {"tree": {"sha": self.commits[match.group(1)]}}
        if match := re.fullmatch(r"git/trees/([0-9a-f]{40})", path):
            return {"tree": copy.deepcopy(self.trees[match.group(1)]), "truncated": False}
        if match := re.fullmatch(r"git/blobs/([0-9a-f]{40})", path):
            return {"encoding": "base64", "content": base64.b64encode(self.blobs[match.group(1)].encode()).decode()}
        if path.startswith("actions/workflows/forge.yml/runs?"):
            self.listings.append(path)
            listed = [run for run in self.runs if not self.hidden.get(run["id"])]
            self.hidden = {run_id: count - 1 for run_id, count in self.hidden.items() if count > 1}
            return {"workflow_runs": copy.deepcopy(listed)}
        if match := re.fullmatch(r"actions/runs/(\d+)/attempts/(\d+)/jobs\?per_page=100", path):
            return {"jobs": copy.deepcopy(self.jobs[(int(match.group(1)), int(match.group(2)))])}
        if match := re.fullmatch(r"actions/runs/(\d+)/artifacts\?per_page=100", path):
            assert pages
            return [{"artifacts": copy.deepcopy(self.artifacts.get(int(match.group(1)), []))}]
        raise AssertionError(f"unexpected API call: {endpoint}")

    def download_artifact(self, identity, run_info, directory):
        path = directory / f"artifact-{identity['id']}.zip"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr(forge.TOOLCHAIN_FILE, self.downloads[identity["id"]])
        return path

    @contextlib.contextmanager
    def installed(self):
        with mock.patch.object(forge.rc, "api", self.api), \
                mock.patch.object(forge.rc, "download_artifact", self.download_artifact), environment():
            yield


class EvidenceTests(unittest.TestCase):
    def github(self, release_files=None, run_files=None, **run):
        github = FakeGitHub()
        github.commit(SHA, release_files or BASE_TREE)
        github.commit(OTHER, run_files or BASE_TREE)
        github.forge_run(FORGE_RUN, OTHER, **run)
        return github

    def evidence(self, github):
        with fixture_directory() as directory, github.installed(), settling_clock():
            return forge.forge_ci_evidence(SHA, directory)

    def test_a_run_of_identical_forge_inputs_is_reused(self):
        changed = {**BASE_TREE, "crates/meerkat-meta/src/lib.rs": "other rust", ".github/workflows/ci.yml": "x"}
        record = self.evidence(self.github(run_files=changed))
        self.assertEqual((record["run_id"], record["head_sha"]), (FORGE_RUN, OTHER))
        self.assertEqual(set(record["tuple"]["trees"]), set(forge.EVIDENCE_TREES))
        self.assertEqual(set(record["tuple"]["files"]), {
            *forge.EVIDENCE_FILES, "installer/requirements-guide-sdk.txt", "installer/requirements-standalone.txt",
            "installer/vendor/guide/meerkat_meta-0.9.2-py3-none-any.whl"})
        self.assertEqual(record["toolchain"]["declared"], {"python": "3.12", "node": "22"})
        with environment():
            forge.validate_forge_ci(record)

    def test_a_run_of_the_release_commit_itself_is_preferred(self):
        github = self.github()
        github.forge_run(FORGE_RUN - 5, SHA)
        self.assertEqual(self.evidence(github)["head_sha"], SHA)

    def test_each_changed_tuple_member_refuses_reuse(self):
        members = {
            "trees:installer": ("installer/launchkit/server.py", "changed server"),
            "trees:console": ("console/src/app.ts", "changed console"),
            "trees:packages/console-core": ("packages/console-core/index.ts", "changed core"),
            "trees:packages/console-components": ("packages/console-components/index.ts", "changed components"),
            "trees:sdk/python": ("sdk/python/meerkat_meta/__init__.py", "changed sdk"),
            f"files:{forge.FORGE_CI}": (forge.FORGE_CI, BASE_TREE[forge.FORGE_CI] + "# changed\n"),
            "files:console/package-lock.json": ("console/package-lock.json", "changed console lock"),
            "files:installer/web/package-lock.json": ("installer/web/package-lock.json", "changed web lock"),
            "files:installer/requirements-standalone.txt": ("installer/requirements-standalone.txt", "changed"),
            "files:installer/vendor/guide/meerkat_meta-0.9.2-py3-none-any.whl": (
                "installer/vendor/guide/meerkat_meta-0.9.2-py3-none-any.whl", "changed wheel"),
        }
        for member, (path, content) in members.items():
            with self.subTest(member):
                github = self.github(run_files={**BASE_TREE, path: content})
                with self.assertRaises(forge.ProtocolError) as refusal:
                    self.evidence(github)
                self.assertIn(member, str(refusal.exception))
                self.assertIn("Dispatch forge.yml", str(refusal.exception))

    def test_a_run_that_tested_another_gateway_source_is_not_reused(self):
        """installer/tests/test_gateway_shutdown_horizon.py pins the shutdown
        horizon the gateway source advertises, so the bound run must have
        tested the candidate's own copy of that source."""
        path = "crates/meerkat-meta/src/rpc_gateway.rs"
        github = self.github(run_files={**BASE_TREE, path: "another horizon"})
        with self.assertRaises(forge.ProtocolError) as refusal:
            self.evidence(github)
        self.assertIn(f"files:{path}", str(refusal.exception))
        self.assertIn("Dispatch forge.yml", str(refusal.exception))

    def test_a_new_requirements_file_or_wheel_refuses_reuse(self):
        for path in ("installer/requirements-extra.txt", "installer/vendor/guide/extra-1.0-py3-none-any.whl"):
            with self.subTest(path), self.assertRaisesRegex(forge.ProtocolError, re.escape(f"files:{path}")):
                self.evidence(self.github(release_files={**BASE_TREE, path: "new"}))

    def test_a_run_without_both_green_jobs_or_a_fitting_toolchain_is_not_evidence(self):
        cases = {
            "backend failed": {"jobs": {"backend": "failure", "frontend": "success"}},
            "frontend missing": {"jobs": {"backend": "success"}},
            "no toolchain record": {"toolchain": {"backend": {"python": "Python 3.12.12", "node": "v22.21.1"}}},
            "other Python": {"toolchain": {"backend": {"python": "Python 3.13.1", "node": "v22.21.1"},
                                           "frontend": {"python": "Python 3.12.3", "node": "v22.21.1"}}},
            "other Node": {"toolchain": {"backend": {"python": "Python 3.12.12", "node": "v22.21.1"},
                                         "frontend": {"python": "Python 3.12.3", "node": "v20.19.0"}}},
        }
        for label, run in cases.items():
            with self.subTest(label), self.assertRaisesRegex(forge.ProtocolError, "no green dispatched Forge run"):
                self.evidence(self.github(**run))

    def test_only_dispatched_runs_are_listed_and_counted(self):
        github = self.github()
        self.evidence(github)
        self.assertEqual(len(github.listings), 1)
        self.assertIn("event=workflow_dispatch", github.listings[0])
        self.assertIn("status=success", github.listings[0])

    def test_a_pull_request_run_is_never_evidence(self):
        """It tested refs/pull/N/merge, not its head_sha, so even a run whose
        head has the release commit's exact Forge inputs is refused."""
        for head in (OTHER, SHA):
            with self.subTest(head=head):
                github = FakeGitHub()
                github.commit(SHA, BASE_TREE)
                github.commit(OTHER, BASE_TREE)
                github.forge_run(FORGE_RUN, head, event="pull_request")
                with self.assertRaisesRegex(forge.ProtocolError, "no green dispatched Forge run"):
                    self.evidence(github)

    def test_publication_rechecks_the_bound_run(self):
        github = self.github()
        record = self.evidence(github)
        manifest = {"source_sha": SHA, "forge_ci": record}
        github.extra[f"actions/runs/{FORGE_RUN}"] = github.runs[0]
        with github.installed():
            forge.recheck_forge_ci(manifest)
        github.commit(SHA, {**BASE_TREE, "sdk/python/meerkat_meta/__init__.py": "changed"})
        with github.installed(), self.assertRaisesRegex(forge.ProtocolError, "no longer tested"):
            forge.recheck_forge_ci(manifest)
        github.commit(SHA, BASE_TREE)
        for change in ({"conclusion": "failure"}, {"event": "pull_request"}):
            github.extra[f"actions/runs/{FORGE_RUN}"] = {**github.runs[0], **change}
            with self.subTest(change), github.installed(), \
                    self.assertRaisesRegex(forge.ProtocolError, "no longer green"):
                forge.recheck_forge_ci(manifest)


def artifact_zip(path, members):
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in members.items():
            archive.writestr(name, data)
    return path


def package_members(target="linux-x86_64", data=b"package bytes"):
    name = forge.package_name(VERSION, target)
    return {name: data, name + ".sha256": f"{sha256(data)}  {name}\n".encode(),
            forge.provenance_name(VERSION, target): b"bundle"}


class CollectTests(unittest.TestCase):
    def collect(self, directory, head, *, ref=forge.MAIN, ci=True, members=None, artifacts=None, statuses=(),
                unlisted=0, reads=None):
        """statuses: what the candidate run reports before in_progress;
        unlisted: how many artifact listings come back empty first."""
        packages = {11: members or package_members()}
        run = {"id": RUN, "run_attempt": ATTEMPT, "head_sha": head, "head_branch": ref.removeprefix("refs/heads/"),
               "event": "workflow_dispatch", "path": forge.WORKFLOW, "status": "in_progress",
               "repository": {"full_name": REPO, "id": 1}, "head_repository": {"full_name": REPO, "id": 1}}
        listed = artifacts if artifacts is not None else [
            {"id": 11, "name": forge.package_artifact_name("linux-x86_64", ATTEMPT), "digest": "sha256:" + "e" * 64}]
        provenance, runs, statuses = [], [], list(statuses)
        reads = [] if reads is None else reads

        def api(endpoint, *, pages=False):
            reads.append(endpoint.removeprefix(f"repos/{REPO}/"))
            if endpoint.endswith(f"actions/runs/{RUN}"):
                return {**run, "status": statuses.pop(0) if statuses else "in_progress"}
            if endpoint.endswith(f"actions/runs/{RUN}/artifacts?per_page=100"):
                if reads.count(reads[-1]) <= unlisted:
                    return [{"artifacts": []}]
                return [{"artifacts": listed}]
            raise AssertionError(endpoint)

        def download(identity, run_info, work):
            return artifact_zip(work / f"artifact-{identity['id']}.zip", packages[identity["id"]])

        env = {"RELEASE_SHA": head, "RELEASE_VERSION": VERSION, "RELEASE_TAG": TAG, "GITHUB_SHA": head,
               "GITHUB_WORKFLOW_SHA": head, "GITHUB_REF": ref, "GITHUB_RUN_ID": str(RUN),
               "GITHUB_RUN_ATTEMPT": str(ATTEMPT), "CI_RUN_ID": str(CI_RUN) if ci else "",
               "CI_RUN_ATTEMPT": "1" if ci else ""}
        with environment(**env), working_directory(directory), mock.patch.object(forge.rc, "api", api), \
                mock.patch.object(forge.rc, "download_artifact", download), \
                mock.patch.object(forge.rc, "verify_provenance", lambda *a, **k: provenance.append((a, k))), \
                mock.patch.object(forge.rc, "verify_run", lambda *a, **k: runs.append((a, k))), \
                mock.patch.object(forge, "forge_ci_evidence", lambda sha, work: evidence_record(sha)), \
                contextlib.redirect_stdout(io.StringIO()), settling_clock():
            forge.collect(forge.parse_args(["collect", "--output-dir", str(directory / "candidate")]))
        return json.loads((directory / "candidate" / forge.MANIFEST).read_text()), provenance, runs

    def test_a_main_candidate_binds_packages_ci_and_forge_evidence(self):
        with fixture_directory() as directory:
            head = git_fixture(directory)
            manifest, provenance, runs = self.collect(directory, head)
            with environment():
                forge.validate_manifest(manifest)
            self.assertEqual((manifest["source_sha"], manifest["tag"], manifest["ci"]),
                             (head, TAG, {"run_id": CI_RUN, "run_attempt": 1}))
            self.assertEqual(manifest["forge_ci"]["head_sha"], head)
            self.assertEqual(manifest["packages"][0]["sha256"], sha256(b"package bytes"))
            self.assertEqual(manifest["inputs_sha256"], sha256((directory / forge.INPUTS).read_bytes()))
            self.assertEqual(runs, [((CI_RUN, 1, head), {"ci": True})])
            (args, kwargs), = provenance
            self.assertEqual((args[0].name, args[1].name, args[2:], kwargs),
                             (forge.package_name(VERSION, "linux-x86_64"), forge.provenance_name(VERSION, "linux-x86_64"),
                              (head, RUN, ATTEMPT), {"ref": forge.MAIN}))

    def test_a_branch_dry_run_binds_no_ci_and_verifies_against_its_branch(self):
        with fixture_directory() as directory:
            head = git_fixture(directory)
            manifest, provenance, runs = self.collect(directory, head, ref="refs/heads/ci/dry", ci=False)
            self.assertIsNone(manifest["ci"])
            self.assertEqual(runs, [])
            self.assertEqual(provenance[0][1], {"ref": "refs/heads/ci/dry"})

    def test_collect_refusals(self):
        bad_checksum = package_members()
        bad_checksum[forge.package_name(VERSION, "linux-x86_64") + ".sha256"] = b"0" * 64 + b"  x\n"
        cases = {
            "main without CI": ({"ci": False}, "binds its green exact-main CI run"),
            "branch with CI": ({"ref": "refs/heads/x", "ci": True}, "binds no CI run"),
            "missing artifact": ({"artifacts": []}, "missing package artifact"),
            "checksum file": ({"members": bad_checksum}, "checksum file mismatch"),
        }
        for label, (options, message) in cases.items():
            with self.subTest(label), fixture_directory() as directory:
                head = git_fixture(directory)
                with self.assertRaisesRegex(forge.ProtocolError, message):
                    self.collect(directory, head, **options)

    def test_collect_re_reads_its_run_until_it_reports_in_progress(self):
        """GitHub reported the running candidate as queued for a while."""
        with fixture_directory() as directory:
            head = git_fixture(directory)
            reads = []
            manifest, _, _ = self.collect(directory, head, statuses=("queued", "waiting"), reads=reads)
            self.assertEqual(manifest["run_id"], RUN)
            self.assertEqual(reads, [f"actions/runs/{RUN}"] * 3 + [f"actions/runs/{RUN}/artifacts?per_page=100"])

    def test_collect_re_reads_the_artifact_listing_until_it_names_every_package(self):
        """GitHub listed none of the finished build's artifacts for a while."""
        with fixture_directory() as directory:
            head = git_fixture(directory)
            reads = []
            manifest, _, _ = self.collect(directory, head, unlisted=2, reads=reads)
            self.assertEqual(manifest["packages"][0]["artifact"]["id"], 11)
            self.assertEqual(reads, [f"actions/runs/{RUN}"] + [f"actions/runs/{RUN}/artifacts?per_page=100"] * 3)


THIRD = "c" * 40
CI_LISTING = f"actions/workflows/ci.yml/runs?branch=main&event=push&head_sha={SHA}&per_page=100"


def ci_run(run_id=CI_RUN, status="completed", conclusion="success", **changes):
    return {"id": run_id, "run_attempt": 1, "head_sha": SHA, "head_branch": "main", "event": "push",
            "status": status, "conclusion": conclusion, **changes}


def candidate_run(status="in_progress", **changes):
    return {"id": RUN, "run_attempt": ATTEMPT, "head_sha": SHA, "head_branch": "main", "event": "workflow_dispatch",
            "path": forge.WORKFLOW, "status": status, "repository": {"full_name": REPO, "id": 1},
            "head_repository": {"full_name": REPO, "id": 1}, **changes}


def package_artifact(target="linux-x86_64", artifact_id=11):
    return {"id": artifact_id, "name": forge.package_artifact_name(target, ATTEMPT), "digest": "sha256:" + "e" * 64}


class RereadTests(unittest.TestCase):
    """GitHub's run and artifact listings are eventually consistent. A
    candidate read that misses is re-read REREAD_DELAY seconds later, for at
    most REREAD_ATTEMPTS reads, each miss logged; the last refuses with the
    text a single read refused with. A refusal no read can change is
    immediate."""

    def outcome(self, call, *responses, **env):
        """call() against an rc.api that answers each read with the next
        response, the last one repeating, and raises an exception response."""
        reads = []

        def api(endpoint, *, pages=False):
            reads.append(endpoint.removeprefix(f"repos/{REPO}/"))
            response = responses[min(len(reads), len(responses)) - 1]
            if isinstance(response, Exception):
                raise response
            return copy.deepcopy(response)

        with environment(**env), mock.patch.object(forge.rc, "api", api), settling_clock() as (sleeps, log):
            try:
                result = call()
            except forge.ProtocolError as error:
                result = error
        return result, reads, sleeps, log.getvalue().splitlines()

    def test_the_budget_is_forty_reads_fifteen_seconds_apart(self):
        self.assertEqual((forge.REREAD_ATTEMPTS, forge.REREAD_DELAY), (40, 15))

    def test_a_miss_is_re_read_and_logged_until_a_read_settles(self):
        read = mock.Mock(side_effect=[*(forge.Miss(f"not listed yet ({n})", "never listed") for n in (1, 2, 3)),
                                      "settled"])
        with settling_clock() as (sleeps, log):
            self.assertEqual(forge.reread("label", "repos/o/r/path?x=1", read), "settled")
        self.assertEqual((read.call_count, sleeps), (4, [15, 15, 15]))
        self.assertEqual(log.getvalue().splitlines(), [
            f"label: repos/o/r/path?x=1: read {n}/40 missed at {15 * (n - 1)} s: not listed yet ({n})"
            for n in (1, 2, 3)])

    def test_a_read_that_never_settles_refuses_after_forty_reads(self):
        read = mock.Mock(side_effect=forge.Miss("not listed", "never listed"))
        with settling_clock() as (sleeps, log), \
                self.assertRaisesRegex(forge.ProtocolError, r"^never listed \(after 40 reads over 585 s\)$"):
            forge.reread("label", "path", read)
        lines = log.getvalue().splitlines()
        self.assertEqual((read.call_count, sleeps, len(lines)), (40, [15] * 39, 40))
        self.assertEqual(lines[-1], "label: path: read 40/40 missed at 585 s: not listed")

    def test_any_other_refusal_is_never_re_read(self):
        for text in ("run identity/attempt changed", "gh api failed: gh: Not Found (HTTP 404)",
                     "gh api failed: gh: Resource not accessible by integration (HTTP 403)"):
            read = mock.Mock(side_effect=forge.ProtocolError(text))
            with self.subTest(text), settling_clock() as (sleeps, log), \
                    self.assertRaisesRegex(forge.ProtocolError, f"^{re.escape(text)}$"):
                forge.reread("label", "path", read)
            self.assertEqual((read.call_count, sleeps, log.getvalue()), (1, [], ""))

    def test_a_transient_api_failure_is_a_miss(self):
        failures = ("gh api failed: gh: HTTP 502", "gh api failed: gh: Server Error (HTTP 500)",
                    "gh api failed: error connecting to api.github.com\n"
                    "check your internet connection or https://githubstatus.com",
                    'gh api failed: Get "https://api.github.com/x": read tcp 10.0.0.1:443: connection reset by peer',
                    'gh api failed: Get "https://api.github.com/x": net/http: TLS handshake timeout',
                    'gh api failed: Get "https://api.github.com/x": unexpected EOF')
        read = mock.Mock(side_effect=[*map(forge.ProtocolError, failures), "settled"])
        with settling_clock() as (sleeps, log):
            self.assertEqual(forge.reread("label", "path", read), "settled")
        lines = log.getvalue().splitlines()
        self.assertEqual((read.call_count, sleeps, len(lines)), (7, [15] * 6, 6))
        self.assertEqual(lines[2], "label: path: read 3/40 missed at 30 s: transient API failure: gh api failed: "
                                   "error connecting to api.github.com check your internet connection or "
                                   "https://githubstatus.com")
        with settling_clock(), self.assertRaisesRegex(
                forge.ProtocolError, r"^gh api failed: gh: HTTP 503 \(after 40 reads over 585 s\)$"):
            forge.reread("label", "path", mock.Mock(side_effect=forge.ProtocolError("gh api failed: gh: HTTP 503")))

    def test_the_classification_matches_the_failure_text_of_rc_api(self):
        results = [subprocess.CompletedProcess(["gh"], 1, b"", b"gh: HTTP 502\n"),
                   subprocess.CompletedProcess(["gh"], 0, b'{"ok": true}', b"")]
        with mock.patch.object(forge.rc.subprocess, "run", side_effect=results) as run, \
                settling_clock() as (sleeps, log):
            self.assertEqual(forge.reread("label", "path", lambda: forge.rc.api("repos/o/r/x")), {"ok": True})
        self.assertEqual((run.call_count, sleeps), (2, [15]))
        self.assertIn("missed at 0 s: transient API failure: gh api failed: gh: HTTP 502", log.getvalue())

    def ci_green(self, *listings):
        """ci-green against one ci.yml listing per read, the last repeating."""
        verified = []
        with fixture_directory() as directory, contextlib.redirect_stdout(io.StringIO()), \
                mock.patch.object(forge.rc, "verify_run", lambda *a, **k: verified.append((a, k))):
            result, reads, sleeps, log = self.outcome(
                forge.ci_green, *({"workflow_runs": item} if isinstance(item, list) else item for item in listings),
                RELEASE_SHA=SHA, GITHUB_OUTPUT=str(directory / "outputs"), GH_TOKEN="ghs_never_logged")
            if not isinstance(result, Exception):
                result = read_outputs(directory / "outputs")
        self.assertEqual(set(reads), {CI_LISTING})
        self.assertNotIn("ghs_never_logged", "\n".join(log))
        return result, len(reads), sleeps, log, verified

    def test_ci_green_re_reads_until_the_exact_main_run_is_listed(self):
        result, reads, sleeps, log, verified = self.ci_green([], [ci_run(head_branch="feature")], [ci_run()])
        self.assertEqual((result, reads, sleeps), ({"run_id": str(CI_RUN), "run_attempt": "1"}, 3, [15, 15]))
        self.assertEqual(verified, [((CI_RUN, 1, SHA), {"ci": True})])
        self.assertEqual(log, [f"exact-main CI: repos/{REPO}/{CI_LISTING}: read {n}/40 missed at {15 * (n - 1)} s: "
                               "no exact-main ci.yml run listed" for n in (1, 2)])

    def test_ci_green_re_reads_while_no_listed_run_is_green_yet(self):
        red = ci_run(CI_RUN - 1, conclusion="failure")
        result, reads, sleeps, log, verified = self.ci_green(
            [ci_run(status="queued", conclusion=None)], [ci_run(status="in_progress", conclusion=None), red],
            [ci_run(), red])
        self.assertEqual((result, reads, sleeps), ({"run_id": str(CI_RUN), "run_attempt": "1"}, 3, [15, 15]))
        self.assertTrue(log[0].endswith(f"missed at 0 s: no listed exact-main run is green yet: {CI_RUN}/queued/None"))
        self.assertTrue(log[1].endswith(f": {CI_RUN}/in_progress/None, {CI_RUN - 1}/completed/failure"))
        self.assertEqual(verified, [((CI_RUN, 1, SHA), {"ci": True})])

    def test_ci_green_refuses_at_once_when_every_listed_run_finished_without_success(self):
        result, reads, sleeps, log, verified = self.ci_green(
            [ci_run(conclusion="failure"), ci_run(CI_RUN + 1, conclusion="cancelled")], [ci_run()])
        self.assertEqual(str(result), f"exact-main CI for {SHA} is not green: "
                                      f"{CI_RUN}/completed/failure, {CI_RUN + 1}/completed/cancelled")
        self.assertEqual((reads, sleeps, log, verified), (1, [], [], []))

    def test_ci_green_refuses_after_the_budget_with_the_text_of_one_read(self):
        for listing, refusal in (
                ([], f"no exact-main CI run exists for {SHA}; push it to main and let ci.yml finish"),
                ([ci_run(status="in_progress", conclusion=None)],
                 f"exact-main CI for {SHA} is not green: {CI_RUN}/in_progress/None")):
            with self.subTest(refusal):
                result, reads, sleeps, log, verified = self.ci_green(listing)
                self.assertEqual(str(result), f"{refusal} (after 40 reads over 585 s)")
                self.assertEqual((reads, sleeps, len(log), verified), (40, [15] * 39, 40, []))

    def test_ci_green_reads_through_a_transient_failure_but_not_a_refusal(self):
        result, reads, sleeps, _, _ = self.ci_green(forge.ProtocolError("gh api failed: gh: HTTP 502"), [ci_run()])
        self.assertEqual((result, reads, sleeps), ({"run_id": str(CI_RUN), "run_attempt": "1"}, 2, [15]))
        result, reads, sleeps, log, _ = self.ci_green(
            forge.ProtocolError("gh api failed: gh: Not Found (HTTP 404)"), [ci_run()])
        self.assertEqual((str(result), reads, sleeps, log), ("gh api failed: gh: Not Found (HTTP 404)", 1, [], []))

    def current_run(self, *values):
        return self.outcome(lambda: forge.current_run(RUN, ATTEMPT, SHA, forge.MAIN), *values)

    def test_the_candidate_run_is_re_read_until_it_reports_in_progress(self):
        result, reads, sleeps, log = self.current_run(candidate_run("queued"), candidate_run("pending"),
                                                      candidate_run())
        self.assertEqual((result, reads, sleeps), (candidate_run(), [f"actions/runs/{RUN}"] * 3, [15, 15]))
        self.assertEqual(log, [f"candidate run: repos/{REPO}/actions/runs/{RUN}: read 1/40 missed at 0 s: "
                               "status=queued",
                               f"candidate run: repos/{REPO}/actions/runs/{RUN}: read 2/40 missed at 15 s: "
                               "status=pending"])

    def test_a_candidate_run_never_in_progress_refuses_after_the_budget(self):
        result, reads, sleeps, log = self.current_run(candidate_run("queued"))
        self.assertEqual(str(result), "collection requires its in-progress candidate run (after 40 reads over 585 s)")
        self.assertEqual((len(reads), sleeps, len(log)), (40, [15] * 39, 40))

    def test_a_mismatched_candidate_run_refuses_at_once(self):
        """Even while it is not yet reported in progress."""
        cases = (({"run_attempt": 2}, "run identity/attempt changed"),
                 ({"head_repository": {"full_name": "someone/meerkat-meta", "id": 2}}, "run repository mismatch"),
                 ({"head_sha": OTHER}, "run source/workflow/event/ref mismatch"),
                 ({"head_branch": "feature"}, "run source/workflow/event/ref mismatch"),
                 ({"path": ".github/workflows/release.yml"}, "run source/workflow/event/ref mismatch"))
        for change, message in cases:
            with self.subTest(change):
                result, reads, sleeps, log = self.current_run(candidate_run("queued", **change), candidate_run())
                self.assertEqual((str(result), len(reads), sleeps, log), (message, 1, [], []))

    def evidence(self, github):
        with fixture_directory() as directory, github.installed(), settling_clock() as (sleeps, log):
            try:
                result = forge.forge_ci_evidence(SHA, directory)
            except forge.ProtocolError as error:
                result = error
        return result, sleeps, log.getvalue().splitlines()

    def evidence_github(self):
        """The matching Forge run, and an older one of other Forge inputs."""
        github = FakeGitHub()
        github.commit(SHA, BASE_TREE)
        github.commit(OTHER, BASE_TREE)
        github.commit(THIRD, {**BASE_TREE, "console/src/app.ts": "older console"})
        github.forge_run(FORGE_RUN, OTHER)
        github.forge_run(FORGE_RUN - 1, THIRD)
        return github

    def test_forge_evidence_re_lists_until_the_matching_run_is_listed(self):
        github = self.evidence_github()
        github.hidden = {FORGE_RUN: 3}
        result, sleeps, log = self.evidence(github)
        self.assertEqual((result["run_id"], result["head_sha"]), (FORGE_RUN, OTHER))
        self.assertEqual((len(github.listings), sleeps, len(log)), (4, [15, 15, 15], 3))
        self.assertTrue(log[0].startswith(
            f"Forge evidence: repos/{REPO}/{github.listings[0]}: read 1/40 missed at 0 s"))
        self.assertTrue(all(line.endswith(": none of 1 listed green dispatched run(s) tested these Forge inputs")
                            for line in log))
        # One object cache spans the listings: no git object is read twice.
        objects = [call for call in github.calls if call.startswith("git/")]
        self.assertEqual(len(objects), len(set(objects)))
        self.assertEqual(objects.count(f"git/commits/{THIRD}"), 1)

    def test_forge_evidence_never_listed_refuses_after_the_budget_with_the_text_of_one_read(self):
        github = self.evidence_github()
        github.hidden = {FORGE_RUN: 1000}
        result, sleeps, log = self.evidence(github)
        self.assertEqual(str(result), "no green dispatched Forge run (backend and frontend) tested the Forge inputs at "
                                      f"{SHA}; the newest green run {FORGE_RUN - 1} differs in trees:console. "
                                      "Dispatch forge.yml for this commit, then rerun the candidate "
                                      "(after 40 reads over 585 s)")
        self.assertEqual((len(github.listings), sleeps, len(log)), (40, [15] * 39, 40))

    def test_a_run_evaluated_on_every_listing_downloads_into_a_folder_per_listing(self):
        github = FakeGitHub()
        github.commit(SHA, BASE_TREE)
        github.commit(OTHER, BASE_TREE)
        github.forge_run(FORGE_RUN, OTHER, toolchain={"backend": {"python": "Python 3.13.1", "node": "v22.21.1"},
                                                      "frontend": {"python": "Python 3.12.3", "node": "v22.21.1"}})
        github.forge_run(FORGE_RUN + 1, SHA)
        github.hidden = {FORGE_RUN + 1: 2}
        result, sleeps, _ = self.evidence(github)
        self.assertEqual((result["run_id"], result["head_sha"], sleeps), (FORGE_RUN + 1, SHA, [15, 15]))

    def artifacts(self, *listings, names=None):
        names = names or [forge.package_artifact_name(target, ATTEMPT) for target in forge.TARGETS]
        return self.outcome(lambda: forge.candidate_artifacts(RUN, names),
                            *([{"artifacts": listing}] for listing in listings))

    def test_the_artifact_listing_is_re_read_until_every_package_is_listed(self):
        linux, macos = package_artifact("linux-x86_64", 11), package_artifact("macos-arm64", 12)
        result, reads, sleeps, log = self.artifacts([], [linux], [linux, macos])
        self.assertEqual((result, sleeps), ({linux["name"]: linux, macos["name"]: macos}, [15, 15]))
        self.assertEqual(reads, [f"actions/runs/{RUN}/artifacts?per_page=100"] * 3)
        self.assertEqual(log, [
            f"candidate artifacts: repos/{REPO}/actions/runs/{RUN}/artifacts?per_page=100: read 1/40 missed at 0 s: "
            f"{linux['name']}, {macos['name']} not listed",
            f"candidate artifacts: repos/{REPO}/actions/runs/{RUN}/artifacts?per_page=100: read 2/40 missed at 15 s: "
            f"{macos['name']} not listed"])

    def test_a_package_never_listed_refuses_after_the_budget_with_the_text_of_one_read(self):
        result, reads, sleeps, log = self.artifacts([package_artifact("macos-arm64", 12)])
        self.assertEqual(str(result),
                         "missing package artifact forge-package-linux-x86_64-1 (after 40 reads over 585 s)")
        self.assertEqual((len(reads), sleeps, len(log)), (40, [15] * 39, 40))

    def test_duplicate_artifact_names_refuse_at_once(self):
        result, reads, sleeps, log = self.artifacts([package_artifact(), package_artifact()], [package_artifact()],
                                                    names=[package_artifact()["name"]])
        self.assertEqual((str(result), len(reads), sleeps, log),
                         ("duplicate artifact names in the candidate run", 1, [], []))


class PublishTests(unittest.TestCase):
    def publish(self, directory, *, manifest=None, existing=None, latest_after="v0.9.3", selection_sha=None):
        manifest = manifest or manifest_value()
        manifest_bytes = json.dumps(manifest).encode()
        selected = selection_value(selection_sha or sha256(manifest_bytes))
        artifacts = {90: {forge.MANIFEST: manifest_bytes, forge.MANIFEST_PROVENANCE: b"manifest bundle"},
                     11: package_members()}
        calls, uploads, latest = [], [], ["v0.9.3", latest_after]
        state = {"release": existing}

        def api(endpoint, *, pages=False):
            if endpoint.endswith(f"actions/runs/{RUN}"):
                return {"head_sha": SHA}
            if endpoint.endswith("releases/latest"):
                return {"tag_name": latest.pop(0)}
            raise AssertionError(endpoint)

        def run(argv, **_):
            calls.append(argv)
            if argv[:3] == ["gh", "release", "create"]:
                state["release"] = {"id": 700, "draft": True, "prerelease": True}
            elif argv[:2] == ["gh", "api"]:
                state["release"] = {"id": 700, "draft": False, "prerelease": True}
            return b""

        def download(identity, run_info, work):
            return artifact_zip(work / f"artifact-{identity['id']}.zip", artifacts[identity["id"]])

        inventory = {} if existing is None else existing.get("inventory", {})
        patches = [
            mock.patch.object(forge.rc, "api", api), mock.patch.object(forge.rc, "run", run),
            mock.patch.object(forge.rc, "verify_run", lambda *a, **k: {"id": RUN}),
            mock.patch.object(forge.rc, "download_artifact", download),
            mock.patch.object(forge.rc, "verify_provenance", lambda *a, **k: None),
            mock.patch.object(forge.rc, "verify_remote_tag", lambda tag, sha: calls.append(["tag", tag, sha])),
            mock.patch.object(forge.rc, "release_for_tag", lambda tag: state["release"]),
            mock.patch.object(forge.rc, "newly_created_release_for_tag", lambda tag: state["release"]),
            mock.patch.object(forge.rc, "release_inventory", lambda release: dict(inventory) if release is existing else {}),
            mock.patch.object(forge.rc, "download_release_inventory",
                              lambda inv, folder: (folder.mkdir(), [(folder / n).write_bytes(existing["bytes"][n])
                                                                    for n in inv])),
            mock.patch.object(forge.rc, "upload", lambda tag, path: uploads.append(path.name)),
            mock.patch.object(forge.rc, "readback", lambda *a: calls.append(["readback"])),
            mock.patch.object(forge, "recheck_forge_ci", lambda value: calls.append(["forge_ci"])),
        ]
        env = {"RELEASE_MODE": "publish", "ARTIFACT_SELECTION": json.dumps(selected)}
        with contextlib.ExitStack() as stack:
            for patch in patches:
                stack.enter_context(patch)
            stack.enter_context(environment(**env))
            stack.enter_context(working_directory(directory))
            stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            forge.publish(None)
        return calls, uploads

    def test_publication_is_a_bound_prerelease_that_never_becomes_latest(self):
        with fixture_directory() as directory:
            calls, uploads = self.publish(directory)
        create = next(call for call in calls if call[:3] == ["gh", "release", "create"])
        for flag in ("--verify-tag", "--draft", "--prerelease", "--latest=false"):
            self.assertIn(flag, create)
        self.assertEqual(create[3], TAG)
        patch = next(call for call in calls if call[:2] == ["gh", "api"])
        self.assertEqual(patch[2:5], ["--method", "PATCH", f"repos/{REPO}/releases/700"])
        self.assertEqual(patch[5:], ["-F", "draft=false", "-F", "prerelease=true", "-f", "make_latest=false"])
        self.assertEqual(uploads[0], forge.SELECTION)
        self.assertEqual(sorted(uploads), sorted([forge.SELECTION, forge.MANIFEST, forge.MANIFEST_PROVENANCE,
                                                  *package_members()]))
        self.assertIn(["forge_ci"], calls)
        self.assertEqual(calls.count(["readback"]), 2)
        self.assertLess(calls.index(create), calls.index(patch))

    def test_publication_refusals(self):
        branch = manifest_value(ref="refs/heads/ci/dry")
        cases = {
            "branch candidate": ({"manifest": branch}, "only a main candidate"),
            "selection bytes": ({"selection_sha": "0" * 64}, "differ from the accepted selection"),
            "published release": ({"existing": {"id": 5, "draft": False}}, "already published"),
            "foreign draft asset": ({"existing": {"id": 5, "draft": True, "inventory": {"other.zip": {}},
                                                  "bytes": {"other.zip": b"x"}}}, "unrecognized assets"),
            "latest moved": ({"latest_after": "v0.9.4"}, "latest release changed from v0.9.3"),
            "latest became Forge": ({"latest_after": TAG}, "not a Meta release"),
        }
        for label, (options, message) in cases.items():
            with self.subTest(label), fixture_directory() as directory, \
                    self.assertRaisesRegex(forge.ProtocolError, message):
                self.publish(directory, **options)

    def test_the_published_verify_command_pins_main_and_the_source(self):
        notes = forge.release_notes(manifest_value())
        self.assertIn(f"--signer-workflow {REPO}/{forge.WORKFLOW} --source-ref refs/heads/main "
                      f"--source-digest {SHA}`", notes)
        guide = (ROOT / "installer/RELEASING.md").read_text()
        self.assertIn("  --source-ref refs/heads/main\n", guide)
        self.assertIn("`git push origin forge-v<version>`", guide)

    def test_a_draft_holding_another_candidates_receipt_is_refused(self):
        existing = {"id": 5, "draft": True, "inventory": {forge.SELECTION: {}},
                    "bytes": {forge.SELECTION: json.dumps(selection_value("1" * 64)).encode()}}
        with fixture_directory() as directory, self.assertRaisesRegex(forge.ProtocolError, "draft asset conflict"):
            self.publish(directory, existing=existing)


class RebindingTests(unittest.TestCase):
    def test_only_this_scripts_copy_of_release_candidate_checks_forge_release(self):
        self.assertEqual(forge.rc.WORKFLOW, ".github/workflows/forge-release.yml")
        meta = load(f"release_candidate_{uuid.uuid4().hex}", ROOT / "scripts/release-candidate.py")
        self.assertEqual(meta.WORKFLOW, ".github/workflows/release.yml")
        self.assertEqual(release_tests.release_candidate.WORKFLOW, ".github/workflows/release.yml")

    def test_verify_package_files_binds_the_checksum_and_the_attestation_ref(self):
        with fixture_directory() as directory:
            folder = directory / "package"
            folder.mkdir()
            for name, data in package_members().items():
                (folder / name).write_bytes(data)
            seen = []
            with mock.patch.object(forge.rc, "verify_provenance", lambda *a, **k: seen.append((a[2:], k))):
                record = forge.verify_package_files(folder, VERSION, "linux-x86_64", SHA, RUN, ATTEMPT, "refs/heads/x")
            self.assertEqual(record, {"target": "linux-x86_64", "name": forge.package_name(VERSION, "linux-x86_64"),
                                      "size": len(b"package bytes"), "sha256": sha256(b"package bytes")})
            self.assertEqual(seen, [((SHA, RUN, ATTEMPT), {"ref": "refs/heads/x"})])
            (folder / (forge.package_name(VERSION, "linux-x86_64") + ".sha256")).write_text("0" * 64 + "  x\n")
            with mock.patch.object(forge.rc, "verify_provenance", lambda *a, **k: None), \
                    self.assertRaisesRegex(forge.ProtocolError, "checksum file mismatch"):
                forge.verify_package_files(folder, VERSION, "linux-x86_64", SHA, RUN, ATTEMPT, "refs/heads/x")


if __name__ == "__main__":
    unittest.main()
