#!/usr/bin/env python3
"""Focused contracts for Meta's CI workflows: the monorepo's
.github/workflows/meta-ci.yml (pull requests, pushes, dispatch), its
meta-installer.yml and meta-e2e-live.yml, and the workflows still kept under
meta/.github."""

import fnmatch
import importlib.util
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[1]
MONOREPO = ROOT.parent
WORKFLOWS = MONOREPO / ".github/workflows"
CI_WORKFLOW = WORKFLOWS / "meta-ci.yml"
INSTALLER_WORKFLOW = WORKFLOWS / "meta-installer.yml"
INSTALLER_RELEASE_WORKFLOW = WORKFLOWS / "meta-installer-release.yml"
E2E_LIVE_WORKFLOW = WORKFLOWS / "meta-e2e-live.yml"
META_WORKFLOWS = (CI_WORKFLOW, INSTALLER_WORKFLOW, E2E_LIVE_WORKFLOW)
# The root CI, whose toolchain, compile environment, cache settings and
# action pins meta-ci.yml shares.
ROOT_CI_WORKFLOW = WORKFLOWS / "ci.yml"
# The root CI job whose Rust setup meta-ci.yml's compile jobs copy: the root
# toolchain, the sccache bucket, mold and the registry-only rust-cache.
ROOT_RUST_JOB = "ratchets"
_CI_CHANGES_SPEC = importlib.util.spec_from_file_location("ci_changes", ROOT / "scripts/ci-changes.py")
ci_changes = importlib.util.module_from_spec(_CI_CHANGES_SPEC)
_CI_CHANGES_SPEC.loader.exec_module(ci_changes)

# The variables rust-cache hashes into its key.
HASHED_ENV = re.compile(r"(?:CARGO|CC|CFLAGS|CXX|CMAKE|RUST)\w*")
SETUP_RUST = "./.github/actions/setup-rust-ci"
# The jobs that run on pull requests only: the root CI runs their tests on
# the push (main unit, feature suite and closure-tests rows for Meta's
# packages).
PULL_REQUEST_ONLY_JOBS = {"rust-unit-build": "rust", "rust-unit": "rust", "rust-conformance": "rust",
                          "rust-integration": "integration",
                          "rust-extension-state-lib": "extension_state", "rust-voice-lib": "voice"}
PULL_REQUEST_ONLY = " && github.event_name == 'pull_request'"
# Meta's workspace packages: the only ones a Meta job names to Cargo.
META_PACKAGES = ("meerkat-meta", "meta-extension-state", "meta-store-conformance")
SETUP_SCCACHE = "./.github/actions/setup-sccache-ci"
SETUP_MOLD = "./.github/actions/setup-mold-ci"
# The step of a rust-unit leg that checks the archive it fetched before any
# test runs.
UNIT_LEG_GUARD = "Check the fetched archive and the workspace path it was built at"
# The text no run line of a Meta workflow may hold (see
# test_cargo_runs_one_package_from_the_root_wrapper).
NAMES_THE_WORKSPACE = re.compile(r"--workspace(?!-remap(\s|$))")
# The four jobs whose compile of meerkat-meta is measured: the step that
# compiles, its whole `run` text (the commands as they were, with Cargo's
# `--timings` on the compiling one and nothing else added), and what the
# report line says was built.
_BACKENDS = ("library_mode_reference", "console_acceptance_fixture", "memory_console_reference",
             "workgraph_console_reference", "mcp_apps_gateway_fixture")
_EXTENSION_STATE = "extension-state,announcement-test-faults,console-test-faults"
COMPILE_PATHS = {
    "rust-unit-build": (
        "Build the lib test archive",
        "./scripts/repo-cargo nextest archive -p meerkat-meta --lib --timings "
        '--archive-file "${RUNNER_TEMP}/nextest-meta-unit.tar.zst"',
        "meerkat-meta lib test, default features",
    ),
    "rust-extension-state-lib": (
        "Build the extension-state lib tests",
        f"./scripts/repo-cargo test -p meerkat-meta --features {_EXTENSION_STATE} --lib --timings --no-run",
        f"meerkat-meta lib test, features {_EXTENSION_STATE}, filter extension_ document_mcp_child_supply",
    ),
    "rust-voice-lib": (
        "Build the voice lib tests",
        "./scripts/repo-cargo test -p meerkat-meta --features openai-live --lib --timings --no-run",
        "meerkat-meta lib test, features openai-live, filters console_voice and gateway_openai_live_registration",
    ),
    "console-backends": (
        "Build console acceptance backends",
        "./scripts/repo-cargo build -p meerkat-meta " + " ".join(f"--example {name}" for name in _BACKENDS)
        + " --timings",
        "meerkat-meta examples " + ", ".join(_BACKENDS) + ", default features",
    ),
}
# The two lib jobs whose build is a step of its own: the step that then runs
# the tests on that build, and its whole `run` text.
LIB_TEST_STEPS = {
    "rust-extension-state-lib": (
        "Extension-state lib tests",
        f"./scripts/repo-cargo test -p meerkat-meta --features {_EXTENSION_STATE} --lib "
        "-- extension_ document_mcp_child_supply",
    ),
    "rust-voice-lib": (
        "Voice lib cases",
        "./scripts/repo-cargo test -p meerkat-meta --features openai-live --lib console_voice\n"
        "./scripts/repo-cargo test -p meerkat-meta --features openai-live --lib gateway_openai_live_registration",
    ),
}
# The one implementation of the 30-second resource readings and of the
# runner's size print, and the compile steps that run their command under it.
READINGS = "./scripts/ci-resource-readings.sh"
READINGS_SCRIPT = MONOREPO / "scripts/ci-resource-readings.sh"
WITH_READINGS = ("rust-unit-build", "rust-extension-state-lib", "rust-voice-lib")
SIZE_PRINT = "Show the runner's size before the compile"
MEASURE_BEFORE = "Compile statistics, before"
MEASURE_AFTER = "Compile statistics and timing report, after"
MEASURE_UPLOAD = "Retain the Cargo timing reports"
SWAP_PREFLIGHT = "Show the runner's size and add swap before the compile"
# What stands in for a bucket name in the rows: it must never reach a line
# the report step prints.
FIXTURE_LOCATION_NAME = "fixture-cache-location-name"


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text())


def triggers(workflow: dict) -> dict:
    # YAML 1.1 reads the bare key `on` as True.
    return workflow.get("on", workflow.get(True))


def step_run(text: str, name: str) -> str:
    """The dedented `run` script of the workflow step named `name` (the
    root's scripts/ci-release-projection-selftest.mjs reads it the same
    way)."""
    lines = text.split("\n")
    at = next(index for index, line in enumerate(lines) if line.strip() == f"- name: {name}")
    step_indent = lines[at].index("-")
    run_at = None
    for index in range(at + 1, len(lines)):
        if re.match(r"^\s*#", lines[index]):
            continue
        indent = len(lines[index]) - len(lines[index].lstrip())
        if lines[index].strip() and indent <= step_indent:
            break
        if re.match(r"^\s+run: \|\s*$", lines[index]):
            run_at = index
            break
    assert run_at is not None, f"step {name} has a run block"
    run_indent = len(lines[run_at]) - len(lines[run_at].lstrip())
    body = []
    for line in lines[run_at + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= run_indent:
            break
        body.append(line)
    indent = min(len(line) - len(line.lstrip()) for line in body if line.strip())
    return "\n".join(line[indent:] for line in body).rstrip()


def meta_test_targets() -> list[tuple[str, dict]]:
    """Meta's workspace packages' targets as Cargo reads them (`cargo
    metadata --no-deps` from the monorepo's root resolves nothing)."""
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--offline", "--format-version", "1"],
        cwd=MONOREPO, check=True, capture_output=True, text=True,
    )
    metadata = json.loads(result.stdout)
    members = set(metadata["workspace_members"])
    return [
        (package["name"], target)
        for package in metadata["packages"]
        if package["id"] in members and package["name"] in META_PACKAGES
        for target in package["targets"]
    ]


def job_block(name: str, path: Path = CI_WORKFLOW) -> str:
    """Return exactly one top-level job, bounded by the next job header."""
    lines = path.read_text().splitlines()
    start = lines.index(f"  {name}:")
    for offset, line in enumerate(lines[start + 1 :], start=start + 1):
        if line.startswith("  ") and not line.startswith("   ") and line.endswith(":"):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


def steps_of(job: dict) -> list[dict]:
    return job.get("steps") or []


def uses_of(step: dict) -> str:
    return (step.get("uses") or "").split("@", 1)[0]


def hashed(env: dict | None) -> dict:
    return {key: str(value) for key, value in (env or {}).items() if HASHED_ENV.fullmatch(key)}


def rust_jobs(workflow: dict) -> dict[str, dict]:
    """The jobs that compile Rust: those that set up sccache."""
    return {name: job for name, job in workflow["jobs"].items()
            if any(uses_of(step) == SETUP_SCCACHE for step in steps_of(job))}


def run_lines(workflow: dict):
    """(job, step, line) for every line of every run step."""
    for name, job in workflow["jobs"].items():
        for step in steps_of(job):
            for line in str(step.get("run") or "").splitlines():
                yield name, step, line.strip()


# A runner label that names its image. A -latest label moves to a new image on
# GitHub's schedule (ubuntu-latest left 24.04 on 2026-10-19), and with it the
# toolchains, glibc and preinstalled tools every job reads.
PINNED_RUNNER = re.compile(r"^(ubuntu-\d\d\.\d\d(-arm)?|macos-\d+|windows-\d{4}(-vs\d{4})?)$")
# The one runs-on expression a job may use, and the one matrix it may take the
# labels from without naming them inline: the release build legs, which
# test_the_release_build_legs_run_on_pinned_runner_images reads.
MATRIX_RUNS_ON = "${{ matrix.runs-on }}"
RELEASE_BUILD_MATRIX = "${{ fromJSON(needs.resolve_release.outputs.build_matrix) }}"


def meta_workflow_files(root: Path) -> list[Path]:
    """Meta's workflows in a tree: the ones under `root`'s .github, and when
    `root` is Meta's directory in the monorepo, the root's meta-*.yml."""
    found = sorted((root / ".github" / "workflows").glob("*.y*ml"))
    if root == ROOT:
        found += [path for path in META_WORKFLOWS if path.is_file()]
    return found


def runner_problems(root: Path) -> tuple[list[str], list[str]]:
    """Every workflow job's runner labels, read as YAML, and why any of them
    floats or cannot be read. A runs-on that is not one inline label (a block
    list, a flow list or a mapping) is refused, never skipped."""
    labels, problems = [], []
    for path in meta_workflow_files(root):
        workflow = yaml.safe_load(path.read_text()) or {}
        for name, job in (workflow.get("jobs") or {}).items():
            if not isinstance(job, dict) or "runs-on" not in job:
                continue
            where = f"{path.name} job {name}"
            value = job["runs-on"]
            if not isinstance(value, str):
                problems.append(f"{where}: runs-on is a {type(value).__name__} this check cannot read; "
                                "name one pinned label inline")
                continue
            if value == MATRIX_RUNS_ON:
                matrix = (job.get("strategy") or {}).get("matrix")
                if matrix == RELEASE_BUILD_MATRIX:
                    continue
                values = []
                if isinstance(matrix, dict):
                    axis = matrix.get("runs-on")
                    values += axis if isinstance(axis, list) else ([] if axis is None else [axis])
                    values += [entry["runs-on"] for entry in matrix.get("include") or []
                               if isinstance(entry, dict) and "runs-on" in entry]
                if not values:
                    problems.append(f"{where}: runs-on {value} names no matrix label this check can read")
            elif "${{" in value:
                problems.append(f"{where}: runs-on {value} is an expression this check cannot read")
                continue
            else:
                values = [value]
            for label in values:
                labels.append(label)
                if not isinstance(label, str) or "latest" in label or not PINNED_RUNNER.match(label):
                    problems.append(f"{where}: runs-on {label} does not name a pinned runner image")
    return labels, problems


# An image reference that names its digest.
DIGEST = re.compile(r"@sha256:[0-9a-f]{64}$")
# A `container:` or `image:` key with an inline value. The value is read
# literally: a flow mapping (`container: {image: ...}`) or an action input
# that happens to be named `image` is refused rather than parsed.
IMAGE_KEY = re.compile(r"^\s*(?:-\s*)?(container|image):\s*(\S.*?)\s*$")
# A `docker://` image reference, wherever a line names one.
DOCKER_URL = re.compile(r"docker://[^\s\"',)]+")
# A key whose value is a block scalar (`run: |`, `description: >-`).
BLOCK_SCALAR = re.compile(r"^(\s*)(-\s+)?[^\s#-][^#]*?:\s*[|>][-+0-9]*\s*$")
# A Dockerfile FROM line's image, after any --platform style flags.
FROM_LINE = re.compile(r"^\s*FROM\s+(?:--\S+\s+)*(\S+)(?:\s+AS\s+(\S+))?\s*$", re.IGNORECASE)
# Image references accepted without a digest, and why. None today: the one
# exception, the self-hosted Bazel platform's private image, left with Meta's
# Bazel files when Meta joined the monorepo's workspace.
UNPINNED_IMAGES: dict[re.Pattern, str] = {}
# The installer's launch templates and the installer test's frozen copy of
# them. A user builds their own app from these, so they are not CI or release
# images and Renovate does not manage them; their FROM lines stay as written.
# Every other tracked container build file's FROM lines must name a digest.
USER_TEMPLATE_DOCKERFILES = re.compile(r"installer/(?:tests/fixtures/[^/]+/)?launchkit/templates_[^/]+/Dockerfile")


def is_container_build_file(name: str) -> bool:
    """A Dockerfile or Containerfile, by any of the names builds use for one
    (`Dockerfile.ci`, `app.Dockerfile`, `Containerfile`)."""
    lower = name.lower()
    return lower.startswith(("dockerfile", "containerfile")) or lower.endswith((".dockerfile", ".containerfile"))


def without_comment(line: str) -> str:
    """The line before a YAML comment, outside quotes."""
    quote = None
    index = 0
    while index < len(line):
        char = line[index]
        if quote:
            if quote == '"' and char == "\\":
                index += 1
            elif char == quote:
                quote = None
        elif char in "'\"":
            quote = char
        elif char == "#" and (index == 0 or line[index - 1].isspace()):
            return line[:index]
        index += 1
    return line


def yaml_lines(text: str):
    """(number, line) for every line that is not a block scalar's text."""
    block_key_indent = -1
    for number, line in enumerate(text.split("\n"), 1):
        indent = len(line) - len(line.lstrip())
        if block_key_indent >= 0:
            if not line.strip() or indent > block_key_indent:
                continue
            block_key_indent = -1
        yield number, line
        block = BLOCK_SCALAR.match(without_comment(line).rstrip())
        if block:
            block_key_indent = len(block.group(1)) + len(block.group(2) or "")


def image_references(value: str) -> list[str] | None:
    """The images a `container:` or `image:` value names: the value itself, or
    the quoted literals of an expression that look like one; None when an
    expression names none this check can read."""
    unquoted = re.sub(r"^['\"]|['\"]$", "", value)
    if "${{" not in unquoted:
        return [unquoted]
    images = [literal for literal in re.findall(r"'([^']*)'", unquoted)
              if literal and not re.search(r"\s", literal) and re.search(r"[/:]", literal)]
    return images or None


def tracked_files(root: Path) -> list[str]:
    """The repository's tracked files below `root`, or every file of a tree
    with no git."""
    inside = subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], cwd=root,
                            capture_output=True, text=True)
    if inside.returncode == 0 and inside.stdout.strip() == "true":
        listed = subprocess.run(["git", "ls-files", "-z"], cwd=root, check=True, capture_output=True, text=True)
        return [name for name in listed.stdout.split("\0") if name]
    return sorted(str(path.relative_to(root)) for path in root.rglob("*") if path.is_file())


def image_problems(root: Path) -> tuple[list[tuple[str, str]], list[str]]:
    """Every container image Meta's CI names, with where it is named, and why
    any of them floats: a `container:` or `image:` value in a .github YAML
    file, a `docker://` reference on any line of a .github file (a `uses:`
    action or a run script), one in tools/bazel's BUILD, BUILD.bazel and .bzl
    files or .bazelrc, and the FROM lines of every tracked Dockerfile or
    Containerfile except the installer's launch templates
    (USER_TEMPLATE_DOCKERFILES). Each must carry an @sha256: digest or be
    listed in UNPINNED_IMAGES. (The root's own workflows and Bazel files are
    the root's scripts/ci-action-pins.mjs's.)"""
    found, problems = [], []

    def check(where: str, image: str) -> None:
        found.append((where, image))
        if not DIGEST.search(image) and not any(pattern.fullmatch(image) for pattern in UNPINNED_IMAGES):
            problems.append(f"{where}: {image} names its image without an @sha256: digest")

    github = root / ".github"
    for path in sorted(path for path in github.rglob("*") if path.is_file()) if github.is_dir() else []:
        text = path.read_text()
        if path.suffix in (".yml", ".yaml"):
            for number, line in yaml_lines(text):
                key = IMAGE_KEY.match(without_comment(line))
                if not key:
                    continue
                where = f"{path.relative_to(root)}:{number}"
                images = image_references(key.group(2))
                if images is None:
                    problems.append(f"{where}: a {key.group(1)} this check cannot read; "
                                    "name the image and its @sha256: digest literally")
                for image in images or []:
                    check(where, image)
        for number, line in enumerate(text.split("\n"), 1):
            for image in DOCKER_URL.findall(without_comment(line)):
                check(f"{path.relative_to(root)}:{number}", image)
    bazel_root = root / "tools" / "bazel"
    bazel = sorted(path for path in bazel_root.rglob("*")
                   if path.name in ("BUILD.bazel", "BUILD") or path.suffix == ".bzl") if bazel_root.is_dir() else []
    if (root / ".bazelrc").is_file():
        bazel.append(root / ".bazelrc")
    for path in bazel:
        for number, line in enumerate(path.read_text().splitlines(), 1):
            for image in DOCKER_URL.findall(line):
                check(f"{path.relative_to(root)}:{number}", image)
    for relative in tracked_files(root):
        if not is_container_build_file(Path(relative).name) or USER_TEMPLATE_DOCKERFILES.fullmatch(relative):
            continue
        stages = set()
        for number, line in enumerate((root / relative).read_text().splitlines(), 1):
            match = FROM_LINE.match(line)
            if not match:
                continue
            if match.group(1).lower() != "scratch" and match.group(1) not in stages:
                check(f"{relative}:{number}", match.group(1))
            if match.group(2):
                stages.add(match.group(2))
    return found, problems


