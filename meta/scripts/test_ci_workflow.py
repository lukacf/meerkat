#!/usr/bin/env python3
"""Focused contracts for Meta's CI workflows: the monorepo's
.github/workflows/meta-ci.yml (pull requests, pushes, dispatch), its
meta-installer.yml and meta-e2e-live.yml, and the workflows still kept under
meta/.github."""

import importlib.util
import json
import os
import re
import subprocess
import tempfile
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
SETUP_SCCACHE = "./.github/actions/setup-sccache-ci"


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text())


def triggers(workflow: dict) -> dict:
    # YAML 1.1 reads the bare key `on` as True.
    return workflow.get("on", workflow.get(True))


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
        setup_order = [SETUP_RUST, SETUP_SCCACHE, "rui314/setup-mold", "Swatinem/rust-cache"]
        self.assertEqual([use for use in reference_uses if use in setup_order], setup_order)
        jobs = rust_jobs(meta)
        self.assertEqual(sorted(jobs), sorted(["test-python", "console-backends", "console-fixtures",
                                               "console-acceptance", "flow-editor"]))
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
        for meerkat-meta alone: a step in meta/ reaches it through make with
        CARGO=../scripts/repo-cargo. No step names the workspace, so no Meta
        job compiles Core's or Connect's packages, and the planner's product
        rows stay the only workspace lanes."""
        commands = []
        for path in (CI_WORKFLOW, E2E_LIVE_WORKFLOW):
            workflow = load(path)
            default = ((workflow.get("defaults") or {}).get("run") or {}).get("working-directory")
            self.assertEqual(default, "meta", path.name)
            for _, step, line in run_lines(workflow):
                self.assertNotIn("--workspace", line, path.name)
                self.assertIsNone(re.search(r"(^|[\s;&|(])cargo\s", line), f"{path.name}: raw cargo in {line}")
                if "repo-cargo" not in line:
                    continue
                commands.append(line)
                if line.startswith("./scripts/repo-cargo "):
                    self.assertEqual(step.get("working-directory"), ".", line)
                    self.assertIn(" -p meerkat-meta ", f"{line} ", line)
                else:
                    self.assertRegex(line, r"^make (test-python|e2e-live) CARGO=\.\./scripts/repo-cargo$")
        self.assertEqual(sum(line.startswith("make ") for line in commands), 2)
        self.assertEqual(len(commands), 7, commands)
        self.assertNotIn("repo-cargo", INSTALLER_WORKFLOW.read_text())

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
                else:
                    self.assertIsNone(permissions)
                for step in steps_of(job):
                    if uses_of(step) == "actions/checkout":
                        self.assertIs((step.get("with") or {}).get("persist-credentials"), False)
                        self.assertNotIn("ref", step.get("with") or {})
                        self.assertNotIn("repository", step.get("with") or {})

    def test_the_root_files_meta_ci_runs_reach_every_lane(self):
        """A change to a root file every compile job runs (the setup actions,
        the script they run, the root wrapper) runs every lane, as the
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
                           + " ".join(f"--example {name}" for name in examples))
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
            "- run: node console/browser-e2e.cjs --shard=${{ matrix.shard }}/6\n",
            "name: console-browser-${{ github.run_id }}-${{ matrix.shard }}-${{ github.run_attempt }}\n",
        ]
        positions = [browser.index(marker) for marker in order]
        self.assertEqual(positions, sorted(positions))
        # Six legs, each dealt its own scenarios by the registry's
        # --shard=i/6; the matrix and the shard count must agree. MCP Apps
        # run once, in leg 1, on its evidence path under meta/.
        legs = re.search(r"(?m)^        shard: \[([0-9, ]+)\]$", browser).group(1).split(", ")
        self.assertEqual(legs, [str(index) for index in range(1, 7)])
        self.assertIn(f"--shard=${{{{ matrix.shard }}}}/{len(legs)}\n", browser)
        self.assertIn("      fail-fast: false\n", browser)
        self.assertEqual(browser.count("        if: matrix.shard == 1\n"), 3)
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
                     "flow-editor-contracts", "flow-editor"}
        missing = must_gate - listed
        self.assertFalse(missing, f"these jobs must gate the suite and are absent from `needs`: {sorted(missing)}")
        self.assertEqual(listed | {"gate"}, set(workflow["jobs"]), "every job gates")

        checks = dict(re.findall(r'(?m)^\s+check ([a-z-]+) "\$\{\{ needs\.\1\.result \}\}" (required|[a-z_]+)$', block))
        self.assertEqual(set(checks), listed, "every job in `needs` is checked exactly by name")
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
            self.assertEqual(condition, f"needs.changes.outputs.{mode} == 'true'", name)
            needs = workflow["jobs"][name]["needs"]
            self.assertIn("changes", [needs] if isinstance(needs, str) else needs, name)
            lanes.setdefault(mode, set()).add(name)
        self.assertEqual(lanes.pop("meta"), {"meta-checks", "test-typescript"})
        # The workflow's lanes are exactly the classifier's, job for job.
        self.assertEqual(lanes, {lane: set(jobs) for lane, jobs in ci_changes.LANE_JOBS.items() if jobs})
        self.assertEqual(checks["changes"], "required")

    def run_gate(self, results: dict[str, str], lanes: dict[str, str]) -> subprocess.CompletedProcess:
        """Run the gate step's shell with job results substituted the way
        GitHub renders them and the lane outputs in its environment."""
        lines = job_block("gate").splitlines()
        start = lines.index("        run: |") + 1
        body = "\n".join(line[10:] for line in lines[start:] if line.startswith("          ") or not line)
        script = re.sub(
            r"\$\{\{ needs\.([a-z-]+)\.result \}\}",
            lambda match: results.get(match.group(1), "success"),
            body,
        )
        env = {key: value for key, value in os.environ.items() if not key.startswith("LANE_")}
        env.update({f"LANE_{lane.upper()}": value for lane, value in lanes.items()})
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
            if name in ("changes", "gate"):
                continue
            with self.subTest(job=name):
                self.assertRegex(job["if"], r"^needs\.changes\.outputs\.[a-z_]+ == 'true'$")

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