class CiWorkflowTests(unittest.TestCase):
    def test_meta_ci_builds_with_the_root_toolchain_and_compile_environment(self):
        """meta-ci.yml compiles with the root CI's toolchain, environment and
        caches, so the dependency builds the root CI wrote to the sccache
        bucket, and its registry entry, are hits: rust-cache hashes the
        CARGO*/RUST* variables into its key and sccache serves an object only
        to a compile with the same inputs. Rust comes from the root's setup
        action alone, never from a toolchain action of its own."""
        channel = tomllib.loads((MONOREPO / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        meta, root = load(CI_WORKFLOW), load(ROOT_CI_WORKFLOW)
        self.assertEqual(meta["env"]["RUSTUP_TOOLCHAIN"], channel)
        self.assertEqual(hashed(meta["env"]), hashed(root["env"]))
        self.assertEqual(meta["env"]["SCCACHE_GCS_BUCKET_NAME"], root["env"]["SCCACHE_GCS_BUCKET_NAME"])
        reference = root["jobs"][ROOT_RUST_JOB]
        reference_uses = [uses_of(step) for step in steps_of(reference)]
        reference_sccache = next(step for step in steps_of(reference) if uses_of(step) == SETUP_SCCACHE)
        reference_cache = next(step for step in steps_of(reference) if uses_of(step) == "Swatinem/rust-cache")
        setup_order = [SETUP_RUST, SETUP_SCCACHE, SETUP_MOLD, "Swatinem/rust-cache"]
        self.assertEqual([use for use in reference_uses if use in setup_order], setup_order)
        jobs = rust_jobs(meta)
        self.assertEqual(sorted(jobs), sorted(["test-python", "console-backends", "console-fixtures",
                                               "console-acceptance", "flow-editor", "rust-unit-build",
                                               "rust-examples", "rust-conformance", "rust-integration",
                                               "rust-features-check", "rust-extension-state",
                                               "rust-extension-state-lib", "rust-voice", "rust-voice-lib",
                                               "warm-cache"]))
        for name, job in jobs.items():
            with self.subTest(job=name):
                self.assertEqual(hashed(job.get("env")), hashed(reference.get("env")))
                uses = [uses_of(step) for step in steps_of(job)]
                self.assertEqual([use for use in uses if use in setup_order], setup_order)
                sccache = next(step for step in steps_of(job) if uses_of(step) == SETUP_SCCACHE)
                self.assertEqual(sccache["with"], reference_sccache["with"])
                cache = next(step for step in steps_of(job) if uses_of(step) == "Swatinem/rust-cache")
                self.assertEqual(cache["with"]["shared-key"], reference_cache["with"]["shared-key"])
                self.assertEqual(cache["with"]["cache-targets"], reference_cache["with"]["cache-targets"])
                # The root CI's entry, restored and never saved here.
                self.assertIs(cache["with"]["save-if"], False)
        for path in META_WORKFLOWS:
            source = path.read_text()
            self.assertIsNone(re.search(r"(?m)^\s*-?\s*uses:\s*dtolnay/rust-toolchain", source), path.name)
        # The other jobs that need Rust: the metadata checks and the live lane.
        self.assertIn(SETUP_RUST, [uses_of(step) for step in steps_of(meta["jobs"]["meta-checks"])])
        live = load(E2E_LIVE_WORKFLOW)["jobs"]["e2e-live"]
        self.assertIn(SETUP_RUST, [uses_of(step) for step in steps_of(live)])

    def test_cargo_runs_one_package_from_the_root_wrapper(self):
        """Cargo runs from the repository's root through ./scripts/repo-cargo,
        for Meta's packages alone: a step in meta/ reaches it through make
        with CARGO=../scripts/repo-cargo. No step names the workspace, so no
        Meta job compiles Core's or Connect's packages (only as Meta's
        dependencies, from the root CI's cache), and the planner's product
        rows stay the only workspace lanes."""
        commands = []
        for path in (CI_WORKFLOW, E2E_LIVE_WORKFLOW):
            workflow = load(path)
            default = ((workflow.get("defaults") or {}).get("run") or {}).get("working-directory")
            self.assertEqual(default, "meta", path.name)
            for _, step, line in run_lines(workflow):
                # No step may hold the text `--workspace`, Cargo's flag in
                # any spelling, except as nextest's `--workspace-remap`,
                # which only tells an archived test binary where this
                # checkout is and selects no package.
                self.assertIsNone(NAMES_THE_WORKSPACE.search(line), f"{path.name}: {line}")
                self.assertIsNone(re.search(r"(^|[\s;&|(])cargo\s", line), f"{path.name}: raw cargo in {line}")
                if "repo-cargo" not in line:
                    continue
                commands.append(line)
                # A compile step may run its command under the resource
                # readings script: the command is what follows the script.
                line = line.removeprefix(f"{READINGS} ")
                if line.startswith("./scripts/repo-cargo "):
                    self.assertEqual(step.get("working-directory"), ".", line)
                    packages = re.findall(r"(?:^|\s)-p\s+(\S+)", line)
                    self.assertTrue(packages, line)
                    self.assertLessEqual(set(packages), set(META_PACKAGES), line)
                else:
                    self.assertRegex(line, r"^make (test-python|e2e-live) CARGO=\.\./scripts/repo-cargo$")
        self.assertEqual(sum(line.startswith("make ") for line in commands), 2)
        # Two more than before the lib jobs' builds became steps of their
        # own: each of the two jobs gained one build command.
        self.assertEqual(len(commands), 27, commands)
        self.assertNotIn("repo-cargo", INSTALLER_WORKFLOW.read_text())

    def test_the_no_workspace_rule_passes_only_the_archive_remap_flag(self):
        """The rule of the test above refuses the text `--workspace` in a run
        line however it is written (a bare flag, with a value, inside an
        array, in quotes, as the head of another word), and passes it only as
        nextest's `--workspace-remap` followed by a space or the line's end."""
        refused = (
            "./scripts/repo-cargo nextest run --workspace --no-fail-fast",
            "./scripts/repo-cargo test --workspace",
            "./scripts/repo-cargo check --workspace=all",
            "args=(--workspace)",
            'args+=("--workspace")',
            "./scripts/repo-cargo test '--workspace'",
            "./scripts/repo-cargo test --workspace-remapped",
            "cargo-nextest nextest run --archive-file a.tar.zst --workspace-remap=/somewhere",
            'cargo-nextest nextest run --archive-file a.tar.zst --workspace-remap "${GITHUB_WORKSPACE}" --workspace',
        )
        passed = (
            'cargo-nextest nextest run --archive-file a.tar.zst --workspace-remap "${GITHUB_WORKSPACE}" --no-fail-fast',
            'cargo-nextest nextest list --archive-file "${archive}" --workspace-remap "${GITHUB_WORKSPACE}" \\',
            "cargo-nextest nextest list --archive-file a.tar.zst --workspace-remap",
            "./scripts/repo-cargo nextest run -p meerkat-meta --lib --no-fail-fast",
        )
        for line in refused:
            with self.subTest(refused=line):
                self.assertIsNotNone(NAMES_THE_WORKSPACE.search(line))
        for line in passed:
            with self.subTest(passed=line):
                self.assertIsNone(NAMES_THE_WORKSPACE.search(line))

    def test_every_external_action_is_pinned_and_takes_the_root_pins(self):
        """The repository's Actions policy refuses an action that is not
        pinned to a full-length commit SHA. Each pin names the ref it was taken
        from in a trailing comment, one ref pins one commit everywhere, and
        Meta's root workflows take each action and ref the root's other
        workflows and actions already pin."""

        def pins_of(paths) -> dict[tuple[str, str], set[str]]:
            pins = {}
            for path in paths:
                for number, line in enumerate(path.read_text().splitlines(), 1):
                    found = re.match(r"\s*(?:-\s+)?uses:\s*(\S+)(.*)$", line)
                    if not found or found.group(1).startswith(("./", "docker://")):
                        continue
                    where = f"{path.relative_to(MONOREPO)}:{number}"
                    pinned = re.fullmatch(r"([\w.-]+/[\w./-]+)@([0-9a-f]{40})", found.group(1))
                    self.assertIsNotNone(pinned, f"{where}: {found.group(1)} is not pinned to a full commit SHA")
                    ref = re.fullmatch(r"\s+# (\S+)\s*", found.group(2))
                    self.assertIsNotNone(ref, f"{where}: the pin must name its ref in a trailing comment")
                    pins.setdefault((pinned.group(1), ref.group(1)), set()).add(pinned.group(2))
            return pins

        meta = pins_of(sorted((ROOT / ".github").rglob("*.y*ml")) + list(META_WORKFLOWS))
        root = pins_of(sorted(path for path in (MONOREPO / ".github").rglob("*.y*ml") if path not in META_WORKFLOWS))
        self.assertTrue(meta)
        for (action, ref), shas in sorted(meta.items()):
            self.assertEqual(len(shas), 1, f"{action} {ref} is pinned to {sorted(shas)}")
            if (action, ref) in root:
                self.assertEqual(shas, root[(action, ref)], f"{action} {ref} differs from the root's pin")
        for (action, ref), _ in sorted(pins_of(META_WORKFLOWS).items()):
            self.assertIn((action, ref), root, f"{action} {ref}: take a pin the root's workflows use")

    def test_every_job_runs_on_a_pinned_runner_image(self):
        """Each workflow job names its runner image, read as YAML: a job's
        inline label, or every label of the inline matrix it takes runs-on
        from; the release build legs are checked below. A runs-on this check
        cannot read is refused rather than skipped."""
        labels, problems = runner_problems(ROOT)
        self.assertEqual(problems, [])
        self.assertGreater(len(labels), 20)

    def test_the_release_build_legs_run_on_pinned_runner_images(self):
        """build_binaries takes runs-on from matrix data: the resolver's
        build_matrix, made of release-candidate.py BUILD_LEGS. A floating
        label there reaches the job without ever appearing in a workflow."""
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertEqual(release.count(f"runs-on: {MATRIX_RUNS_ON}"), 1)
        self.assertEqual(release.count(f"matrix: {RELEASE_BUILD_MATRIX}"), 1)
        spec = importlib.util.spec_from_file_location("release_candidate_legs", ROOT / "scripts/release-candidate.py")
        candidate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(candidate)
        legs = [leg["runs-on"] for leg in candidate.BUILD_LEGS]
        emitted = [leg["runs-on"] for tier in ("core", "trailing") for leg in candidate.build_matrix(tier)["include"]]
        self.assertEqual(sorted(emitted), sorted(legs))
        for label in legs:
            self.assertNotIn("latest", label, f"BUILD_LEGS runs-on {label} floats")
            self.assertRegex(label, PINNED_RUNNER)

    def test_every_container_image_is_pinned_by_digest_under_renovate(self):
        """A container tag moves when its publisher pushes; the build
        container fixes the release binaries' glibc floor. Every image
        Meta's CI names carries its digest, unless UNPINNED_IMAGES lists it
        with its reason: image_problems reads job and service containers,
        docker:// references in .github, and the FROM lines of every tracked
        Dockerfile or Containerfile except the installer's launch templates,
        which users build their own apps from. renovate.json proposes new
        digests (never automerged) for every pinned one, and the release
        runbook compares them with Core's."""
        found, problems = image_problems(ROOT)
        self.assertEqual(problems, [])
        renovate = json.loads((ROOT / "renovate.json").read_text())
        (manager,) = renovate["customManagers"]
        self.assertEqual(manager["datasourceTemplate"], "docker")
        patterns = [re.compile(item.strip("/")) for item in manager["managerFilePatterns"]]
        (match,) = [re.compile(item.replace("(?<", "(?P<")) for item in manager["matchStrings"]]
        (rule,) = [rule for rule in renovate["packageRules"] if rule.get("matchManagers") == ["custom.regex"]]
        self.assertIs(rule["automerge"], False)
        pinned = [(where, image) for where, image in found if DIGEST.search(image)]
        for where, image in pinned:
            relative = where.rsplit(":", 1)[0]
            self.assertTrue(any(pattern.search(relative) for pattern in patterns),
                            f"{relative} is not under Renovate's digest management")
            proposal = match.fullmatch(image)
            self.assertIsNotNone(proposal, f"{where}: Renovate's digest manager cannot read {image}")
            self.assertEqual(proposal.group("currentDigest"), image.rsplit("@", 1)[1])
        self.assertEqual(sorted({where.rsplit(":", 1)[0] for where, _ in pinned}),
                         [".github/workflows/release.yml"], pinned)
        # Every exception still names an image CI uses, so a stale one fails.
        for pattern in UNPINNED_IMAGES:
            self.assertTrue(any(pattern.fullmatch(image) for _, image in found), pattern.pattern)
        self.assertIn("compare Meta's pinned\ncontainer digests with Core's", (ROOT / "CONTRIBUTING.md").read_text())

    def test_the_pin_checks_refuse_each_floating_form(self):
        """The runner and image checks on planted trees: a clean tree passes,
        and each floating or unreadable form of a runner label or an image
        reference fails on the file it is planted in. The installer's launch
        templates are the only Dockerfiles left out."""
        # The run script's `image:` line is shell text, not a key.
        workflow = ("jobs:\n  build:\n    runs-on: ubuntu-24.04\n"
                    "    container: docker.io/library/rust:1@sha256:" + "a" * 64 + "\n    steps:\n"
                    "      - run: |\n          cat > values.yaml <<'VALUES'\n          image: example/app:1\n"
                    "          VALUES\n")
        bazel = ('platform(\n    exec_properties = {\n        "container-image": "docker://docker.io/library/'
                 'buildpack-deps:22.04@sha256:' + "b" * 64 + '",\n    },\n)\n')
        template = "FROM rust:1.94-bookworm AS build\nFROM debian:bookworm-slim\n"
        clean = {
            ".github/workflows/ci.yml": workflow,
            "tools/bazel/platforms/BUILD.bazel": bazel,
            "installer/launchkit/templates_app/Dockerfile": template,
            "installer/tests/fixtures/history_v1/launchkit/templates_app/Dockerfile": template,
            "ci/Dockerfile": "FROM docker.io/library/rust:1@sha256:" + "c" * 64 + " AS base\nFROM base\nFROM scratch\n",
        }
        planted = {
            "G1 a job container by tag": (".github/workflows/g1.yml",
                                          "jobs:\n  a:\n    runs-on: ubuntu-24.04\n"
                                          "    container: ghcr.io/example/builder:1\n    steps: []\n"),
            "G2 a service image by tag": (".github/workflows/g2.yml",
                                          "jobs:\n  a:\n    runs-on: ubuntu-24.04\n    services:\n"
                                          "      db:\n        image: postgres:16\n    steps: []\n"),
            "G3 runs-on as a block list": (".github/workflows/g3.yml",
                                           "jobs:\n  a:\n    runs-on:\n      - ubuntu-latest\n    steps: []\n"),
            "G4 a docker:// action by tag": (".github/workflows/g4.yml",
                                             "jobs:\n  a:\n    runs-on: ubuntu-24.04\n    steps:\n"
                                             "      - uses: docker://alpine:3.20\n"),
            "G5 a Bazel platform by tag beside a pinned one": (
                "tools/bazel/platforms/BUILD.bazel",
                bazel + '"container-image": "docker://quay.io/example/rbe:22.04",\n'),
            "a floating runner label": (".github/workflows/latest.yml",
                                        "jobs:\n  a:\n    runs-on: ubuntu-latest\n    steps: []\n"),
            "runs-on as a mapping": (".github/workflows/mapping.yml",
                                     "jobs:\n  a:\n    runs-on:\n      group: large\n    steps: []\n"),
            "an unreadable container expression": (".github/workflows/expr.yml",
                                                   "jobs:\n  a:\n    runs-on: ubuntu-24.04\n"
                                                   "    container: ${{ inputs.image }}\n    steps: []\n"),
            "a composite action's docker:// step": (".github/actions/x/action.yml",
                                                    "runs:\n  using: composite\n  steps:\n"
                                                    "    - uses: docker://alpine:3.20\n"),
            "a docker:// image in a run script": (".github/workflows/script.yml",
                                                  "jobs:\n  a:\n    runs-on: ubuntu-24.04\n    steps:\n"
                                                  "      - run: |\n          skopeo inspect docker://alpine:3.20\n"),
            "a Dockerfile outside the launch templates": ("installer/launchkit/Dockerfile",
                                                          "FROM rust:1.94-bookworm\n"),
            "a named Dockerfile": ("ci/app.Dockerfile", "FROM rust:1.94-bookworm\n"),
            "a Containerfile": ("ci/Containerfile", "FROM rust:1.94-bookworm\n"),
            "a matrix label that floats": (".github/workflows/matrix.yml",
                                           "jobs:\n  a:\n    runs-on: ${{ matrix.runs-on }}\n    strategy:\n"
                                           "      matrix:\n        runs-on: [ubuntu-24.04, macos-latest]\n"
                                           "    steps: []\n"),
        }

        def problems_of(files: dict[str, str]) -> list[str]:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                for relative, text in files.items():
                    (root / relative).parent.mkdir(parents=True, exist_ok=True)
                    (root / relative).write_text(text)
                return runner_problems(root)[1] + image_problems(root)[1]

        self.assertEqual(problems_of(clean), [])
        for case, (relative, text) in planted.items():
            with self.subTest(case=case):
                problems = problems_of({**clean, relative: text})
                self.assertTrue(any(problem.startswith((relative, Path(relative).name)) for problem in problems),
                                problems)

    def test_renovate_proposes_action_updates_like_core(self):
        """Actions run with the workflow token, OIDC and release secrets.
        Renovate keeps them pinned to commit SHAs
        (helpers:pinGitHubActionDigests), proposes a new version a week after
        its release, asks for approval before moving a pin whose tag now names
        another commit, and never automerges one: Core's github-actions rules,
        with the github-actions rule last so no later rule overrides it."""
        renovate = json.loads((ROOT / "renovate.json").read_text())
        self.assertIn("helpers:pinGitHubActionDigests", renovate.get("extends", []))
        self.assertEqual(sorted(renovate.get("enabledManagers", [])), ["custom.regex", "github-actions"])
        rules = renovate["packageRules"]
        last = rules[-1]
        self.assertEqual(last["matchManagers"], ["github-actions"])
        self.assertEqual(last["minimumReleaseAge"], "7 days")
        self.assertIs(last["automerge"], False)
        (digest,) = [rule for rule in rules if rule.get("matchManagers") == ["github-actions"]
                     and rule.get("matchUpdateTypes") == ["digest"]]
        self.assertIs(digest["dependencyDashboardApproval"], True)
        self.assertFalse([rule for rule in rules if rule.get("automerge") is True])
        self.assertNotIn("automerge", renovate)

    def test_meta_ci_holds_no_secret_and_grants_the_identity_only_to_compile_jobs(self):
        """Pull request code runs here with the workflow token read-only. A
        job gets the GCP workload identity (id-token) only to set up the
        sccache bucket the root CI shares, the root CI's model; no job holds
        a secret, and no checkout keeps the token."""
        workflow = load(CI_WORKFLOW)
        source = CI_WORKFLOW.read_text()
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        self.assertNotIn("secrets.", source)
        for name, job in workflow["jobs"].items():
            with self.subTest(job=name):
                permissions = job.get("permissions")
                if name in rust_jobs(workflow):
                    self.assertEqual(permissions, {"contents": "read", "id-token": "write"})
                elif name == "gate":
                    # The budget step reads this run's jobs and attempts.
                    self.assertEqual(permissions, {"contents": "read", "actions": "read"})
                else:
                    self.assertIsNone(permissions)
                for step in steps_of(job):
                    if uses_of(step) == "actions/checkout":
                        self.assertIs((step.get("with") or {}).get("persist-credentials"), False)
                        self.assertNotIn("ref", step.get("with") or {})
                        self.assertNotIn("repository", step.get("with") or {})

    def test_the_root_files_meta_ci_runs_reach_every_lane(self):
        """A change to a root file the compile jobs run (the setup actions,
        the scripts they run, the root wrapper) runs every lane, as the
        wrapper and the warm-dependency fetch did in Meta's own repository:
        ci-changes.py's MONOREPO_JOB_FILES is exactly what meta-ci.yml uses."""
        workflow = load(CI_WORKFLOW)
        used = set()
        for _, job in workflow["jobs"].items():
            for step in steps_of(job):
                use = step.get("uses") or ""
                if use.startswith("./"):
                    action = Path(use[2:]) / "action.yml"
                    used.add(action.as_posix())
                    text = (MONOREPO / action).read_text()
                    for script in re.findall(r"/\.\./\.\./\.\./(scripts/[\w.-]+)", text):
                        used.add(script)
                for line in str(step.get("run") or "").splitlines():
                    for script in re.findall(r"(?:^|[\s=])(?:\./|\.\./)(scripts/[\w.-]+)", line):
                        if step.get("working-directory") == "." or "../scripts/" in line:
                            used.add(script)
        self.assertEqual(used, set(ci_changes.MONOREPO_JOB_FILES))

    def test_decorator_authority_gate_actually_runs_in_ci(self):
        """A structural gate nobody invokes is a control that cannot fail.

        `verify-decorator-authority.py` exists because a decorator omitting
        `request_attempt_authority` compiles clean, warns nothing, and stranded
        72 identities downstream. If it is ever dropped from `meta-checks` the
        defect class goes unwatched again with no other signal, so pin the
        invocation here rather than trusting that the file's presence means it
        runs.
        """
        self.assertIn("- run: python3 scripts/verify-decorator-authority.py", job_block("meta-checks"))

    def test_meta_checks_run_every_contract_script_the_monorepo_keeps(self):
        """meta-checks runs Meta's contract tests and checks in meta/, on every
        run that reaches Meta: every scripts/test_*.py. The retired
        warm-dependency and cache-cleanup jobs took their scripts with them,
        and the Rust checks (rustfmt, unused dependencies, the Bazel graph)
        are the root CI's."""
        runs = [step["run"] for step in steps_of(load(CI_WORKFLOW)["jobs"]["meta-checks"]) if "run" in step]
        for kept in ("python3 scripts/test_ci_workflow.py", "python3 scripts/test_ci_changes.py",
                     "scripts/verify-version-parity.sh", "python3 scripts/verify-crate-publication.py",
                     "python3 scripts/verify-crate-package-files.py", "scripts/check-memory-bright-line",
                     "scripts/memory-evals --check"):
            self.assertIn(kept, runs)
        for retired in ("test_ci_warm_deps.py", "test_ci_drop_caches.py", "repo-cargo fmt",
                        "repo-cargo machete", "generate-bazel-rust-builds"):
            self.assertFalse([run for run in runs if retired in run], retired)
        tests = {path.name for path in (ROOT / "scripts").glob("test_*.py")}
        run_tests = {Path(run.split()[-1]).name for run in runs if run.startswith("python3 scripts/test_")}
        self.assertEqual(tests - run_tests, set())

    def test_the_continuity_export_scripts_run_their_tests_in_ci(self):
        """The standalone continuity export scripts ship to hosts that run
        them once, before they upgrade, with no other check behind them;
        so does the blob restore beside them, which a restore from a
        continuity mirror runs once.
        meta-checks runs on every run that reaches Meta; ci-changes.py lets
        their changes reach no lane on that promise. Each suite runs twice:
        on the runner's own Python and Node.js, and again, last in the job,
        on the oldest runtimes a 0.9.3 host may run them on, the 0.9.3 SDKs'
        floors (Python 3.10 and Node.js 18)."""
        python_suite = "- run: python3 tools/continuity-export/test_continuity_export.py\n"
        restore_suite = "- run: python3 tools/continuity-export/test_restore_blobs.py\n"
        node_suite = "- run: node --test tools/continuity-export/continuity-export.test.mjs\n"
        lines = job_block("meta-checks").splitlines()
        # job_block ends at the next job's header, below the comment above it.
        while lines and (not lines[-1].strip() or lines[-1].startswith("  #")):
            lines.pop()
        steps = "\n".join(lines) + "\n"
        floor_rows = re.search(
            "      - uses: actions/setup-python@[0-9a-f]{40} # v5\n"
            "        with:\n"
            '          python-version: "3\\.10"\n'
            f"      {re.escape(python_suite)}"
            f"      {re.escape(restore_suite)}"
            "      - uses: actions/setup-node@[0-9a-f]{40} # v6\n"
            "        with:\n"
            '          node-version: "18"\n'
            f"      {re.escape(node_suite)}"
            r"\Z", steps)
        self.assertIsNotNone(floor_rows, "meta-checks must end with the suites on the floor runtimes")
        before = steps[:floor_rows.start()]
        for suite in (python_suite, restore_suite, node_suite):
            self.assertEqual(before.count(suite), 1, suite)
        # The first runs use the runner's own runtimes.
        self.assertNotIn("actions/setup-python@", before)
        self.assertNotIn("actions/setup-node@", before)

    def test_typescript_sdk_suite_actually_runs_in_ci(self):
        """705 TypeScript tests existed and gated nothing.

        No job ran `sdk/typescript` at all: the only Makefile reference is
        `publish-dry-run-typescript`, which builds and packs without testing.
        So a wrong RPC method name, or a parser reading camelCase where the
        gateway sends snake_case, reached npm behind a typecheck that cannot
        see either. Pin the invocation rather than the job's existence - a job
        that stops running its tests looks identical to one that passes.

        `validate` and not `test`: most of these tests import the built
        `dist/`, so `test` alone fails with ERR_MODULE_NOT_FOUND.
        """
        self.assertIn("- run: npm --prefix sdk/typescript run validate --silent", job_block("test-typescript"))

    def test_no_job_caps_build_parallelism(self):
        """A CARGO_BUILD_JOBS cap throttled the dependency graph of the jobs
        that carried it. Every job builds uncapped."""
        for path in META_WORKFLOWS:
            code = [line for line in path.read_text().splitlines() if not line.strip().startswith("#")]
            self.assertEqual([line for line in code if "CARGO_BUILD_JOBS" in line], [], path.name)
            self.assertEqual([line for line in code if re.search(r"repo-cargo .*(\s-j\s*\d|--jobs)", line)], [],
                             path.name)

    def test_live_lanes_print_cargo_output_without_color_codes(self):
        """A run that holds a model provider's key is read by grep: its pass
        counts and its summary lines. dtolnay/rust-toolchain sets
        CARGO_TERM_COLOR=always when nothing set it, so Cargo and nextest put
        color codes inside those lines, and a check for a passing summary
        failed a run whose rows had all passed. Each such workflow sets the
        variable to never for all its jobs, and nothing sets color back. The
        live lane restores no cache and no compiled object, so nothing it runs
        beside the secret was built by another run."""
        live = []
        for path in meta_workflow_files(ROOT):
            source = path.read_text()
            if not re.search(r"secrets\.(ANTHROPIC|OPENAI|GEMINI|GOOGLE)_API_KEY\b", source):
                continue
            live.append(path.name)
            workflow = yaml.safe_load(source) or {}
            self.assertEqual((workflow.get("env") or {}).get("CARGO_TERM_COLOR"), "never", path.name)
            self.assertIsNone(re.search(r"--color[ =]+(always|auto)", source), path.name)
            for name, job in (workflow.get("jobs") or {}).items():
                self.assertNotIn("CARGO_TERM_COLOR", job.get("env") or {}, f"{path.name} job {name}")
                for step in job.get("steps") or []:
                    self.assertNotIn("CARGO_TERM_COLOR", step.get("env") or {}, f"{path.name} job {name}")
        self.assertIn("meta-e2e-live.yml", live)
        e2e = E2E_LIVE_WORKFLOW.read_text()
        self.assertEqual(e2e.count("${{ secrets.ANTHROPIC_API_KEY }}"), 1)
        for cache in ("rust-cache", "actions/cache", "setup-sccache-ci", "cache:", "id-token"):
            self.assertNotIn(cache, e2e)

    def test_console_checks_live_voice_evidence_without_paid_calls(self):
        block = job_block("console")
        self.assertIn("node console/voice-e2e-live.cjs --self-test\n", block)
        self.assertIn("node console/voice-e2e-live.cjs --self-test-audio\n", block)
        self.assertNotIn("run e2e:voice:live", block)

    def test_console_backends_are_built_once_from_fresh_bundles(self):
        """console-backends builds the five acceptance backends after `run
        build` (they embed crates/meerkat-meta/console-dist), packs exactly
        those five (an artifact keeps no file mode) and hands them to console
        and every console-browser leg, which run them from
        META_EXAMPLE_BIN_DIR and compile nothing."""
        examples = ["library_mode_reference", "console_acceptance_fixture", "memory_console_reference",
                    "workgraph_console_reference", "mcp_apps_gateway_fixture"]
        workflow = load(CI_WORKFLOW)
        backends = workflow["jobs"]["console-backends"]
        runs = [step.get("run", "") for step in steps_of(backends)]
        build = runs.index("./scripts/repo-cargo build -p meerkat-meta "
                           + " ".join(f"--example {name}" for name in examples) + " --timings")
        self.assertLess(runs.index("npm --prefix console run build --silent"), build)
        pack = next(run for run in runs if run.startswith("tar "))
        self.assertEqual(pack, 'tar -C "${CARGO_TARGET_DIR}/debug/examples" -cf "${RUNNER_TEMP}/console-backends.tar" '
                         + " ".join(examples))
        upload = next(step for step in steps_of(backends) if uses_of(step) == "actions/upload-artifact")
        self.assertEqual(upload["with"]["name"], "console-backends")
        self.assertIs(upload["with"]["overwrite"], True)
        for name in ("console", "console-browser"):
            with self.subTest(job=name):
                job = workflow["jobs"][name]
                self.assertEqual(job["needs"], ["changes", "console-backends"])
                fetch = next(step for step in steps_of(job) if uses_of(step) == "actions/download-artifact")
                self.assertEqual(fetch["with"]["name"], "console-backends")
                unpack = next(step["run"] for step in steps_of(job) if "META_EXAMPLE_BIN_DIR" in step.get("run", ""))
                self.assertIn('tar -C "${bin_dir}" -xf "${RUNNER_TEMP}/console-backends/console-backends.tar"', unpack)
                self.assertNotIn("repo-cargo", job_block(name))
                self.assertNotIn(SETUP_RUST, [uses_of(step) for step in steps_of(job)])

    def test_console_fixture_unit_regressions_build_with_the_fixtures(self):
        """The acceptance fixture example's own tests run in console-fixtures,
        built in one --no-run with the customization fixtures (one
        meerkat-meta library build for both), on Meta's 16 MiB test stack,
        before the npm script's cargo test finds its target fresh."""
        steps = steps_of(load(CI_WORKFLOW)["jobs"]["console-fixtures"])
        runs = [step.get("run", "") for step in steps]
        build = runs.index("./scripts/repo-cargo test -p meerkat-meta --example console_acceptance_fixture "
                           "--test console_customization_fixtures --no-run")
        unit = runs.index("./scripts/repo-cargo test -p meerkat-meta --example console_acceptance_fixture")
        fixtures = runs.index("npm --prefix console run fixtures:console-customization --silent")
        self.assertLess(build, unit)
        self.assertLess(unit, fixtures)
        for index in (unit, fixtures):
            self.assertEqual(steps[index]["env"]["RUST_MIN_STACK"], "16777216")
        products = json.loads((MONOREPO / "ci/products.json").read_text())["products"]
        (meta,) = [product for product in products if product["name"] == "meta"]
        self.assertEqual(meta["test_stack"], 16777216)
        package = json.loads((ROOT / "console/package.json").read_text())
        self.assertIn("scripts/repo-cargo test -p meerkat-meta --test console_customization_fixtures",
                      package["scripts"]["fixtures:console-customization"])
        self.assertEqual(CI_WORKFLOW.read_text().count("--example console_acceptance_fixture\n"), 1)

    def test_console_browser_and_perf_run_beside_console(self):
        """The two longest console steps run in their own jobs, each exactly
        once: perf:typing:browser bundles its own host and needs no Rust; the
        browser scenarios need the shared backends and freshly built console
        bundles, in that order."""
        source = CI_WORKFLOW.read_text()
        for command, count in (
            ("- run: npm --prefix console run perf:typing:browser --silent\n", 1),
            ("node console/browser-e2e.cjs", 1),
            ("run: npm --prefix console run e2e:mcp-apps --silent\n", 3),
            ("- run: node console/api-e2e.cjs\n", 1),
        ):
            self.assertEqual(source.count(command), count, command)

        perf = job_block("console-perf")
        self.assertIn("- run: npm --prefix console run perf:typing:browser --silent\n", perf)
        self.assertIn("playwright install --with-deps chromium", perf)
        self.assertNotIn("repo-cargo", perf)

        browser = job_block("console-browser")
        order = [
            "- name: Unpack the console acceptance backends\n",
            "- run: npm --prefix console run build --silent\n",
            "- run: npm --prefix console run build:acceptance --silent\n",
            "playwright install --with-deps chromium",
            "- name: Native MCP Apps rendering and reload\n",
            "- name: Native MCP Apps polling during an active model turn\n",
            "- name: Native MCP Apps on a builder host with identity-first members\n",
            "- if: matrix.shard != 'mcp-apps'\n        run: node console/browser-e2e.cjs --shard=${{ matrix.shard }}/6\n",
            "name: console-browser-${{ github.run_id }}-${{ matrix.shard }}-${{ github.run_attempt }}\n",
        ]
        positions = [browser.index(marker) for marker in order]
        self.assertEqual(positions, sorted(positions))
        # Six scenario legs, each dealt its own scenarios by the registry's
        # --shard=i/6 (the matrix and the shard count must agree), and one
        # leg that runs only the MCP Apps checks, on its evidence path under
        # meta/: the legs wait for the backends, so leg 1 no longer carries
        # them on top of its share.
        legs = re.search(r"(?m)^        shard: \[([0-9a-z, -]+)\]$", browser).group(1).split(", ")
        self.assertEqual(legs, [*(str(index) for index in range(1, 7)), "mcp-apps"])
        self.assertIn(f"--shard=${{{{ matrix.shard }}}}/{len(legs) - 1}\n", browser)
        self.assertIn("      fail-fast: false\n", browser)
        self.assertEqual(browser.count("        if: matrix.shard == 'mcp-apps'\n"), 3)
        self.assertIn("if-no-files-found: ${{ matrix.shard == 'mcp-apps' && 'error' || 'warn' }}\n", browser)
        self.assertEqual(browser.count("META_BROWSER_EVIDENCE: ${{ github.workspace }}/meta/output/playwright/"
                                       "console-acceptance/mcp-apps\n"), 3)
        self.assertIn("          path: meta/output/playwright/console-acceptance/\n", browser)
        # The gateway lanes take the default backend; only the builder lane
        # selects console_acceptance_fixture.
        self.assertEqual(browser.count("          META_MCP_APPS_BACKEND: builder\n"), 1)

        console = job_block("console")
        for kept in (
            "- run: node console/api-e2e.cjs\n",
            "- run: node console/voice-e2e.cjs\n",
            "- run: node console/memory-e2e.cjs\n",
            "- run: node console/workgraph-e2e.cjs\n",
            "- run: npm --prefix console run embedded:freshness --silent\n",
            "name: console-${{ github.run_id }}-${{ github.run_attempt }}\n",
        ):
            self.assertIn(kept, console)

    def test_the_acceptance_pack_runs_the_binaries_built_through_the_root_wrapper(self):
        acceptance = job_block("console-acceptance")
        order = [
            "- run: npm --prefix console run build --silent\n",
            "./scripts/repo-cargo build -p meerkat-meta --example incident_command_center --bin mcp_fixture\n",
            'echo "META_EXAMPLE_BIN_DIR=${bin_dir}" >> "${GITHUB_ENV}"\n',
            "run: examples/001-incident-command-center-pack/topology_examples.sh\n",
        ]
        positions = [acceptance.index(marker) for marker in order]
        self.assertEqual(positions, sorted(positions))
        script = (ROOT / "examples/001-incident-command-center-pack/topology_examples.sh").read_text()
        self.assertIn("for required in incident_command_center mcp_fixture; do", script)

    def test_the_flow_editor_jobs_split_its_make_target(self):
        """flow-editor-contracts runs the checks that need neither the binary
        nor a browser; flow-editor builds meta_flow_editor and runs the
        rendered browser checks. Together they run exactly the npm scripts
        `make test-flow-editor` runs."""
        makefile = (ROOT / "Makefile").read_text()
        target = re.search(r"(?m)^test-flow-editor:[^\n]*\n((?:\t[^\n]*\n)+)", makefile).group(1)
        expected = re.findall(r"npm --prefix flow-editor run (\S+) --silent", target)
        self.assertEqual(len(expected), 4)
        jobs = {}
        for name in ("flow-editor-contracts", "flow-editor"):
            block = job_block(name)
            jobs[name] = re.findall(r"- run: npm --prefix flow-editor run (\S+) --silent\n", block)
        self.assertEqual(jobs["flow-editor-contracts"], ["test:controller", "test:visual-contract"])
        self.assertEqual(jobs["flow-editor"], ["test:browser-source", "test:browser-interactions"])
        self.assertEqual(sorted(jobs["flow-editor-contracts"] + jobs["flow-editor"]), sorted(expected))
        contracts = job_block("flow-editor-contracts")
        self.assertNotIn("repo-cargo", contracts)
        self.assertNotIn("playwright", contracts)
        browser = job_block("flow-editor")
        self.assertLess(browser.index("./scripts/repo-cargo build -p meerkat-meta --bin meta_flow_editor\n"),
                        browser.index("test:browser-source"))

    def test_the_budget_is_the_root_ci_step_byte_for_byte(self):
        """At the cutover Meta CI gets the root CI's time limit: lane
        execution 1080 s and push-to-terminal 1200 s, runner queue excluded,
        enforced on pull requests only. The gate runs ci.yml's own budget
        step, copied byte for byte (the root's
        scripts/ci-release-projection-selftest.mjs checks the same from
        Core's side), with this workflow's job names, and the jobs that wait
        for another job besides the classification are exactly those its
        LANE_UPSTREAMS names: a lane's execution path then holds every job
        it waited for."""
        meta_text, root_text = CI_WORKFLOW.read_text(), ROOT_CI_WORKFLOW.read_text()
        name = "Enforce lane execution budget"
        self.assertEqual(step_run(meta_text, name), step_run(root_text, name))
        meta, root = load(CI_WORKFLOW), load(ROOT_CI_WORKFLOW)
        for key in ("CI_MAX_EXEC_SECONDS", "CI_MAX_TERMINAL_SECONDS", "CI_QUEUE_WARN_SECONDS"):
            self.assertEqual(meta["env"][key], root["env"][key], key)
        self.assertEqual(meta["env"]["CI_MAX_EXEC_SECONDS"], "1080")
        self.assertEqual(meta["env"]["CI_MAX_TERMINAL_SECONDS"], "1200")
        gate = meta["jobs"]["gate"]
        step = next(step for step in steps_of(gate) if step.get("name") == name)
        env = step["env"]
        self.assertEqual(step["if"], "${{ !cancelled() }}")
        self.assertEqual(env["ENFORCE_BUDGET"], "${{ github.event_name == 'pull_request' }}")
        self.assertEqual(env["GATE_JOB_NAME"], gate["name"])
        self.assertEqual(env["CLASSIFICATION_JOB_NAME"], meta["jobs"]["changes"]["name"])
        self.assertEqual(env["ROOT_JOB_NAMES"], meta["jobs"]["changes"]["name"])
        self.assertEqual(env["MAX_EXEC_SECONDS"], "${{ env.CI_MAX_EXEC_SECONDS }}")
        self.assertEqual(env["MAX_TERMINAL_SECONDS"], "${{ env.CI_MAX_TERMINAL_SECONDS }}")
        self.assertEqual(env["QUEUE_WARN_SECONDS"], "${{ env.CI_QUEUE_WARN_SECONDS }}")
        self.assertEqual(env["GUARD_LANE_REASONS"], "{}")
        declared = {}
        for entry in env["LANE_UPSTREAMS"].split(";"):
            waiter, _, upstreams = entry.partition("=")
            declared[waiter] = set(upstreams.split("|"))
        waits = {}
        for job_id, job in meta["jobs"].items():
            needs = job.get("needs") or []
            needs = [needs] if isinstance(needs, str) else needs
            upstream = {meta["jobs"][need]["name"] for need in needs if need != "changes"}
            if job_id == "gate" or not upstream:
                continue
            label = job["name"].split(" (")[0] + ("*" if "strategy" in job else "")
            waits[label] = upstream
        self.assertEqual(declared, waits)

    def test_the_integration_shards_cover_every_target_once(self):
        """rust-integration runs meerkat-meta's integration tests in shards
        by test target name, each shard a list of patterns (rust-examples
        and rust-conformance run the other two packages' whole suites).
        Their union is every integration test target of meerkat-meta only
        while each matches exactly one pattern of one shard, none needs a
        feature the shard does not enable, and no package turns its tests/
        discovery off."""
        workflow = load(CI_WORKFLOW)
        job = workflow["jobs"]["rust-integration"]
        rows = job["strategy"]["matrix"]["include"]
        # The shards are numbered 1..N and the job's name says of how many.
        self.assertEqual([row["shard"] for row in rows], list(range(1, len(rows) + 1)))
        self.assertEqual(job["name"], f"rust-integration (${{{{ matrix.shard }}}}/{len(rows)})")
        patterns = [pattern for row in rows for pattern in row["tests"].split()]
        problems = []
        for package in META_PACKAGES:
            manifest = MONOREPO / "meta" / "crates" / package / "Cargo.toml"
            if tomllib.loads(manifest.read_text()).get("package", {}).get("autotests") is False:
                problems.append(f"{package}: autotests = false")
        for package, target in meta_test_targets():
            # rust-examples and rust-conformance run the two small crates'
            # whole suites.
            if target["kind"] != ["test"] or package != "meerkat-meta":
                continue
            label = f"{package} test {target['name']}"
            if target.get("required-features"):
                problems.append(f"{label}: required-features {target['required-features']}")
            matched = [pattern for pattern in patterns if fnmatch.fnmatchcase(target["name"], pattern)]
            if len(matched) != 1:
                problems.append(f"{label}: matches {matched}")
        self.assertEqual(problems, [])
        # Every pattern selects something, or Cargo refuses the shard.
        names = [target["name"] for package, target in meta_test_targets()
                 if target["kind"] == ["test"] and package == "meerkat-meta"]
        for pattern in patterns:
            self.assertTrue(fnmatch.filter(names, pattern), pattern)
        # The step passes one --test per pattern, without letting the shell
        # expand them against the checkout.
        self.assertIn('read -r -a patterns <<< "${TESTS}"\n', job_block("rust-integration"))

    def test_the_unit_lane_builds_once_and_its_partitions_run_every_lib_test_once(self):
        """rust-unit-build compiles meerkat-meta's lib test harness once into
        a nextest archive, checks that the count partitions cover its tests
        exactly once and hands it to the run's legs; each rust-unit leg runs
        one count partition from that archive and compiles nothing. The
        matrix is 1..N and the job's name, the build's check and the leg's
        --partition all say N, so every test runs in exactly one leg. The
        selection (the package's lib, default features), the nextest profile
        (the default one) and Meta's 16 MiB stack are the ones the legs had
        when each compiled the harness itself."""
        workflow = load(CI_WORKFLOW)
        build, job = workflow["jobs"]["rust-unit-build"], workflow["jobs"]["rust-unit"]
        legs = job["strategy"]["matrix"]["partition"]
        self.assertEqual(legs, list(range(1, len(legs) + 1)))
        self.assertEqual(job["name"], f"rust-unit (${{{{ matrix.partition }}}}/{len(legs)})")
        self.assertIs(job["strategy"]["fail-fast"], False)
        self.assertEqual(build["name"], "rust-unit-build")
        self.assertNotIn("strategy", build)
        self.assertEqual(build["needs"], "changes")
        self.assertEqual(job["needs"], ["changes", "rust-unit-build"])
        self.assertEqual(build["if"], job["if"])
        archive = "nextest-meta-unit.tar.zst"
        built_at = "nextest-meta-unit.workspace"
        artifact = "nextest-meta-unit-${{ github.run_id }}"
        stack = "16777216"

        def named(owner: dict, name: str) -> tuple[int, dict]:
            found = [(index, step) for index, step in enumerate(steps_of(owner)) if step.get("name") == name]
            self.assertEqual(len(found), 1, name)
            return found[0]

        # The build: one archive of the package's lib tests, through the root
        # wrapper, on Meta's stack (the step's environment reaches rustc too,
        # as the legs' step did). It is the step's whole script, run under
        # the resource readings script
        # (test_the_resource_readings_script_exits_with_its_commands_status).
        compile_at, compile_step = named(build, "Build the lib test archive")
        self.assertEqual(compile_step["working-directory"], ".")
        self.assertEqual(compile_step["env"], {"RUST_MIN_STACK": stack})
        self.assertEqual(compile_step["run"].strip(), f'{READINGS} {COMPILE_PATHS["rust-unit-build"][1]}')
        self.assertEqual(COMPILE_PATHS["rust-unit-build"][1],
                         "./scripts/repo-cargo nextest archive -p meerkat-meta --lib --timings "
                         f'--archive-file "${{RUNNER_TEMP}}/{archive}"')
        archives = [line for _, _, line in run_lines(workflow) if re.search(r"\bnextest archive\b", line)]
        self.assertEqual(len(archives), 1, archives)
        record_at, record = named(build, "Record the workspace path the archive was built at")
        self.assertEqual(record["run"].strip(),
                         f"printf '%s\\n' \"${{GITHUB_WORKSPACE}}\" > \"${{RUNNER_TEMP}}/{built_at}\"")
        check_at, check = named(build, "Verify the partitions cover every test exactly once")
        self.assertEqual(check["working-directory"], ".")
        self.assertEqual(check["env"], {"PARTITIONS": str(len(legs))})
        self.assertIn(f'archive="${{RUNNER_TEMP}}/{archive}"\n', check["run"])
        self.assertIn('cargo-nextest nextest list --archive-file "${archive}" '
                      '--workspace-remap "${GITHUB_WORKSPACE}" \\\n', check["run"])
        self.assertIn('list --partition "count:${index}/${PARTITIONS}" >> "${RUNNER_TEMP}/union.txt"\n', check["run"])
        self.assertIn('[[ -s "${RUNNER_TEMP}/all.txt" ]]', check["run"])
        self.assertIn('cmp -s "${RUNNER_TEMP}/all.txt" "${RUNNER_TEMP}/union.txt"', check["run"])
        self.assertNotIn("--profile", check["run"])
        # The archive's upload; the timing reports' upload is the job's last
        # step (test_the_four_compile_paths_are_measured_in_steps_of_their_own).
        upload_at, upload = named(build, "Retain the lib test archive")
        self.assertEqual(uses_of(upload), "actions/upload-artifact")
        self.assertEqual([step.get("name") for step in steps_of(build) if uses_of(step) == "actions/upload-artifact"],
                         ["Retain the lib test archive", MEASURE_UPLOAD])
        # Keyed by the run, not the attempt: a rerun of one failed leg finds
        # the archive the first attempt built, and a rerun of the build
        # replaces it.
        self.assertEqual(upload["with"]["name"], artifact)
        self.assertEqual([line.strip() for line in upload["with"]["path"].splitlines()],
                         [f"${{{{ runner.temp }}}}/{archive}", f"${{{{ runner.temp }}}}/{built_at}"])
        self.assertEqual(upload["with"]["if-no-files-found"], "error")
        self.assertIs(upload["with"]["overwrite"], True)
        self.assertEqual(upload["with"]["retention-days"], 1)
        self.assertEqual([compile_at, record_at, check_at, upload_at],
                         sorted([compile_at, record_at, check_at, upload_at]))
        self.assertEqual(build["timeout-minutes"], job["timeout-minutes"])

        # The legs: the checkout, Rust, nextest and the run's own archive,
        # fetched by name alone (no token, run or repository: the action then
        # reads the current run's artifacts only).
        self.assertEqual([uses_of(step) for step in steps_of(job) if uses_of(step)],
                         ["actions/checkout", SETUP_RUST, "./.github/actions/setup-cargo-tool-ci",
                          "actions/download-artifact"])
        fetch_at, fetch = next((index, step) for index, step in enumerate(steps_of(job))
                               if uses_of(step) == "actions/download-artifact")
        self.assertEqual(fetch["with"], {"name": artifact, "path": "${{ runner.temp }}/nextest-archive"})
        # The lib's tests read fixtures by the build's absolute path
        # (env!("CARGO_MANIFEST_DIR")), and some return early when a file
        # is absent: a leg at another path fails before it runs any. The
        # same step refuses a fetched archive or path record that is not a
        # non-empty file (test_a_unit_leg_refuses_an_archive_it_cannot_trust
        # runs it).
        guard_at, guard = named(job, UNIT_LEG_GUARD)
        self.assertEqual(guard["shell"], "bash")
        self.assertIn(f'archive="${{RUNNER_TEMP}}/nextest-archive/{archive}"\n', guard["run"])
        self.assertIn(f'record="${{RUNNER_TEMP}}/nextest-archive/{built_at}"\n', guard["run"])
        self.assertIn('if [[ ! -s "${archive}" ]]; then\n', guard["run"])
        self.assertIn('if [[ ! -s "${record}" ]]; then\n', guard["run"])
        self.assertIn('if [[ -z "${built}" || "${built}" != "${GITHUB_WORKSPACE}" ]]; then\n', guard["run"])
        self.assertEqual(guard["run"].count("exit 1\n"), 3)
        run_at, step = named(job, "Lib tests")
        self.assertEqual([fetch_at, guard_at, run_at], sorted([fetch_at, guard_at, run_at]))
        self.assertEqual(run_at, len(steps_of(job)) - 1)
        self.assertEqual(step["working-directory"], ".")
        self.assertEqual(step["env"], {"RUST_MIN_STACK": stack, "PARTITION": "${{ matrix.partition }}"})
        # The step that runs the tests decides its leg: nothing lets it be
        # skipped, pass on a failure or run under another shell.
        for key in ("continue-on-error", "if", "shell"):
            self.assertNotIn(key, step)
        # The same for the rest of the lane: no other step of the build or
        # of a leg can be skipped or fail without failing its job, and
        # neither job can fail without failing the run. Only the build's
        # three measurement steps carry a condition or continue-on-error
        # (test_the_four_compile_paths_are_measured_in_steps_of_their_own),
        # and its swap step continue-on-error alone
        # (test_the_unit_build_adds_swap_before_its_compile).
        for owner in (build, job):
            self.assertNotIn("continue-on-error", owner)
            for each in steps_of(owner):
                if each.get("name") not in (MEASURE_BEFORE, MEASURE_AFTER, MEASURE_UPLOAD, SWAP_PREFLIGHT):
                    for key in ("continue-on-error", "if"):
                        self.assertNotIn(key, each, each.get("name") or uses_of(each))
        # A partition that selects no test is a failure, said in the command
        # and not left to nextest's default. The command is the step's whole
        # script: a leg holds no resource sampler (the readings are printed
        # where the harness is compiled, by the build's step).
        self.assertEqual(step["run"].strip(),
                         f'cargo-nextest nextest run --archive-file "${{RUNNER_TEMP}}/nextest-archive/{archive}" '
                         '--workspace-remap "${GITHUB_WORKSPACE}" --no-tests=fail '
                         f'--partition "count:${{PARTITION}}/{len(legs)}" --no-fail-fast')
        # A leg compiles nothing and holds no identity: no wrapper, compiler
        # cache, linker or registry step, no permissions of its own.
        self.assertNotIn("permissions", job)
        self.assertNotIn("env", job)
        code = "\n".join(line for line in job_block("rust-unit").splitlines() if not line.strip().startswith("#"))
        for absent in ("repo-cargo", SETUP_SCCACHE, SETUP_MOLD, "Swatinem/rust-cache", "id-token", "RUSTC_WRAPPER"):
            self.assertNotIn(absent, code)

    def test_a_unit_leg_refuses_an_archive_it_cannot_trust(self):
        """Before it runs a test, a rust-unit leg checks what it fetched: the
        archive and the record of the workspace path it was built at are
        non-empty files, and that path is the leg's own. The step's own
        script, run as the runner runs a `shell: bash` step, goes on only for
        the build's path; another path, a missing, empty or blank record and
        a missing or empty archive each stop the leg with their own error
        line."""
        steps = steps_of(load(CI_WORKFLOW)["jobs"]["rust-unit"])
        (guard,) = [step for step in steps if step.get("name") == UNIT_LEG_GUARD]
        self.assertEqual(guard["shell"], "bash")
        self.assertLess(steps.index(guard), len(steps) - 1)
        self.assertEqual(steps[-1]["name"], "Lib tests")
        here = "/work/meerkat"
        cases = (
            ("the build's path", b"archive", f"{here}\n", 0, ""),
            ("another path", b"archive", "/elsewhere/meerkat\n", 1, "was built at '/elsewhere/meerkat'"),
            ("no record", b"archive", None, 1, "path the archive was built at is missing or empty"),
            ("an empty record", b"archive", "", 1, "path the archive was built at is missing or empty"),
            ("a blank line for a record", b"archive", "\n", 1, "was built at ''"),
            ("no archive", None, f"{here}\n", 1, "the lib test archive is missing or empty"),
            ("an empty archive", b"", f"{here}\n", 1, "the lib test archive is missing or empty"),
        )
        for name, archive, record, status, said in cases:
            with self.subTest(case=name), tempfile.TemporaryDirectory() as temp:
                fetched = Path(temp) / "nextest-archive"
                fetched.mkdir()
                if archive is not None:
                    (fetched / "nextest-meta-unit.tar.zst").write_bytes(archive)
                if record is not None:
                    (fetched / "nextest-meta-unit.workspace").write_text(record)
                result = subprocess.run(
                    ["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", guard["run"]],
                    env={"PATH": os.environ["PATH"], "RUNNER_TEMP": temp, "GITHUB_WORKSPACE": here},
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                errors = [line for line in result.stdout.splitlines() if line.startswith("::error::")]
                self.assertEqual(len(errors), status, result.stdout)
                if said:
                    self.assertIn(said, errors[0])

    def test_the_resource_readings_script_exits_with_its_commands_status(self):
        """scripts/ci-resource-readings.sh is the one implementation of the
        30-second resource readings: Meta CI's unit build and the build steps
        of the extension-state and voice lib jobs run their one command under
        it, and each step so starts a sampler of its own. The readings must
        never decide the step: errexit is off before the sampler starts, the
        sampler is one subshell whose status nobody reads, the command
        stands on a line of its own with nothing appended, and the script
        ends with that command's exit status. Only the sampler holds a
        trap, so a signal ends the script's shell as it ends a one-line
        step. A reading holds numbers and fixed words only: a process name
        is compared with a fixed list and never printed, and no line holds
        arguments, environment values or paths. With `--size` the script
        prints the runner's size once, the lines the swap step prints first,
        and adds no swap. The steps' scripts are also run here, the way the
        runner starts a step that names no shell (`bash -e`), with a
        stand-in for the repository's wrapper."""
        self.assertTrue(os.access(READINGS_SCRIPT, os.X_OK), "the script is executable")
        source = READINGS_SCRIPT.read_text()
        self.assertEqual(source.splitlines()[0], "#!/usr/bin/env bash")
        self.assertNotRegex(source, r"[^\x00-\x7f]")
        lines = [line.strip() for line in source.splitlines()[1:] if line.strip() and not line.strip().startswith("#")]
        head, frame = lines[:lines.index("set +e")], lines[lines.index("set +e"):]
        # Before the sampler: the size print, and a refusal of anything that
        # is neither a command nor `--size`.
        self.assertEqual(head, [
            'if [[ "$#" -eq 1 && "$1" == "--size" ]]; then',
            "show_resources() {", "nproc", "free -m", "if [[ -d /mnt ]]; then", "df -h / /mnt", "else", "df -h /", "fi",
            "swapon --show || true", "}", "show_resources", "exit", "fi",
            'if [[ "$#" -eq 0 || "$1" == -* ]]; then',
            'echo "usage: ci-resource-readings.sh COMMAND [ARGUMENT...] | --size" >&2', "exit 2", "fi",
        ])
        # The size print is the one the unit build's swap step prints first.
        size = head[head.index("show_resources() {"):head.index("}") + 1]
        (swap,) = [step for step in steps_of(load(CI_WORKFLOW)["jobs"]["rust-unit-build"])
                   if step.get("name") == SWAP_PREFLIGHT]
        printed = [line.strip() for line in swap["run"].splitlines() if line.strip()]
        at = printed.index("show_resources() {")
        self.assertEqual(printed[at:at + len(size)], size)
        begin, end, at = frame.index("("), frame.index(") 2>/dev/null &"), frame.index('"$@"')
        self.assertEqual(frame[:begin + 1], ["set +e", "("])
        self.assertEqual(frame[end:at], [") 2>/dev/null &", "sampler=$!"])
        self.assertEqual(frame[at + 1:], [
            "status=$?",
            'kill "${sampler}" 2>/dev/null',
            'pkill -P "${sampler}" 2>/dev/null',
            'wait "${sampler}" 2>/dev/null',
            'exit "${status}"',
        ])
        sampler = frame[begin + 1:end]
        outside = head + frame[:begin] + frame[end:]
        self.assertFalse([line for line in outside if "trap" in line or "exec" in line], "only the sampler traps")
        self.assertEqual(sum("status" in line for line in outside), 2, "the status is set once and used once")
        for line in sampler:
            self.assertIsNone(re.search(r"\bstatus=|\$\{?status\b", line), f"the sampler neither sets nor reads the status: {line}")
            self.assertNotIn("GITHUB_", line, "the sampler writes no workflow command file")
            self.assertIsNone(re.search(r"(^|[^<])>[^&]|>>", line.replace("2>/dev/null", "")),
                              f"the sampler writes to its output only: {line}")
            for exit_code in re.findall(r"\bexit\b\s*(\S*)", line):
                self.assertIn(exit_code.rstrip(";'"), ("0",), line)
            # Every external command of the sampler but its sleep is one
            # `stat` under a time limit, run as the command substitution's
            # own process, so ending the sampler's children ends it.
            self.assertEqual(line.count("$("), line.count("$((") + len(re.findall(r"\$\(exec timeout \d+ stat -", line)), line)
            self.assertIsNone(re.search(r"(?:^|[\s(])(du|df|ps|find|ls|cat|env|printenv)(\s|$)", line),
                              f"the sampler walks no directory and lists no process arguments or environment: {line}")
        self.assertIn("for ((sample = 1; sample <= 200; sample++)); do", sampler)
        # Between two samples the sampler waits thirty one-second slices and,
        # before each, leaves when its parent is no longer the step's shell:
        # it outlives that shell by about a second at most.
        self.assertEqual(sampler[-9:], [
            "for ((slice = 0; slice < 30; slice++)); do",
            'parent=""',
            'read -r _ _ _ parent _ < "/proc/${BASHPID}/stat"',
            'if [[ "${parent:-$$}" != "$$" ]] || ! kill -0 "$$"; then exit 0; fi',
            "sleep 1 &",
            "nap=$!",
            'wait "${nap}" || exit 0',
            "done",
            "done",
        ])
        self.assertEqual(sum(line.startswith("sleep ") for line in sampler), 1)
        # A process name is compared with fixed names and never printed: the
        # variable that holds it is set from the process's status file and
        # used once, in the `case` whose arms are literal names; the lines
        # are built from numbers and fixed words only.
        self.assertIn("for entry in /proc/[0-9]*/status; do", sampler)
        self.assertEqual([line for line in sampler if "name" in line],
                         ['name="" resident=""', 'Name:) name="${value}${rest:+ ${rest}}" ;;', 'case "${name}" in'])
        self.assertEqual([line for line in sampler if "entry" in line],
                         ["for entry in /proc/[0-9]*/status; do", 'done < "${entry}"'])
        arms = sampler[sampler.index('case "${name}" in') + 1:]
        arms = arms[:arms.index("esac")]
        self.assertEqual(arms[-1], "*) other=$((other + resident)) ;;")
        for arm in arms[:-1]:
            self.assertRegex(arm, r"^[a-z0-9][a-z0-9.-]*( \| [a-z0-9][a-z0-9.-]*)*\) [a-z]+=\$\(\([a-z]+ \+ resident\)\) ;;$")
        printed = [line for line in sampler if line.startswith(("printf ", '"${now}"'))]
        self.assertEqual(printed, [
            "printf -v now '%(%H:%M:%SZ)T' -1",
            "printf 'resource reading [%s] %d: memory %s; swap %s; memory pressure full avg10 %s; "
            "root filesystem %s%s; resident: %s\\n' \\",
            '"${now}" "${sample}" "${memory}" "${swap}" "${pressure}" "${root}" "${target}" "${sums}"',
        ])
        self.assertFalse([line for line in sampler if line.startswith("echo ")], "the sampler prints with one printf")
        for variable in ("memory", "swap", "root", "target", "sums"):
            for line in sampler:
                assigned = re.match(rf'{variable}\+?="(.*)"$', line)
                if assigned:
                    self.assertIsNone(re.search(r"\$\{|\$[A-Za-z_]", assigned.group(1)),
                                      f"{variable} is built from arithmetic and fixed words: {line}")
        self.assertEqual([line for line in sampler if "pressure=" in line], [
            'pressure="unavailable"',
            'if [[ "${kind}" == full && "${average}" =~ ^avg10=[0-9.]+$ ]]; then pressure="${average#avg10=}"; fi',
        ])

        program = "\n".join((
            f"#!{sys.executable}",
            "import os, sys, time",
            "open(os.environ['STAND_IN_ARGUMENTS'], 'w').write('\\n'.join(sys.argv[1:]))",
            "time.sleep(float(os.environ['STAND_IN_SECONDS']))",
            "sys.exit(int(os.environ['STAND_IN_STATUS']))",
            "",
        ))
        free = r"(\d+ MiB free of \d+ MiB|unavailable)"
        reading = re.compile(
            r"^resource reading \[[0-9:Z]*\] 1: memory (\d+ MiB available of \d+ MiB|unavailable); "
            r"swap (\d+ MiB used of \d+ MiB|unavailable); memory pressure full avg10 ([0-9.]+|unavailable); "
            rf"root filesystem {free}(; target filesystem {free})?; resident: (unavailable|compiler \d+ MiB, "
            r"linker \d+ MiB, compile cache \d+ MiB, build tool \d+ MiB, test runner \d+ MiB, other \d+ MiB)$")

        def staged(tmp):
            """A checkout's scripts folder with the readings script and a
            stand-in for the wrapper, and a path that holds bash alone."""
            scripts = Path(tmp) / "scripts"
            scripts.mkdir()
            (scripts / "repo-cargo").write_text(program)
            (scripts / "repo-cargo").chmod(0o755)
            shutil.copy(READINGS_SCRIPT, scripts / READINGS_SCRIPT.name)
            (scripts / READINGS_SCRIPT.name).chmod(0o755)
            only_bash = Path(tmp) / "only-bash"
            only_bash.mkdir()
            (only_bash / "bash").symlink_to(shutil.which("bash"))
            return str(only_bash)

        workflow = load(CI_WORKFLOW)
        for job_name in WITH_READINGS:
            compile_name, command, _ = COMPILE_PATHS[job_name]
            (step,) = [step for step in steps_of(workflow["jobs"][job_name]) if step.get("name") == compile_name]
            for key in ("shell", "continue-on-error", "if"):
                self.assertNotIn(key, step)
            self.assertEqual(step["run"].strip(), f"{READINGS} {command}")
            cases = (
                ("the command passes", 0, os.environ.get("PATH", ""), None),
                ("the command fails", 1, os.environ.get("PATH", ""), None),
                ("the build fails", 101, os.environ.get("PATH", ""), None),
                ("no tool of the sampler exists", 3, None, None),
                ("the step's output cannot be written", 4, os.environ.get("PATH", ""), "/dev/full"),
            )
            for name, status, path, output in cases:
                with self.subTest(job=job_name, case=name), tempfile.TemporaryDirectory() as tmp:
                    only_bash = staged(tmp)
                    script = Path(tmp) / "step.sh"
                    script.write_text(step["run"])
                    arguments = Path(tmp) / "arguments"
                    runner_temp = Path(tmp) / "runner-temp"
                    env = {"PATH": only_bash if path is None else path, "RUNNER_TEMP": str(runner_temp),
                           "CARGO_TARGET_DIR": str(Path(tmp) / "target"), "STAND_IN_ARGUMENTS": str(arguments),
                           "STAND_IN_SECONDS": "2", "STAND_IN_STATUS": str(status)}
                    sink = open(output, "w") if output else None
                    try:
                        result = subprocess.run([shutil.which("bash"), "-e", str(script)], cwd=tmp, env=env, text=True,
                                                timeout=60, stdout=sink or subprocess.PIPE, stderr=subprocess.PIPE)
                    finally:
                        if sink:
                            sink.close()
                    self.assertEqual(result.returncode, status, result.stderr)
                    self.assertEqual(arguments.read_text().split("\n"),
                                     shlex.split(command.replace("${RUNNER_TEMP}", str(runner_temp)))[1:])
                    if output is None:
                        readings = result.stdout.splitlines()
                        self.assertEqual(len(readings), 1, result.stdout)
                        self.assertRegex(readings[0], reading)
                        self.assertEqual(result.stderr, "", "the sampler writes nothing to the step's error output")

        # Neither a command nor `--size`: refused, and nothing runs.
        for arguments in ([], ["--sizes"], ["--size", "more"], ["-x", "true"]):
            with self.subTest(arguments=arguments), tempfile.TemporaryDirectory() as tmp:
                staged(tmp)
                result = subprocess.run([str(Path(tmp) / "scripts" / READINGS_SCRIPT.name), *arguments], cwd=tmp,
                                        text=True, capture_output=True, timeout=60)
                self.assertEqual((result.returncode, result.stdout), (2, ""))
                self.assertIn("usage: ci-resource-readings.sh COMMAND", result.stderr)
        # `--size` prints with the four commands alone, in order, and takes
        # their status; it reads no file and starts no sampler.
        with tempfile.TemporaryDirectory() as tmp:
            staged(tmp)
            tools = Path(tmp) / "tools"
            tools.mkdir()
            for tool in ("nproc", "free", "df", "swapon"):
                (tools / tool).write_text(f"#!{shutil.which('bash')}\necho \"{tool} $*\"\n")
                (tools / tool).chmod(0o755)
            (tools / "bash").symlink_to(shutil.which("bash"))
            result = subprocess.run([str(Path(tmp) / "scripts" / READINGS_SCRIPT.name), "--size"], cwd=tmp, text=True,
                                    capture_output=True, timeout=60, env={"PATH": str(tools)})
            self.assertEqual((result.returncode, result.stderr), (0, ""))
            printed = result.stdout.splitlines()
            self.assertEqual(printed[:2] + printed[-1:], ["nproc ", "free -m", "swapon --show"])
            self.assertIn(printed[2], ("df -h / /mnt", "df -h /"))
            self.assertEqual(len(printed), 4)

        # The script's shell ended alone, as by a signal sent to it only: the
        # shell ends by that signal, as a one-line step's command does, and
        # the sampler, which was not signalled, is gone about a second later.
        if not os.path.exists("/proc/self/stat"):
            return
        compile_name, command, _ = COMPILE_PATHS["rust-unit-build"]
        (step,) = [step for step in steps_of(workflow["jobs"]["rust-unit-build"]) if step.get("name") == compile_name]
        with tempfile.TemporaryDirectory() as tmp:
            staged(tmp)
            script = Path(tmp) / "step.sh"
            script.write_text(step["run"])
            env = {"PATH": os.environ.get("PATH", ""), "RUNNER_TEMP": str(Path(tmp) / "runner-temp"),
                   "CARGO_TARGET_DIR": str(Path(tmp) / "target"),
                   "STAND_IN_ARGUMENTS": str(Path(tmp) / "arguments"), "STAND_IN_SECONDS": "30", "STAND_IN_STATUS": "0"}
            marker = str(Path(tmp) / "scripts" / READINGS_SCRIPT.name).encode()

            def of_the_script():
                """(pid, parent) of the script's shell and of its sampler."""
                found = []
                for entry in os.listdir("/proc"):
                    if entry.isdigit():
                        try:
                            command_line = Path(f"/proc/{entry}/cmdline").read_bytes()
                            if marker in command_line.replace(b"./scripts/", str(Path(tmp) / "scripts").encode() + b"/"):
                                parent = int(Path(f"/proc/{entry}/stat").read_text().rsplit(")", 1)[1].split()[1])
                                found.append((int(entry), parent))
                        except (OSError, ValueError, IndexError):
                            pass
                return found

            shell = subprocess.Popen([shutil.which("bash"), "-e", str(script)], cwd=tmp, env=env, text=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
            try:
                self.assertRegex(shell.stdout.readline().rstrip("\n"), reading)
                running = of_the_script()
                self.assertGreaterEqual(len(running), 2, "the script's shell and its sampler")
                (own,) = [pid for pid, parent in running if parent == shell.pid]
                os.kill(own, signal.SIGTERM)
                self.assertEqual(shell.wait(timeout=10), 128 + signal.SIGTERM)
                deadline = time.monotonic() + 3
                while of_the_script() and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertEqual(of_the_script(), [], "the sampler left with its parent")
            finally:
                try:
                    os.killpg(shell.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                shell.stdout.close()
                shell.wait()

    def test_the_lib_jobs_build_in_a_step_of_their_own_and_then_only_run(self):
        """The extension-state and voice lib jobs compiled and ran their
        tests in one step: a runner lost in such a step leaves one step
        record and no word on whether the compile had ended. The compile is
        now a step of its own: the job's own command with `--no-run` and
        Cargo's `--timings`, the same package, features and stack, run under
        the resource readings script. The test step after it runs the same
        command without `--timings` and with its filters, so that it finds
        the build fresh and compiles nothing again. Before the compile one
        step prints the runner's size; it adds no swap. Nothing else of the
        jobs changes: the runner, the limit, the condition and the other
        steps."""
        workflow = load(CI_WORKFLOW)
        stack = "16777216"
        for name, (test_name, tests) in LIB_TEST_STEPS.items():
            with self.subTest(job=name):
                job = workflow["jobs"][name]
                self.assertEqual((job["runs-on"], job["timeout-minutes"]), ("ubuntu-24.04", 90))
                self.assertNotIn("continue-on-error", job)
                steps = steps_of(job)
                names = [step.get("name") for step in steps]
                build_name, build, _ = COMPILE_PATHS[name]
                at = names.index(build_name)
                self.assertEqual(names[at - 2:at + 3], [SIZE_PRINT, MEASURE_BEFORE, build_name, MEASURE_AFTER, test_name])
                size, compile_step, test_step = steps[at - 2], steps[at], steps[at + 2]
                self.assertEqual(size, {"name": SIZE_PRINT, "continue-on-error": True, "working-directory": ".",
                                        "run": f"{READINGS} --size"})
                for step in (compile_step, test_step):
                    self.assertEqual(sorted(step), ["env", "name", "run", "working-directory"])
                    self.assertEqual(step["working-directory"], ".")
                    self.assertEqual(step["env"], {"RUST_MIN_STACK": stack})
                self.assertEqual(compile_step["run"].strip(), f"{READINGS} {build}")
                self.assertEqual(test_step["run"].strip(), tests)
                # The same selection and features in the build and in every
                # test command: what follows them is `--timings --no-run`
                # for the build and the test's filter for a test.
                selection = build.removesuffix(" --timings --no-run")
                self.assertNotEqual(selection, build)
                self.assertRegex(selection, r"^\./scripts/repo-cargo test -p meerkat-meta --features [a-z,-]+ --lib$")
                for line in tests.splitlines():
                    self.assertTrue(line.startswith(selection + " "), line)
                    self.assertNotIn("--timings", line)
                    self.assertNotIn("--no-run", line)
                    self.assertRegex(line.removeprefix(selection + " "), r"^(-- )?[a-z_]+( [a-z_]+)*$")
                self.assertEqual(sum(line.count("--no-run") for _, step, line in run_lines(workflow)
                                     if step is compile_step), 1)
                # No swap, and no build-job or thread setting, in these jobs.
                for step in steps:
                    self.assertIsNone(re.search(r"swapon -|mkswap|fallocate|CARGO_BUILD_JOBS|--test-threads|RUST_TEST_THREADS",
                                                str(step.get("run") or "") + str(step.get("env") or "")), step.get("name"))

    def test_the_unit_build_adds_swap_before_its_compile(self):
        """rust-unit-build compiles the lib test harness once for the whole
        lane, and that compile reaches a hosted runner's memory. Before it,
        one step shows the runner's size and adds 12 GiB of swap, with the
        size and the guard of the root CI's step for its Meta rows: on /mnt
        when it is a mount point with 20 GiB free, else on / when it has 40
        GiB free, else no swap and a warning. It is the build's step alone:
        no leg and no other job adds swap. It stands before the statistics
        step that precedes the compile, changes no command of the job, and
        carries continue-on-error and nothing else: a failure here leaves the
        compile to run as it did without it, and nothing lets the step be
        skipped. The step's script is run here, as the runner runs a
        `shell: bash` step, with stand-ins for the commands that read the
        disk and the ones that need root."""
        workflow = load(CI_WORKFLOW)
        holders = [name for name, job in workflow["jobs"].items()
                   if any(step.get("name") == SWAP_PREFLIGHT for step in steps_of(job))]
        self.assertEqual(holders, ["rust-unit-build"])
        for name, job in workflow["jobs"].items():
            for step in steps_of(job):
                if step.get("name") != SWAP_PREFLIGHT:
                    self.assertIsNone(re.search(r"\b(swapon|mkswap|fallocate)\b", step.get("run") or ""),
                                      f"{name}: {step.get('name')}")
        steps = steps_of(workflow["jobs"]["rust-unit-build"])
        names = [step.get("name") for step in steps]
        self.assertEqual(names.count(SWAP_PREFLIGHT), 1)
        at = names.index(SWAP_PREFLIGHT)
        self.assertEqual(names[at + 1:at + 3], [MEASURE_BEFORE, "Build the lib test archive"])
        step = steps[at]
        self.assertEqual(sorted(step), ["continue-on-error", "name", "run", "shell"])
        self.assertIs(step["continue-on-error"], True)
        self.assertEqual(step["shell"], "bash")
        lines = [line.strip() for line in step["run"].splitlines() if line.strip()]
        self.assertEqual(lines[0], "set -euo pipefail")
        # From the swap file's name to the end: the root CI's lines, the
        # guard on free space and the size included.
        self.assertEqual(lines[lines.index("swapfile=/meta-unit-swapfile"):], [
            "swapfile=/meta-unit-swapfile",
            "need_gib=40",
            "if mountpoint -q /mnt; then",
            "mnt_avail_kib=\"$(df --output=avail -k /mnt | tail -n 1 | tr -d ' ')\"",
            "if ((mnt_avail_kib >= 20 * 1024 * 1024)); then",
            "swapfile=/mnt/meta-unit-swapfile",
            "need_gib=20",
            "fi",
            "fi",
            "avail_kib=\"$(df --output=avail -k \"$(dirname \"${swapfile}\")\" | tail -n 1 | tr -d ' ')\"",
            "if ((avail_kib < need_gib * 1024 * 1024)); then",
            'echo "::warning title=Meta unit row swap::under ${need_gib} GiB free for ${swapfile}: no swap added"',
            "exit 0",
            "fi",
            'echo "Adding 12 GiB of swap at ${swapfile}"',
            'sudo fallocate -l 12GiB "${swapfile}"',
            'sudo chmod 600 "${swapfile}"',
            'sudo mkswap "${swapfile}"',
            'sudo swapon "${swapfile}"',
            "show_resources",
        ])
        # Nothing of the step runs in the background or outlives it, and
        # only the four swap commands need root.
        self.assertFalse([line for line in lines if line.endswith("&") or "nohup" in line])
        self.assertEqual(sum(line.startswith("sudo ") for line in lines), 4)
        gib = 1024 * 1024
        on_root, on_mnt = "/meta-unit-swapfile", "/mnt/meta-unit-swapfile"
        cases = (
            ("/mnt is a mount point with 20 GiB free", True, 20 * gib, 5 * gib, "", 0, on_mnt),
            ("/mnt has a kibibyte less and / has 40 GiB", True, 20 * gib - 1, 40 * gib, "", 0, on_root),
            ("no /mnt mount point and / has 40 GiB", False, 0, 40 * gib, "", 0, on_root),
            ("no /mnt mount point and / has a kibibyte less than 40 GiB", False, 0, 40 * gib - 1, "", 0, None),
            ("/mnt and / are both short", True, 20 * gib - 1, 40 * gib - 1, "", 0, None),
            ("the swap file cannot be allocated", False, 0, 40 * gib, "fallocate", 1, on_root),
        )
        for name, mounted, mnt, root, fails, status, swapfile in cases:
            with self.subTest(case=name), tempfile.TemporaryDirectory() as temp:
                fake = Path(temp) / "bin"
                fake.mkdir()
                log = Path(temp) / "root-commands"
                stand_ins = {
                    "mountpoint": f"#!/bin/sh\nexit {0 if mounted else 1}\n",
                    "df": ("#!/bin/sh\n"
                           'if [ "$1" = "--output=avail" ]; then\n'
                           "  echo Avail\n"
                           f'  if [ "$3" = /mnt ]; then echo " {mnt}"; else echo " {root}"; fi\n'
                           "else\n"
                           "  echo 'Filesystem Size Used Avail Use% Mounted on'\n"
                           "fi\n"),
                    "swapon": "#!/bin/sh\nexit 0\n",
                    "sudo": ("#!/bin/sh\n"
                             f'echo "$*" >> "{log}"\n'
                             f'if [ "$1" = "{fails}" ]; then exit 1; fi\n'),
                }
                for command, text in stand_ins.items():
                    (fake / command).write_text(text)
                    (fake / command).chmod(0o755)
                result = subprocess.run(
                    ["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", step["run"]],
                    env={"PATH": f"{fake}:{os.environ['PATH']}"}, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
                ran = log.read_text().splitlines() if log.exists() else []
                warnings = [line for line in result.stdout.splitlines() if line.startswith("::warning ")]
                if swapfile is None:
                    self.assertEqual(ran, [])
                    self.assertEqual(warnings, [f"::warning title=Meta unit row swap::under 40 GiB free for {on_root}: "
                                                "no swap added"])
                    self.assertNotIn("Adding", result.stdout)
                elif fails:
                    # The script stops at the command that failed; the step's
                    # continue-on-error is what lets the job go on.
                    self.assertEqual(ran, [f"fallocate -l 12GiB {swapfile}"])
                else:
                    self.assertEqual(warnings, [])
                    self.assertIn(f"Adding 12 GiB of swap at {swapfile}\n", result.stdout)
                    self.assertEqual(ran, [f"fallocate -l 12GiB {swapfile}", f"chmod 600 {swapfile}",
                                           f"mkswap {swapfile}", f"swapon {swapfile}"])

    def test_the_examples_build_takes_the_bin_tests_profile_on_a_pull_request(self):
        """rust-examples builds every example with one wrapper line whose
        Cargo profile follows the event: `test` exactly on a pull request,
        where the same job then builds the bins' test harnesses in the test
        profile, and `dev` on every other event, where the examples build is
        the job's only build. The value is a variable of the step whose name
        is not one the registry cache hashes, so the job's compile
        environment stays the root CI's. The bin tests and
        meta-extension-state's suite keep their two lines and their
        pull-request-only condition."""
        workflow = load(CI_WORKFLOW)
        job = workflow["jobs"]["rust-examples"]
        steps = steps_of(job)
        names = [step.get("name") for step in steps]
        build = steps[names.index("Build every example")]
        variable = "EXAMPLES_PROFILE"
        self.assertEqual(build["working-directory"], ".")
        self.assertEqual(build["run"].strip(),
                         f'./scripts/repo-cargo build -p meerkat-meta --examples --profile "${{{variable}}}"')
        self.assertNotIn("if", build)
        self.assertNotIn("continue-on-error", build)
        selected = "${{ github.event_name == 'pull_request' && 'test' || 'dev' }}"
        self.assertEqual(build["env"], {variable: selected})
        self.assertIsNone(HASHED_ENV.fullmatch(variable))
        self.assertNotIn(variable, job.get("env") or {})
        self.assertNotIn(variable, workflow["env"])

        # The expression, read as GitHub reads `a == 'x' && 'y' || 'z'` (the
        # middle value is a non-empty string, so it is the result when the
        # comparison holds), for each event this workflow runs on.
        form = re.fullmatch(r"\$\{\{ github\.event_name == '([a-z_]+)' && '([a-z]+)' \|\| '([a-z]+)' \}\}", selected)
        self.assertIsNotNone(form)
        event, on_event, otherwise = form.groups()
        events = set(triggers(workflow))
        self.assertEqual(events, {"push", "pull_request", "workflow_dispatch"})
        profiles = {name: on_event if name == event else otherwise for name in events}
        self.assertEqual(profiles, {"pull_request": "test", "push": "dev", "workflow_dispatch": "dev"})

        # One examples build in the job, and no profile flag elsewhere in it.
        lines = [line for name, _, line in run_lines(workflow) if name == "rust-examples"]
        self.assertEqual([line for line in lines if "--examples" in line], [build["run"].strip()])
        self.assertEqual([line for line in lines if "--profile" in line or "--cargo-profile" in line],
                         [build["run"].strip()])

        # The pull request's second build and its tests, as they were: the
        # bins' harnesses in Cargo's test profile (no profile flag), then
        # meta-extension-state's suite, both only on a pull request.
        tests = steps[names.index("Bin tests and meta-extension-state's suite")]
        self.assertLess(names.index("Build every example"), names.index("Bin tests and meta-extension-state's suite"))
        self.assertEqual(tests["run"].strip().splitlines(), [
            "./scripts/repo-cargo nextest run -p meerkat-meta --bins --no-fail-fast",
            "./scripts/repo-cargo nextest run -p meta-extension-state --no-fail-fast",
        ])
        self.assertEqual(tests["working-directory"], ".")
        self.assertEqual(tests["env"], {"RUST_MIN_STACK": "16777216"})
        only_on = f"github.event_name == '{event}'"
        self.assertEqual(tests["if"], only_on)
        self.assertEqual(steps[names.index("Install nextest")]["if"], only_on)
        self.assertEqual(job["if"], "needs.changes.outputs.rust == 'true'")

        # What the choice rests on: the two profiles' settings differ in the
        # root manifest (the test profile sets its codegen units, dev does
        # not), so a dev build and a test build share no target unit.
        profile = tomllib.loads((MONOREPO / "Cargo.toml").read_text())["profile"]
        self.assertIn("codegen-units", profile["test"])
        self.assertNotIn("codegen-units", profile["dev"])

    def test_the_four_compile_paths_are_measured_in_steps_of_their_own(self):
        """The four jobs that compile meerkat-meta's long targets (the unit
        build, the extension-state lib, the voice lib and the console
        backends) are measured without touching what they do. The compile
        step keeps exactly its command, with Cargo's `--timings` on it: no
        suffix, pipe, condition or continue-on-error, so its exit stays the
        command's own. The unit build and the two lib builds run that
        command under the resource readings script, which prints readings
        and exits with the command's status
        (test_the_resource_readings_script_exits_with_its_commands_status);
        the console backends' build runs bare. The compiler
        cache's statistics are read in a step directly before and a step
        directly after it, the same bytes in the four jobs, and the timing
        reports are uploaded by the job's last step. Each added step carries
        continue-on-error: it can only keep its own failure from failing the
        job. No other job holds such a step."""
        workflow = load(CI_WORKFLOW)
        added = (MEASURE_BEFORE, MEASURE_AFTER, MEASURE_UPLOAD)
        measured = {name for name, job in workflow["jobs"].items()
                    if any(step.get("name") in added for step in steps_of(job))}
        self.assertEqual(measured, set(COMPILE_PATHS))
        before_runs, after_runs = set(), set()
        for name, (compile_name, command, selection) in COMPILE_PATHS.items():
            with self.subTest(job=name):
                steps = steps_of(workflow["jobs"][name])
                names = [step.get("name") for step in steps]
                for step_name in (compile_name, *added):
                    self.assertEqual(names.count(step_name), 1, step_name)
                at = names.index(compile_name)
                compile_step = steps[at]
                script = compile_step["run"].strip()
                self.assertEqual(script, f"{READINGS} {command}" if name in WITH_READINGS else command)
                self.assertEqual(len(command.splitlines()), 1)
                for key in ("continue-on-error", "if", "shell"):
                    self.assertNotIn(key, compile_step)
                for line in command.splitlines():
                    self.assertTrue(line.startswith("./scripts/repo-cargo "), line)
                    self.assertIsNone(re.search(r"[;|&<>`]|\$\(|\bset [+-]", line), line)
                self.assertEqual(command.count("--timings"), 1)
                self.assertIn(" --timings", command.splitlines()[0])
                before, after = steps[at - 1], steps[at + 1]
                self.assertEqual((before.get("name"), after.get("name")), (MEASURE_BEFORE, MEASURE_AFTER))
                for step in (before, after):
                    self.assertIs(step["continue-on-error"], True)
                    self.assertEqual(step["shell"], "bash")
                    self.assertNotIn("uses", step)
                self.assertNotIn("if", before)
                self.assertNotIn("env", before)
                self.assertEqual(after["if"], "${{ !cancelled() }}")
                # No key of the step's own carries the cache's name: the
                # runner writes a run step's `env:` into the log. The script
                # reads the name from the workflow's environment.
                self.assertEqual(after["env"], {"INSTR_SELECTION": selection, "INSTR_PACKAGE": "meerkat-meta"})
                # The report line names the job by its key; the run's job
                # listing names it by `name`: the two are the same word.
                self.assertEqual(workflow["jobs"][name]["name"], name)
                before_runs.add(before["run"])
                after_runs.add(after["run"])
                upload = steps[-1]
                self.assertEqual(upload.get("name"), MEASURE_UPLOAD)
                self.assertEqual(uses_of(upload), "actions/upload-artifact")
                self.assertIs(upload["continue-on-error"], True)
                self.assertEqual(upload["if"], "${{ !cancelled() }}")
                self.assertEqual(upload["with"], {
                    "name": "meta-ci-timings-${{ github.job }}-${{ github.run_id }}-${{ github.run_attempt }}",
                    "path": "${{ github.workspace }}/target/cargo-timings/",
                    "if-no-files-found": "ignore",
                    "retention-days": 7,
                })
                self.assertNotIn("strategy", workflow["jobs"][name])
        # One text for each added step, whichever job it stands in.
        self.assertEqual(len(before_runs), 1)
        self.assertEqual(len(after_runs), 1)
        (before_run,), (after_run,) = before_runs, after_runs
        read = 'sccache --show-stats --stats-format json > "${{RUNNER_TEMP}}/instr-sccache-{}.json" 2>/dev/null || true'
        self.assertEqual(before_run.strip(), read.format("before"))
        self.assertEqual(after_run.splitlines()[0], read.format("after"))
        # The statistics are only read (never zeroed), only as JSON into a
        # file (the text form prints the cache's location), and the report
        # names no location field.
        for text in (before_run, after_run):
            self.assertEqual(len(re.findall(r"\bsccache --", text)), 1)
            self.assertNotIn("--zero-stats", text)
            self.assertNotIn("show-adv-stats", text)
        for word in ("cache_location", "Cache location", "basedirs"):
            self.assertNotIn(word, after_run)
        # The cache's bucket name is the workflow's own variable and an input
        # of the cache's setup action; no step's `env:` repeats it.
        self.assertIn('os.environ.get("SCCACHE_GCS_BUCKET_NAME", "")', after_run)
        for name, job in workflow["jobs"].items():
            for step in steps_of(job):
                self.assertNotIn("SCCACHE_GCS_BUCKET_NAME", json.dumps(step.get("env") or {}), name)
        self.assertEqual(workflow["env"]["CARGO_TARGET_DIR"], "${{ github.workspace }}/target")

    def run_step(self, script: str, env: dict[str, str], path_dir: Path) -> subprocess.CompletedProcess:
        """A step's `run` text as the runner runs a `shell: bash` step, with
        only `path_dir` on PATH."""
        return subprocess.run(
            [shutil.which("bash"), "--noprofile", "--norc", "-eo", "pipefail", "-c", script],
            env={**env, "PATH": str(path_dir)}, capture_output=True, text=True,
        )

    def measured_after_step(self) -> str:
        steps = steps_of(load(CI_WORKFLOW)["jobs"]["rust-unit-build"])
        (step,) = [step for step in steps if step.get("name") == MEASURE_AFTER]
        return step["run"]

    @staticmethod
    def sccache_reading(requests: int, hits: int, misses: int, compiled_seconds: float, hit_seconds: float,
                        reasons: dict[str, int], not_compile: int) -> str:
        """The statistics as sccache prints them with --stats-format json: its
        server info, the counters under `stats`, and the cache's location."""
        def duration(seconds: float) -> dict:
            return {"secs": int(seconds), "nanos": int(round((seconds - int(seconds)) * 1e9))}
        return json.dumps({
            "stats": {
                "compile_requests": requests,
                "requests_unsupported_compiler": 0,
                "requests_not_compile": not_compile,
                "requests_not_cacheable": sum(reasons.values()),
                "requests_executed": hits + misses,
                "cache_errors": {"counts": {}, "adv_counts": {}},
                "cache_hits": {"counts": {"Rust": hits - 2, "C/C++": 2}, "adv_counts": {"rust": hits}},
                "cache_misses": {"counts": {"Rust": misses}, "adv_counts": {"rust": misses}},
                "cache_timeouts": 0,
                "cache_read_errors": 0,
                "non_cacheable_compilations": 0,
                "forced_recaches": 0,
                "cache_write_errors": 0,
                "cache_writes": 0,
                "cache_write_duration": duration(0),
                "cache_read_hit_duration": duration(hit_seconds),
                "compilations": misses,
                "compiler_write_duration": duration(compiled_seconds),
                "compile_fails": 0,
                "not_cached": reasons,
                "dist_compiles": {},
                "dist_errors": 0,
                "multi_level": None,
            },
            "cache_location": f"gcs, name: {FIXTURE_LOCATION_NAME}, prefix: sccache/fixture",
            "cache_size": None,
            "max_cache_size": None,
            "use_preprocessor_cache_mode": False,
            "version": "0.18.0",
            "basedirs": [f"/work/{FIXTURE_LOCATION_NAME}"],
        })

    def measured_scene(self, root: Path, *, tools: bool = True, before: bool = True,
                       after: str | None = None, reports: dict[str, str] | None = None) -> tuple[dict, Path]:
        """A runner's temporary directory, target directory and PATH for the
        report step: stand-ins for sccache, rustc and Cargo (or none), the
        reading taken before the compile (or none), and timing reports."""
        temp, target, tools_dir = root / "temp", root / "target", root / "bin"
        for folder in (temp, target, tools_dir):
            folder.mkdir()
        (tools_dir / "python3").symlink_to(os.path.realpath(sys.executable))
        if before:
            (temp / "instr-sccache-before.json").write_text(
                self.sccache_reading(10, 7, 3, 30.0, 1.5, {"crate-type": 1}, 1))
        if tools:
            answer = root / "sccache-answer.json"
            answer.write_text(after if after is not None
                              else self.sccache_reading(410, 399, 5, 75.5, 41.5,
                                                        {"crate-type": 3, "incremental": 1}, 6))
            stand_ins = {
                "sccache": 'case "$*" in --version) echo "sccache 0.18.0";; '
                           f'*) printf "%s\\n" "$(<"{answer}")";; esac',
                "rustc": 'echo "rustc 1.97.0 (fixture)"',
                "cargo": 'echo "cargo 1.97.0 (fixture)"',
            }
            for name, body in stand_ins.items():
                tool = tools_dir / name
                tool.write_text(f"#!{shutil.which('bash')}\n{body}\n")
                tool.chmod(0o755)
        if reports is not None:
            (target / "cargo-timings").mkdir()
            for name, text in reports.items():
                (target / "cargo-timings" / name).write_text(text)
        env = {
            "RUNNER_TEMP": str(temp), "CARGO_TARGET_DIR": str(target), "GITHUB_JOB": "rust-unit-build",
            "GITHUB_RUN_ID": "4242", "GITHUB_RUN_ATTEMPT": "1", "RUNNER_NAME": "fixture-runner",
            "ImageOS": "ubuntu24", "ImageVersion": "20261116.133.1",
            "INSTR_SELECTION": COMPILE_PATHS["rust-unit-build"][2], "INSTR_PACKAGE": "meerkat-meta",
            "SCCACHE_GCS_BUCKET_NAME": FIXTURE_LOCATION_NAME,
        }
        return env, tools_dir

    def test_the_compile_report_prints_deltas_and_never_the_cache_location(self):
        """The step after a compile prints, from two readings of the compiler
        cache's statistics, the difference of each counter and of the three
        time totals it keeps, the reasons of the calls it did not handle,
        and the selected package's lines of Cargo's timing report. The
        cache's location is in the statistics it reads and never in a line
        it prints; a timing report that holds the name is deleted before the
        upload, and said so by count."""
        # The report page's own forms, as Cargo 1.97.0 writes them: summary
        # cells and the unit data embedded in the page.
        unit_data = json.dumps([
            {"i": 0, "name": "serde", "version": "1.0.228", "mode": "todo", "target": "", "start": 0.5,
             "duration": 2.25, "features": ["default", "std"], "sections": None,
             "unblocked_units": [1], "unblocked_rmeta_units": []},
            {"i": 1, "name": "meerkat-meta", "version": "0.0.0", "mode": "todo",
             "target": ' meerkat_meta "lib" (test)', "start": 95.0, "duration": 1610.5, "features": [],
             "sections": None, "unblocked_units": [], "unblocked_rmeta_units": []},
        ])
        clean = (
            "<html><table>\n<tr>\n<td>Targets:</td><td>meerkat-meta 0.0.0 (lib)</td>\n</tr>\n"
            "<tr>\n<td>Profile:</td><td>test</td>\n</tr>\n<tr>\n<td>Fresh units:</td><td>390</td>\n</tr>\n"
            "<tr>\n<td>Dirty units:</td><td>2</td>\n</tr>\n<tr>\n<td>Total units:</td><td>392</td>\n</tr>\n"
            "<tr>\n<td>Total time:</td><td>1701.2s (28m 21.2s)</td>\n</tr>\n</table><script>\n"
            f"const UNIT_DATA = {unit_data};\nconst CONCURRENCY_DATA = [];\n</script></html>\n"
        )
        with tempfile.TemporaryDirectory() as root:
            env, tools_dir = self.measured_scene(Path(root), reports={
                "cargo-timing.html": clean,
                "cargo-timing-20261119T030000Z.html": clean.replace("<html>", f"<html>{FIXTURE_LOCATION_NAME}"),
            })
            result = self.run_step(self.measured_after_step(), env, tools_dir)
            kept = sorted(path.name for path in (Path(root) / "target" / "cargo-timings").iterdir())
            reading = json.loads((Path(root) / "temp" / "instr-sccache-after.json").read_text())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        printed = result.stdout + result.stderr
        # The reading the step took does hold the location; no line says it.
        self.assertIn(FIXTURE_LOCATION_NAME, reading["cache_location"])
        self.assertNotIn(FIXTURE_LOCATION_NAME, printed)
        for word in ("cache_location", "Cache location", "gcs", "basedirs"):
            self.assertNotIn(word, printed)
        lines = result.stdout.splitlines()
        self.assertTrue(lines and all(line.startswith("meta-ci-instr: ") for line in lines), result.stdout)
        said = [line.removeprefix("meta-ci-instr: ") for line in lines]
        self.assertEqual(said, [
            "job=rust-unit-build job_id=unknown (not read by this step; join the run's job listing by name) "
            'run=4242 attempt=1 runner=fixture-runner image=ubuntu24/20261116.133.1 '
            'selection="meerkat-meta lib test, default features"',
            'versions rustc="rustc 1.97.0 (fixture)" cargo="cargo 1.97.0 (fixture)" sccache="sccache 0.18.0" '
            "(nextest: its install step's own line, in the jobs that install it)",
            "sccache delta compile_requests=400 requests_executed=394 cache_hits=392 cache_misses=2 "
            "cache_errors=0 cache_timeouts=0 cache_read_errors=0 forced_recaches=0 cache_writes=0 "
            "cache_write_errors=0 compilations=2 compile_fails=0 non_cacheable_compilations=0",
            "sccache delta not handled: non_cacheable_calls=3 non_compilation_calls=5 unsupported_compiler_calls=0",
            "sccache delta non-cacheable reasons: crate-type=2 incremental=1",
            "sccache delta seconds: compiling=45.5 (2 compilations sccache ran) cache_read_hits=40.0 (392 hits) "
            "cache_writes=0.0 (0 writes)",
            "sccache time of non-cacheable calls: unknown (sccache keeps no time for a call it does not "
            "handle; see the Cargo timing lines)",
            f"Cargo timing reports: 1 file(s), {len(clean.encode())} bytes; 1 withheld (it held the cache's name)",
            'Cargo timing summary: profile="test" total_units="392" fresh_units="390" dirty_units="2" '
            'total_time="1701.2s (28m 21.2s)"',
            "Cargo timing unit: meerkat-meta 0.0.0 target=\"meerkat_meta 'lib' (test)\" mode=todo start=95.0 "
            "duration=1610.5 features=none",
        ])
        self.assertEqual(kept, ["cargo-timing.html"])

    def test_the_compile_report_says_unknown_and_never_fails(self):
        """What the report step cannot read it calls `unknown`, with no
        number in its place, and it ends with status 0 whatever is missing:
        a reading, the tools, the reports, their data, or Python itself."""
        def figures(stdout: str) -> list[str]:
            return [line for line in stdout.splitlines() if "sccache delta" in line]

        with tempfile.TemporaryDirectory() as root:
            env, tools_dir = self.measured_scene(Path(root), before=False, reports={})
            result = self.run_step(self.measured_after_step(), env, tools_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(figures(result.stdout), [])
        self.assertIn("meta-ci-instr: sccache statistics: unknown (the reading before the compile is missing "
                      "or unreadable)\n", result.stdout)
        self.assertIn("meta-ci-instr: Cargo timing reports: 0 file(s), 0 bytes\n", result.stdout)
        self.assertIn("meta-ci-instr: Cargo timing units: unknown (no report)\n", result.stdout)

        with tempfile.TemporaryDirectory() as root:
            env, tools_dir = self.measured_scene(Path(root), after="not statistics", reports={
                "cargo-timing.html": "<html>a report in a form this step does not know</html>\n"})
            result = self.run_step(self.measured_after_step(), env, tools_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(figures(result.stdout), [])
        self.assertIn("meta-ci-instr: sccache statistics: unknown (the reading after the compile is missing "
                      "or unreadable)\n", result.stdout)
        self.assertIn("meta-ci-instr: Cargo timing summary: profile=unknown total_units=unknown "
                      "fresh_units=unknown dirty_units=unknown total_time=unknown\n", result.stdout)
        self.assertIn("meta-ci-instr: Cargo timing units: unknown (report data not recognised)\n", result.stdout)

        # One counter gone from a reading: that figure is unknown, the others stand.
        with tempfile.TemporaryDirectory() as root:
            partial = json.loads(self.sccache_reading(410, 399, 5, 75.5, 41.5, {"crate-type": 3}, 6))
            del partial["stats"]["cache_misses"], partial["stats"]["compiler_write_duration"]
            env, tools_dir = self.measured_scene(Path(root), after=json.dumps(partial))
            result = self.run_step(self.measured_after_step(), env, tools_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(" cache_hits=392 cache_misses=unknown ", result.stdout)
        self.assertIn(" compiling=unknown (2 compilations sccache ran) ", result.stdout)
        self.assertIn("meta-ci-instr: Cargo timing reports: unknown (no report directory)\n", result.stdout)

        # A second reading lower than the first (another server answered) is
        # no measurement: those figures are unknown, never a negative number.
        with tempfile.TemporaryDirectory() as root:
            env, tools_dir = self.measured_scene(
                Path(root), after=self.sccache_reading(4, 2, 1, 5.0, 0.5, {}, 0))
            result = self.run_step(self.measured_after_step(), env, tools_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIsNone(re.search(r"=-\d", result.stdout), result.stdout)
        self.assertIn("sccache delta compile_requests=unknown requests_executed=unknown cache_hits=unknown "
                      "cache_misses=unknown cache_errors=0 ", result.stdout)
        self.assertIn("sccache delta non-cacheable reasons: crate-type=unknown\n", result.stdout)
        self.assertIn(" compiling=unknown (unknown compilations sccache ran) cache_read_hits=unknown "
                      "(unknown hits) cache_writes=0.0 (0 writes)\n", result.stdout)
        self.assertIn("meta-ci-instr: sccache delta: unknown where the second reading was lower than the "
                      "first or not a count (cache_hits, cache_misses, cache_read_hit_duration, compilations, "
                      "compile_requests, compiler_write_duration, not_cached, requests_executed, "
                      "requests_not_cacheable, requests_not_compile)\n", result.stdout)

        # No sccache, rustc or Cargo on PATH.
        with tempfile.TemporaryDirectory() as root:
            env, tools_dir = self.measured_scene(Path(root), tools=False)
            result = self.run_step(self.measured_after_step(), env, tools_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(figures(result.stdout), [])
        self.assertIn('versions rustc="unknown" cargo="unknown" sccache="unknown" ', result.stdout)
        self.assertIn("sccache statistics: unknown (the reading after the compile is missing or unreadable)\n",
                      result.stdout)

        # No Python either: one line, still status 0.
        with tempfile.TemporaryDirectory() as root:
            env, _ = self.measured_scene(Path(root), tools=False)
            empty = Path(root) / "empty"
            empty.mkdir()
            result = self.run_step(self.measured_after_step(), env, empty)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, "meta-ci-instr: unknown (the report script did not run)\n")

        # The step before the compile, with no sccache: nothing printed, status 0.
        steps = steps_of(load(CI_WORKFLOW)["jobs"]["rust-unit-build"])
        (before,) = [step for step in steps if step.get("name") == MEASURE_BEFORE]
        with tempfile.TemporaryDirectory() as root:
            env, _ = self.measured_scene(Path(root), tools=False, before=False)
            empty = Path(root) / "empty"
            empty.mkdir()
            result = self.run_step(before["run"], env, empty)
        self.assertEqual((result.returncode, result.stdout), (0, ""))

    def test_gate_requires_every_job_it_lists(self):
        """A job absent from `gate` can fail without failing the check suite.

        `needs` alone does not gate: with `if: always()` the gate runs
        regardless, so a job missing from the result checks is advisory only.
        Correspondence alone is not enough either: deleting a job from
        `needs` and from the checks together keeps the two lists in agreement,
        so the roster below is a floor, and the correspondence check runs on
        top of it.

        A job may be skipped only when `changes` set its lane (or `meta`, for
        the jobs every run that reaches Meta runs) to exactly "false"; the
        lanes and their jobs are the ones scripts/ci-changes.py classifies,
        and every other job must succeed.
        """
        workflow = load(CI_WORKFLOW)
        gate = workflow["jobs"]["gate"]
        self.assertEqual(gate["name"], "Meta gate")
        self.assertEqual(gate["if"], "always()")
        block = job_block("gate")
        listed = set(gate["needs"])
        must_gate = {"changes", "meta-checks", "test-typescript", "test-python", "console-backends", "console",
                     "console-browser", "console-fixtures", "console-acceptance", "console-perf",
                     "flow-editor-contracts", "flow-editor", "rust-unit-build", "rust-unit", "rust-examples",
                     "rust-conformance",
                     "rust-integration", "rust-features-check", "rust-extension-state",
                     "rust-extension-state-lib", "rust-voice", "rust-voice-lib"}
        missing = must_gate - listed
        self.assertFalse(missing, f"these jobs must gate the suite and are absent from `needs`: {sorted(missing)}")
        # warm-cache only warms the sccache bucket on a push that reaches no
        # Meta lane: a failure leaves a later pull request slower, never
        # wrong, and the root CI's downstream rows prove what it compiles.
        self.assertEqual(listed | {"gate", "warm-cache"}, set(workflow["jobs"]), "every other job gates")
        self.assertEqual(workflow["jobs"]["warm-cache"]["if"],
                         "github.event_name == 'push' && needs.changes.outputs.meta == 'false'")

        found = re.findall(r'(?m)^\s+check ([a-z-]+) "\$\{\{ needs\.\1\.result \}\}" (required|[a-z_]+)( pull_request)?$', block)
        checks = {name: mode for name, mode, _ in found}
        pull_request_only = {name for name, _, only in found if only}
        self.assertEqual(set(checks), listed, "every job in `needs` is checked exactly by name")
        self.assertEqual(pull_request_only, set(PULL_REQUEST_ONLY_JOBS),
                         "exactly the pull-request-only jobs may be skipped on a push")
        self.assertIn('"$3" != "required" && "$2" == "skipped" && "${lane_value}" == "false"', block)
        self.assertIn('local variable="LANE_${3^^}"', block)
        for lane in (*ci_changes.LANES, "meta"):
            self.assertIn(f"LANE_{lane.upper()}: ${{{{ needs.changes.outputs.{lane} }}}}\n", block)
        lanes = {}
        for name, mode in checks.items():
            condition = workflow["jobs"][name].get("if")
            if mode == "required":
                self.assertIsNone(condition, name)
                continue
            only = PULL_REQUEST_ONLY if name in pull_request_only else ""
            self.assertEqual(condition, f"needs.changes.outputs.{mode} == 'true'{only}", name)
            needs = workflow["jobs"][name]["needs"]
            self.assertIn("changes", [needs] if isinstance(needs, str) else needs, name)
            lanes.setdefault(mode, set()).add(name)
        self.assertEqual(lanes.pop("meta"), {"meta-checks", "test-typescript"})
        # The workflow's lanes are exactly the classifier's, job for job.
        self.assertEqual(lanes, {lane: set(jobs) for lane, jobs in ci_changes.LANE_JOBS.items() if jobs})
        self.assertEqual(checks["changes"], "required")

    def run_gate(self, results: dict[str, str], lanes: dict[str, str],
                 event: str = "pull_request") -> subprocess.CompletedProcess:
        """Run the gate's result check with job results substituted the way
        GitHub renders them and the lane outputs and event in its
        environment."""
        lines = job_block("gate").splitlines()
        check_at = lines.index("      - name: Check job results")
        start = lines.index("        run: |", check_at) + 1
        end = next(index for index in range(start, len(lines)) if lines[index].startswith("      - name: "))
        body = "\n".join(line[10:] for line in lines[start:end] if line.startswith("          ") or not line)
        script = re.sub(
            r"\$\{\{ needs\.([a-z-]+)\.result \}\}",
            lambda match: results.get(match.group(1), "success"),
            body,
        )
        env = {key: value for key, value in os.environ.items() if not key.startswith("LANE_")}
        env.update({f"LANE_{lane.upper()}": value for lane, value in lanes.items()})
        env["EVENT"] = event
        return subprocess.run(["bash", "-e", "-c", script], env=env, capture_output=True, text=True)

    def test_gate_accepts_a_skip_only_for_a_lane_classified_false(self):
        names = (*ci_changes.LANES, "meta")
        every = {lane: "true" for lane in names}
        nothing = {lane: "false" for lane in names}
        docs_only = {**nothing, "meta": "true"}
        console_only = {**nothing, "meta": "true", "console": "true", "console_perf": "true", "flow_editor": "true"}
        lane_jobs = {job for jobs in ci_changes.LANE_JOBS.values() for job in jobs}
        all_skipped = {job: "skipped" for job in lane_jobs | {"meta-checks", "test-typescript"}}
        lanes_skipped = {job: "skipped" for job in lane_jobs}
        cases = (
            ("all green", {}, every, 0),
            ("nothing of Meta's changed", all_skipped, nothing, 0),
            ("a docs-only change of Meta's", lanes_skipped, docs_only, 0),
            ("the python lane skipped when classified false", {"test-python": "skipped"}, console_only, 0),
            ("meta-checks skipped while Meta is reached", {"meta-checks": "skipped"}, docs_only, 1),
            ("the unit legs skipped behind a failed build",
             {"rust-unit-build": "failure", "rust-unit": "skipped"}, every, 1),
            ("the unit legs skipped behind a build that passed", {"rust-unit": "skipped"}, every, 1),
            ("the browser legs skipped behind failed backends",
             {"console-backends": "failure", "console-browser": "skipped", "console": "skipped"}, every, 1),
            ("a skipped job of a running lane", {"flow-editor-contracts": "skipped"}, console_only, 1),
            ("a failed job in a skipped lane", {"test-python": "failure"}, console_only, 1),
            ("lanes missing: classification failed", all_skipped, {}, 1),
            ("lanes empty", all_skipped, {lane: "" for lane in names}, 1),
            ("changes failed", {"changes": "failure"}, every, 1),
            ("a cancelled leg", {"console-browser": "cancelled"}, every, 1),
        )
        for name, results, lanes, status in cases:
            with self.subTest(case=name):
                result = self.run_gate(results, lanes)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
        # The pull-request-only jobs: skipped on a push or a dispatch (the
        # root CI runs their tests there), required on a pull request.
        pr_only_skipped = {job: "skipped" for job in PULL_REQUEST_ONLY_JOBS}
        for event, status in (("push", 0), ("workflow_dispatch", 0), ("pull_request", 1)):
            with self.subTest(event=event):
                result = self.run_gate(pr_only_skipped, every, event)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)
        # Without a classification even a push may not skip them.
        result = self.run_gate({**all_skipped, **pr_only_skipped}, {}, "push")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        # A pull-request-only job that failed fails a push too.
        result = self.run_gate({"rust-unit": "failure"}, every, "push")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_every_job_builds_the_commit_its_event_names(self):
        """No dispatch input and no checkout ref can point a job at another
        commit, and no step fetches or moves to one: reading the base
        commit's classifier (git show) and its first parent's id (git
        rev-parse) are the only git uses. Pushes run on main and the release
        branches; any other branch gets this workflow through its pull
        request (or a dispatch), with no path filter, so the required check
        always reports."""
        workflow = load(CI_WORKFLOW)
        on = triggers(workflow)
        self.assertEqual(on["push"], {"branches": ["main", "release/**"]})
        self.assertNotIn("paths", on["pull_request"])
        self.assertNotIn("paths-ignore", on["pull_request"])
        self.assertEqual(on["workflow_dispatch"], {})
        source = CI_WORKFLOW.read_text()
        uses = re.findall(r"(?m)^\s*-?\s*uses:\s*actions/checkout@\S+", source)
        checkouts = re.findall(r"(?m)^ *- uses: actions/checkout@\S+(?: #[^\n]*)?\n((?: {8,}.*\n)*)", source)
        self.assertTrue(uses)
        self.assertEqual(len(checkouts), len(uses))
        for settings in checkouts:
            self.assertNotIn("ref:", settings)
            self.assertNotIn("repository:", settings)
        code = "\n".join(line for line in source.splitlines() if not line.strip().startswith("#"))
        self.assertEqual(re.findall(r"\bgit\s+(fetch|checkout|switch|reset|pull|clone|worktree|restore)\b", code), [])
        self.assertEqual(sorted(set(re.findall(r"\bgit\s+([a-z-]+)", code))), ["rev-parse", "show"])

    def test_the_no_redirect_check_sees_every_checkout_and_git_use(self):
        probe = """
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683
        with:
          ref: ${{ github.event.inputs.sha }}
      - run: git fetch origin pull/1/head && git checkout FETCH_HEAD
"""
        self.assertEqual(len(re.findall(r"(?m)^ *- uses: actions/checkout@\S+(?: #[^\n]*)?\n((?: {8,}.*\n)*)", probe)), 1)
        self.assertIn("ref:", re.findall(r"(?m)^ *- uses: actions/checkout@\S+(?: #[^\n]*)?\n((?: {8,}.*\n)*)", probe)[0])
        self.assertEqual(re.findall(r"\bgit\s+(fetch|checkout|switch|reset|pull|clone|worktree|restore)\b", probe),
                         ["fetch", "checkout"])

    def test_changes_job_classifies_the_run_and_fails_closed(self):
        block = job_block("changes")
        self.assertIn("fetch-depth: 2", block)
        # Every lane is an output, and so is meta.
        outputs = re.findall(r"(?m)^      ([a-z_]+): \$\{\{ steps\.classify\.outputs\.\1 \}\}$", block)
        self.assertEqual(outputs, [*ci_changes.LANES, "meta"])
        self.assertIn(f'          lanes="{" ".join(ci_changes.LANES)} meta"\n', block)
        self.assertIn("git show HEAD^1:meta/scripts/ci-changes.py", block)
        self.assertIn('--monorepo --event "${EVENT}"', block)
        self.assertNotIn("python3 scripts/ci-changes.py", block)
        self.assertNotIn("    if:", block)
        self.assertIn("- run: python3 scripts/test_ci_changes.py\n", job_block("meta-checks"))
        # Every other job waits for it and skips when Meta is not reached.
        workflow = load(CI_WORKFLOW)
        for name, job in workflow["jobs"].items():
            if name in ("changes", "gate", "warm-cache"):
                continue
            with self.subTest(job=name):
                only = re.escape(PULL_REQUEST_ONLY) if name in PULL_REQUEST_ONLY_JOBS else ""
                self.assertRegex(job["if"], rf"^needs\.changes\.outputs\.[a-z_]+ == 'true'{only}$")

    def run_classify_step(self, base_classifier: str | None, *, head_classifier: str | None = None,
                          event: str = "pull_request", before: str = "parent") -> dict[str, str]:
        """Run the classify step in a monorepo whose base commit holds
        `base_classifier` at meta/scripts/ci-changes.py (None: no classifier
        at all) and whose head may change it, from meta/ as the workflow
        does; return the step's GITHUB_OUTPUT as a mapping. `before` is the
        push's previous tip: "parent" names HEAD's first parent."""
        lines = job_block("changes").splitlines()
        start = lines.index("      - id: classify")
        run = lines.index("        run: |", start)
        body = []
        for line in lines[run + 1:]:
            if line and not line.startswith("          "):
                break
            body.append(line[10:] if line else "")
        env = {key: value for key, value in os.environ.items() if not key.startswith("GIT")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_AUTHOR_NAME="ci", GIT_AUTHOR_EMAIL="ci@example.invalid",
                   GIT_COMMITTER_NAME="ci", GIT_COMMITTER_EMAIL="ci@example.invalid")
        with tempfile.TemporaryDirectory() as tmp:
            repo, temp = Path(tmp) / "repo", Path(tmp) / "runner-temp"
            (repo / "meta/scripts").mkdir(parents=True)
            temp.mkdir()
            output = Path(tmp) / "output"

            def git(*args) -> str:
                return subprocess.run(["git", *args], cwd=repo, env=env, check=True, capture_output=True,
                                      text=True).stdout.strip()

            git("init", "-q")
            for index, classifier in enumerate(
                    (base_classifier, head_classifier if head_classifier is not None else base_classifier)):
                if classifier is not None:
                    (repo / "meta/scripts/ci-changes.py").write_text(classifier)
                (repo / "meta/touched.txt").write_text(f"commit {index}\n")
                git("add", "-A")
                git("commit", "-q", "-m", "tree")
            previous = git("rev-parse", "HEAD^1") if before == "parent" else before
            env.update(EVENT=event, BEFORE=previous, RUNNER_TEMP=str(temp), GITHUB_OUTPUT=str(output))
            subprocess.run(["bash", "-e", "-c", "\n".join(body)], cwd=repo / "meta", env=env, check=True,
                           capture_output=True, text=True)
            return dict(line.split("=", 1) for line in output.read_text().splitlines())

    def test_the_base_commits_classifier_decides_and_fails_closed(self):
        names = (*ci_changes.LANES, "meta")
        every = {lane: "true" for lane in names}
        nothing = {lane: "false" for lane in names}

        def printing(values: dict[str, str]) -> str:
            text = "".join(f"{key}={value}\\n" for key, value in values.items())
            return ("import sys\nargs = sys.argv\nassert '--monorepo' in args\n"
                    "open(args[args.index('--github-output') + 1], 'a')"
                    f".write('{text}')\n")

        # A dispatch or schedule runs every lane even if a classifier says none.
        for event in ("workflow_dispatch", "schedule"):
            with self.subTest(event=event):
                self.assertEqual(self.run_classify_step(printing(nothing), event=event), every)
        console_only = {**nothing, "console": "true", "meta": "true"}
        self.assertEqual(self.run_classify_step(printing(console_only)), console_only)
        # The pull request's own classifier never decides its lanes.
        self.assertEqual(self.run_classify_step(printing(console_only), head_classifier=printing(nothing)),
                         console_only)
        # Nothing of Meta's changed: meta=false and no lane, on a push too.
        self.assertEqual(self.run_classify_step(printing(nothing)), nothing)
        self.assertEqual(self.run_classify_step(printing(nothing), event="push"), nothing)
        # A push that reaches Meta runs every lane, whatever lanes were printed.
        self.assertEqual(self.run_classify_step(printing(console_only), event="push"), every)
        # A push of several commits or of a new ref runs every lane unasked.
        for before in ("0" * 40, "1" * 40, ""):
            with self.subTest(before=before):
                self.assertEqual(self.run_classify_step(printing(nothing), event="push", before=before), every)
        # A classifier from before --monorepo prints no meta line: every lane.
        lanes_only = {lane: "false" for lane in ci_changes.LANES}
        for case, base in (("predates --monorepo", printing(lanes_only)),
                           ("misses a lane", printing({k: v for k, v in console_only.items() if k != "python"})),
                           ("fails", "raise SystemExit(3)\n"),
                           ("is absent", None)):
            with self.subTest(base=case):
                self.assertEqual(self.run_classify_step(base), every)


class InstallerWorkflowTests(unittest.TestCase):
    """meta-installer.yml, Meta's installer suite at the monorepo's root."""

    def test_it_runs_for_meta_installer_inputs_and_dispatch_only(self):
        workflow = load(INSTALLER_WORKFLOW)
        on = triggers(workflow)
        self.assertEqual(sorted(on), ["pull_request", "workflow_dispatch"])
        paths = on["pull_request"]["paths"]
        self.assertIn(".github/workflows/meta-installer.yml", paths)
        self.assertEqual([path for path in paths if not path.startswith(ci_changes.MONOREPO_ROOT)],
                         [".github/workflows/meta-installer.yml"])
        self.assertEqual(workflow["defaults"]["run"]["working-directory"], "meta")
        self.assertEqual(workflow["permissions"], {"contents": "read"})

    def test_it_keeps_the_jobs_and_artifacts_the_installer_release_binds(self):
        """scripts/installer-release.py binds the frontend and backend jobs of a
        green dispatched run and their installer-toolchain-* artifacts; the
        backend job checks every shard's plan."""
        spec = importlib.util.spec_from_file_location("installer_release", ROOT / "scripts/installer-release.py")
        release = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(release)
        workflow = load(INSTALLER_WORKFLOW)
        self.assertLessEqual(set(release.EVIDENCE_JOBS), set(workflow["jobs"]))
        self.assertEqual(workflow["jobs"]["backend"]["needs"], ["backend-shard", "agent-team-live"])
        source = INSTALLER_WORKFLOW.read_text()
        self.assertEqual(source.count("name: installer-toolchain-${{ github.job }}-"), 3)
        self.assertEqual(release.INSTALLER_CI, ".github/workflows/meta-installer.yml")
        # Artifact paths are the workspace's, not meta/'s: each names meta/
        # or the runner's temp directory.
        for name, job in workflow["jobs"].items():
            for step in steps_of(job):
                if uses_of(step) in ("actions/upload-artifact", "actions/download-artifact"):
                    with self.subTest(job=name, step=step.get("name")):
                        for path in str(step["with"]["path"]).splitlines():
                            self.assertRegex(path, r"^(meta/|\$\{\{ runner\.temp \}\})")

    def test_the_root_installer_workflows_use_neutral_names_and_workspace_paths(self):
        """Both installer workflows run from the repository root: their file,
        job, artifact and step names never name the installer's product, and
        every path an action resolves from the workspace (artifacts,
        attestations) names meta/ or the runner's temp directory."""
        for path in (INSTALLER_WORKFLOW, INSTALLER_RELEASE_WORKFLOW):
            with self.subTest(workflow=path.name):
                self.assertNotIn("forge", path.read_text().lower())
                workflow = load(path)
                self.assertEqual(workflow["defaults"]["run"]["working-directory"], "meta")
                for name, job in workflow["jobs"].items():
                    for step in steps_of(job):
                        action = uses_of(step)
                        key = {"actions/upload-artifact": "path", "actions/download-artifact": "path",
                               "actions/attest-build-provenance": "subject-path"}.get(action)
                        if key is None:
                            continue
                        for value in str(step["with"][key]).splitlines():
                            self.assertRegex(value, r"^(meta/|\$\{\{ runner\.temp \}\})", f"{name}: {step.get('name')}")


if __name__ == "__main__":
    unittest.main()
