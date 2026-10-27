#!/usr/bin/env python3
"""Focused contracts for the pull-request CI workflow."""

import fnmatch
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
import re
import tomllib
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[1]
CI_WORKFLOW = ROOT / ".github/workflows/ci.yml"
# dtolnay/rust-toolchain keeps one branch per toolchain, and each branch's
# commit hard-codes its version, so a toolchain is pinned to its branch's
# commit. A toolchain bump re-pins it here and in the workflows.
RUST_TOOLCHAIN_ACTION = {"1.97.0": "f08ac54821369508305fcb9e0121ec6e30ae9ada"}
_CI_CHANGES_SPEC = importlib.util.spec_from_file_location("ci_changes", ROOT / "scripts/ci-changes.py")
ci_changes = importlib.util.module_from_spec(_CI_CHANGES_SPEC)
_CI_CHANGES_SPEC.loader.exec_module(ci_changes)

# The target kinds `cargo test` tests by default. Examples are only built,
# and benches and build scripts are left out, unless a manifest sets their
# test flag.
TESTED_BY_DEFAULT = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "bin", "test"}
NOT_TESTED_BY_DEFAULT = {"example", "bench", "custom-build"}


def cargo_workspace_members(root: Path) -> list[dict]:
    """The workspace members as Cargo itself reads them: listed, glob and
    implicit path members alike, with their targets in any valid TOML
    spelling. `cargo metadata --no-deps` resolves nothing and needs no network."""
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--offline", "--format-version", "1"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    members = set(metadata["workspace_members"])
    return [package for package in metadata["packages"] if package["id"] in members]


def split_coverage_problems(packages: list[dict], patterns: list[str]) -> list[str]:
    """Why the CI test jobs would not run what `cargo test` runs by default.

    `test` runs --lib --bins and builds --examples, and each integration
    half runs --test <pattern>. Their union is cargo's default selection
    only while every target keeps its kind's default test flag and has no
    required features, and every integration test target matches exactly
    one pattern. Turning autotests off would silently drop the files under
    tests/ from every run, so it is refused too.
    """
    problems = []
    for package in packages:
        manifest = tomllib.loads(Path(package["manifest_path"]).read_text())
        if manifest.get("package", {}).get("autotests") is False:
            problems.append(f"{package['name']}: autotests = false")
        for target in package["targets"]:
            kinds = set(target["kind"])
            label = f"{package['name']} {'/'.join(target['kind'])} {target['name']}"
            if kinds <= TESTED_BY_DEFAULT:
                default = True
            elif kinds <= NOT_TESTED_BY_DEFAULT:
                default = False
            else:
                problems.append(f"{label}: unknown target kind")
                continue
            if target.get("test") is not default:
                problems.append(f"{label}: test flag {target.get('test')}, default {default}")
            if target.get("required-features"):
                problems.append(f"{label}: required-features {target['required-features']}")
            if kinds == {"test"}:
                matched = [pattern for pattern in patterns if fnmatch.fnmatchcase(target["name"], pattern)]
                if len(matched) != 1:
                    problems.append(f"{label}: matches {matched}")
    return problems


def job_block(name: str) -> str:
    """Return exactly one top-level job, bounded by the next job header."""
    lines = CI_WORKFLOW.read_text().splitlines()
    start = lines.index(f"  {name}:")
    for offset, line in enumerate(lines[start + 1 :], start=start + 1):
        if line.startswith("  ") and not line.startswith("   ") and line.endswith(":"):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


# A runner label that names its image. A -latest label moves to a new image on
# GitHub's schedule (ubuntu-latest left 24.04 on 2026-10-19), and with it the
# toolchains, glibc and preinstalled tools every job reads.
PINNED_RUNNER = re.compile(r"^(ubuntu-\d\d\.\d\d(-arm)?|macos-\d+|windows-\d{4}(-vs\d{4})?)$")
# The one runs-on expression a job may use, and the one matrix it may take the
# labels from without naming them inline: the release build legs, which
# test_the_release_build_legs_run_on_pinned_runner_images reads.
MATRIX_RUNS_ON = "${{ matrix.runs-on }}"
RELEASE_BUILD_MATRIX = "${{ fromJSON(needs.resolve_release.outputs.build_matrix) }}"


def runner_problems(root: Path) -> tuple[list[str], list[str]]:
    """Every workflow job's runner labels, read as YAML, and why any of them
    floats or cannot be read. A runs-on that is not one inline label (a block
    list, a flow list or a mapping) is refused, never skipped."""
    labels, problems = [], []
    for path in sorted((root / ".github" / "workflows").glob("*.y*ml")):
        workflow = yaml.safe_load(path.read_text()) or {}
        for name, job in (workflow.get("jobs") or {}).items():
            if not isinstance(job, dict) or "runs-on" not in job:
                continue
            where = f"{path.relative_to(root)} job {name}"
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
# Image references accepted without a digest, and why. The self-hosted CI
# platform image lives in a private Artifact Registry repository that neither
# Renovate nor this check can read; it is matched by registry, repository and
# tag (any project).
UNPINNED_IMAGES = {
    re.compile(r"docker://europe-west1-docker\.pkg\.dev/[^/]+/meerkat-ci/meerkat-ci-rust:1\.94\.0"): (
        "a private registry neither Renovate nor this check can read: its digest comes from someone "
        "with read access to that registry"
    ),
}
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
    """The repository's tracked files, or every file of a tree with no git."""
    if (root / ".git").exists():
        listed = subprocess.run(["git", "ls-files", "-z"], cwd=root, check=True, capture_output=True, text=True)
        return [name for name in listed.stdout.split("\0") if name]
    return sorted(str(path.relative_to(root)) for path in root.rglob("*") if path.is_file())


def image_problems(root: Path) -> tuple[list[tuple[str, str]], list[str]]:
    """Every container image CI names, with where it is named, and why any of
    them floats: a `container:` or `image:` value in a .github YAML file, a
    `docker://` reference on any line of a .github file (a `uses:` action or
    a run script), one in tools/bazel's BUILD, BUILD.bazel and .bzl files or
    .bazelrc, and the FROM lines of every tracked Dockerfile or Containerfile
    except the installer's launch templates (USER_TEMPLATE_DOCKERFILES). Each must carry
    an @sha256: digest or be listed in UNPINNED_IMAGES."""
    found, problems = [], []

    def check(where: str, image: str) -> None:
        found.append((where, image))
        if not DIGEST.search(image) and not any(pattern.fullmatch(image) for pattern in UNPINNED_IMAGES):
            problems.append(f"{where}: {image} names its image without an @sha256: digest")

    for path in sorted(path for path in (root / ".github").rglob("*") if path.is_file()):
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
    bazel = sorted(path for path in (root / "tools" / "bazel").rglob("*")
                   if path.name in ("BUILD.bazel", "BUILD") or path.suffix == ".bzl")
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
    def test_ci_and_release_use_the_repository_toolchain(self):
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
        version = toolchain["channel"]
        self.assertRegex(version, r"^\d+\.\d+\.\d+$")
        for name in ("ci.yml", "release.yml", "e2e-live.yml"):
            source = (ROOT / ".github/workflows" / name).read_text()
            installs = re.findall(r"uses:\s*dtolnay/rust-toolchain@([0-9a-f]{40}) # (\S+)\n", source)
            self.assertTrue(installs, f"{name} must install its build toolchain")
            self.assertEqual(set(installs), {(RUST_TOOLCHAIN_ACTION[version], version)}, name)
            self.assertEqual(len(installs), source.count("dtolnay/rust-toolchain@"), name)

    def test_every_external_action_is_pinned_to_a_full_commit_sha(self):
        """The repository's Actions policy refuses an action that is not
        pinned to a full-length commit SHA. Each pin names the ref it was taken
        from in a trailing comment, and one ref pins one commit everywhere."""
        pins = {}
        for path in sorted((ROOT / ".github").rglob("*.y*ml")):
            for number, line in enumerate(path.read_text().splitlines(), 1):
                found = re.match(r"\s*(?:-\s+)?uses:\s*(\S+)(.*)$", line)
                if not found or found.group(1).startswith(("./", "docker://")):
                    continue
                where = f"{path.relative_to(ROOT)}:{number}"
                pinned = re.fullmatch(r"([\w.-]+/[\w./-]+)@([0-9a-f]{40})", found.group(1))
                self.assertIsNotNone(pinned, f"{where}: {found.group(1)} is not pinned to a full commit SHA")
                ref = re.fullmatch(r"\s+# (\S+)\s*", found.group(2))
                self.assertIsNotNone(ref, f"{where}: the pin must name its ref in a trailing comment")
                pins.setdefault((pinned.group(1), ref.group(1)), set()).add(pinned.group(2))
        self.assertTrue(pins)
        for (action, ref), shas in sorted(pins.items()):
            self.assertEqual(len(shas), 1, f"{action} {ref} is pinned to {sorted(shas)}")

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
        container fixes the release binaries' glibc floor and the Bazel
        platforms pick the remote build image. Every image CI names carries
        its digest, unless UNPINNED_IMAGES lists it with its reason:
        image_problems reads job and service containers, docker://
        references in .github and the Bazel platforms, and the FROM lines of
        every tracked Dockerfile or Containerfile except the installer's launch templates,
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
                         [".github/workflows/release.yml", "tools/bazel/platforms/BUILD.bazel"], pinned)
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
                 'buildpack-deps:22.04@sha256:' + "b" * 64 + '",\n    },\n)\n'
                 'platform(\n    exec_properties = {\n        "container-image": "docker://europe-west1-docker.'
                 'pkg.dev/example-project/meerkat-ci/meerkat-ci-rust:1.94.0",\n    },\n)\n')
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
                self.assertTrue(any(problem.startswith(relative) for problem in problems), problems)

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

    def test_rust_cache_covers_the_wrapper_target(self):
        """rust-cache saves `<workspace>/target`; repo-cargo must build there.

        Without an explicit CARGO_TARGET_DIR, scripts/repo-cargo builds under
        ~/.cache, so every rust-cache step restored and saved a directory
        nothing was built in. Nothing failed - jobs were only slow - so pin the
        single workflow-wide target and refuse remaps that would split the two
        paths again.
        """
        source = CI_WORKFLOW.read_text()
        header = source.split("\njobs:\n", 1)[0]
        self.assertIn("\n  CARGO_TARGET_DIR: ${{ github.workspace }}/target\n", header)
        self.assertEqual(len(re.findall(r"(?m)^\s+CARGO_TARGET_DIR:", source)), 1)
        self.assertIsNone(re.search(r"(?m)^\s+(workspaces|cache-targets):", source))

    def test_rust_cache_saves_only_main_and_shares_identical_graphs(self):
        """Cached build output makes each rust-cache entry about 1 GB.

        Pull request entries are scoped to their own ref, so saving them only
        evicts main's entries from the 20 GB repository cache. Jobs that
        build the same dependency graph (same features, profile and
        dev-dependency use) share one entry; every other job keeps its own.
        """
        expected = {
            # The workspace lint keeps the key it had in fmt-lint, whose
            # main entry holds its build; fmt-lint itself caches nothing.
            "clippy": "fmt-lint",
            "test": "test",
            "test-integration": "test",
            "test-extension-state": None,
            "extension-state-lint": None,
            "test-voice": None,
            "voice-clippy": None,
            "test-python": "meta-dev-bins",
            "flow-editor": "meta-dev-bins",
            "console": "meta-dev-examples",
            "console-browser": "meta-dev-examples",
            "console-fixtures": "meta-dev-examples",
            "console-acceptance": "meta-dev-examples",
        }
        source = CI_WORKFLOW.read_text()
        self.assertEqual(len(re.findall(r"uses: Swatinem/rust-cache@[0-9a-f]{40} # v2\n", source)), len(expected))
        self.assertEqual(source.count("uses: Swatinem/rust-cache@"), len(expected))
        hashed_env = re.compile(r"(?m)^      ((?:CARGO|CC|CFLAGS|CXX|CMAKE|RUST)\w*): (.*)$")
        group_envs = {}
        for name, shared_key in expected.items():
            step = re.search(
                r"- uses: Swatinem/rust-cache@[0-9a-f]{40} # v2\n((?: {8}.*\n)+)", job_block(name) + "\n"
            )
            self.assertIsNotNone(step, name)
            settings = step.group(1)
            self.assertIn("save-if: ${{ github.ref == 'refs/heads/main' }}", settings, name)
            keys = re.findall(r"shared-key: (\S+)", settings)
            self.assertEqual(keys, [shared_key] if shared_key else [], name)
            if shared_key:
                # rust-cache hashes the job environment into the key, so a
                # group shares one entry only while its jobs agree on it.
                # CARGO_INCREMENTAL is 0 in every job either way: the
                # toolchain action and rust-cache both set it before the hash.
                env = [e for e in hashed_env.findall(job_block(name)) if e != ("CARGO_INCREMENTAL", '"0"')]
                group_envs.setdefault(shared_key, set()).add(tuple(env))
            if (shared_key or "").startswith("meta-dev-"):
                self.assertIn("      CARGO_PROFILE_DEV_DEBUG: line-tables-only\n", job_block(name), name)
        for shared_key, envs in group_envs.items():
            self.assertEqual(len(envs), 1, f"{shared_key}: {envs}")
        header = source.split("\njobs:\n", 1)[0]
        self.assertIn("\npermissions:\n  contents: read\n", header)
        # Jobs that run pull request code stay read-only. Only the cleanup
        # job may delete caches: it runs for pushes to main alone, after every
        # job that saves a rust cache, and drops exactly their families.
        # The Rust jobs only read actions, to fetch warmed dependency builds
        # (scripts/test_ci_warm_deps.py pins those grants).
        self.assertEqual(re.findall(r"(?m)^ +actions: write$", source), ["      actions: write"])
        drop = job_block("drop-superseded-caches")
        self.assertIn("    if: github.event_name == 'push' && github.ref == 'refs/heads/main'\n", drop)
        self.assertIn("    continue-on-error: true\n", drop)
        self.assertIn("    permissions:\n      actions: write\n      contents: read\n", drop)
        needs = re.search(r"(?m)^    needs: \[(.*)\]$", drop).group(1).split(", ")
        self.assertEqual(sorted(needs), sorted(expected))
        families = []
        for name, shared_key in expected.items():
            if (shared_key or name) not in families:
                families.append(shared_key or name)
        actions = [line.strip() for line in drop.splitlines() if re.match(r" +(- )?(uses|run): ", line)]
        self.assertEqual(len(actions), 2, actions)
        self.assertRegex(actions[0], r"^- uses: actions/checkout@[0-9a-f]{40} # v4$")
        self.assertEqual(actions[1], f"run: scripts/ci-drop-superseded-caches.sh {' '.join(families)}")
        self.assertNotIn("drop-superseded-caches", job_block("gate"))
        self.assertNotIn("steps.rust-cache", source)
        self.assertIn("- run: python3 scripts/test_ci_drop_caches.py\n", job_block("fmt-lint"))

    def test_workspace_nextest_preserves_all_failure_evidence(self):
        """The workspace tests run as three jobs that together cover cargo's
        default test selection: lib and bin unit tests in `test`, and the
        integration tests in two `test-integration` halves split by target
        name, one of which also builds the examples. All use the same filter
        and --no-fail-fast. split_coverage_problems holds the conditions under
        which that union is the default selection, on Cargo's own reading of
        every workspace member."""
        block = job_block("test-integration")
        patterns = re.findall(r'(?m)^          - tests: "([^"]+)"$', block)
        self.assertEqual(len(patterns), 2, patterns)
        members = cargo_workspace_members(ROOT)
        integration_tests = [t for package in members for t in package["targets"] if t["kind"] == ["test"]]
        self.assertGreater(len(integration_tests), 100)
        self.assertEqual(split_coverage_problems(members, patterns), [])
        self.assertIn("    strategy:\n      fail-fast: false\n", block)
        filter_ = "-E 'not test(converged_idle_gateway_consumes_near_zero_cpu)'"
        expected = {
            "test": f"scripts/repo-cargo nextest run --workspace --lib --bins {filter_} --no-fail-fast",
            "test-integration": (
                f"scripts/repo-cargo nextest run --workspace --test '${{{{ matrix.tests }}}}' {filter_} --no-fail-fast"
            ),
        }
        for name, command in expected.items():
            commands = [
                line.strip()[len("- run: "):]
                for line in job_block(name).splitlines()
                if line.strip().startswith("- run: scripts/repo-cargo nextest run")
            ]
            self.assertEqual(commands, [command], name)
            self.assertIn('CARGO_PROFILE_DEV_DEBUG: "0"', job_block(name))
            # The test fixtures give their members and child gateways their
            # own fake OpenAI key, so neither job sets one for the whole run.
            self.assertNotIn("OPENAI_API_KEY", job_block(name), name)
            # Every disk report runs last, whatever failed before it. The
            # block ends at the next job, after the comments that precede it.
            lines = job_block(name).splitlines()
            while lines and (not lines[-1].strip() or lines[-1].lstrip().startswith("#")):
                lines.pop()
            self.assertEqual(lines[-3:], ["      - name: Report runner disk", "        if: always()",
                                          "        run: df -h /"], name)
        # The unit build fits the runner's free disk and reclaims nothing;
        # the integration halves reclaim in the background, never on the
        # critical path.
        self.assertNotIn("rm -rf", job_block("test"))
        reclaim = re.search(r"(?m)^      - name: Reclaim runner disk in the background\n        run: \|\n"
                            r"((?:          .*\n)+)", block + "\n")
        self.assertIsNotNone(reclaim)
        self.assertRegex(reclaim.group(1), r"^          sudo -n nohup ionice -c2 -n7 bash -c 'rm -rf ")
        self.assertRegex(reclaim.group(1), r"docker image prune -af' >/dev/null 2>&1 &\n          df -h /\n$")
        self.assertLess(block.index("Reclaim runner disk in the background"), block.index("actions/checkout"))
        # Exactly one half builds the examples.
        self.assertEqual(block.count("            examples: true\n"), 1)
        self.assertIn("      - if: matrix.examples\n        run: scripts/repo-cargo build --workspace --examples\n", block)
        source = CI_WORKFLOW.read_text()
        self.assertEqual(source.count("scripts/repo-cargo build --workspace --examples"), 1)
        self.assertEqual(source.count("scripts/repo-cargo nextest run --workspace"), 2)

    def test_the_unit_job_proves_its_filter_leaves_out_no_lib_or_bin_test(self):
        """`test` is the only automatic run of the lib and bin tests, since
        the pre-push unit gate is manual: a lib test its filter dropped once
        ran in no CI job at all. Before the run it lists those tests with
        the run's own filter, and scripts/verify-unit-test-filter.py fails
        the job on any the filter leaves out."""
        steps = [line.strip()[len("- run: "):] for line in job_block("test").splitlines()
                 if line.strip().startswith("- run: ")]
        run = [step for step in steps if step.startswith("scripts/repo-cargo nextest run")]
        self.assertEqual(len(run), 1, steps)
        filter_ = re.search(r"-E '[^']*'", run[0]).group(0)
        listing = '"${RUNNER_TEMP}/unit-tests.json"'
        self.assertEqual(steps, [
            f"scripts/repo-cargo nextest list --workspace --lib --bins {filter_} --message-format json > {listing}",
            f"python3 scripts/verify-unit-test-filter.py {listing}",
            run[0],
        ])
        self.assertIn("- run: python3 scripts/test_verify_unit_test_filter.py\n", job_block("fmt-lint"))

    def test_the_split_coverage_guard_reads_targets_as_cargo_does(self):
        """Targets, test flags and autotests in any valid TOML spelling, and
        glob or implicit path members, all reach the guard: it reads `cargo
        metadata` and parses manifests as TOML, never the manifest text."""
        package = '[package]\nname = "{}"\nversion = "0.0.0"\nedition = "2021"\n'
        probe = package.format("guard-probe")
        cases = {
            "spaces inside the brackets": (
                {
                    "Cargo.toml": probe + '\n[[ test ]]\nname = "_extra_contract"\npath = "extra/contract.rs"\n\n[workspace]\n',
                    "extra/contract.rs": "",
                },
                ["guard-probe test _extra_contract: matches []"],
            ),
            "quoted keys": (
                {
                    "Cargo.toml": probe + '\n[["test"]]\n"name" = "_quoted_contract"\n"path" = "extra/q.rs"\n\n[workspace]\n',
                    "extra/q.rs": "",
                },
                ["guard-probe test _quoted_contract: matches []"],
            ),
            "an inline array of tables": (
                {
                    "Cargo.toml": 'test = [{ name = "_inline_contract", path = "extra/i.rs" }]\n\n' + probe + "\n[workspace]\n",
                    "extra/i.rs": "",
                },
                ["guard-probe test _inline_contract: matches []"],
            ),
            "a quoted test flag on a bin": (
                {
                    "Cargo.toml": probe + '\n[[bin]]\nname = "probe"\n"test" = false\n\n[workspace]\n',
                    "src/bin/probe.rs": "fn main() {}\n",
                },
                ["guard-probe bin probe: test flag False, default True"],
            ),
            "an inline test flag on an example": (
                {
                    "Cargo.toml": 'example = [{ name = "demo", test = true }]\n\n' + probe + "\n[workspace]\n",
                    "examples/demo.rs": "fn main() {}\n",
                },
                ["guard-probe example demo: test flag True, default False"],
            ),
            "autotests off through dotted keys": (
                {
                    "Cargo.toml": (
                        'package.name = "guard-probe"\npackage.version = "0.0.0"\npackage.edition = "2021"\n'
                        "package.autotests = false\n\n[workspace]\n"
                    ),
                    "tests/alpha.rs": "",
                },
                ["guard-probe: autotests = false"],
            ),
            "a glob member": (
                {
                    "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n',
                    "crates/c/Cargo.toml": package.format("c"),
                    "crates/c/src/lib.rs": "",
                    "crates/c/tests/Upper.rs": "",
                },
                ["c test Upper: matches []"],
            ),
            "an implicit path member": (
                {
                    "Cargo.toml": '[workspace]\nmembers = ["a"]\nresolver = "2"\n',
                    "a/Cargo.toml": package.format("a") + '\n[dependencies]\nb = { path = "../b" }\n',
                    "a/src/lib.rs": "",
                    "b/Cargo.toml": package.format("b"),
                    "b/src/lib.rs": "",
                    "b/tests/_hidden.rs": "",
                },
                ["b test _hidden: matches []"],
            ),
            "required features": (
                {
                    "Cargo.toml": probe + (
                        '\n[features]\nslow = []\n\n[[test]]\nname = "gated"\npath = "extra/gated.rs"\n'
                        'required-features = ["slow"]\n\n[workspace]\n'
                    ),
                    "extra/gated.rs": "",
                },
                ["guard-probe test gated: required-features ['slow']"],
            ),
            # Control: explicit and discovered targets the patterns cover pass.
            "covered targets": (
                {
                    "Cargo.toml": probe + '\n[[test]]\nname = "middle"\npath = "extra/middle.rs"\n\n[workspace]\n',
                    "extra/middle.rs": "",
                    "tests/zeta.rs": "",
                },
                [],
            ),
        }
        for case, (files, expected) in cases.items():
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                for relative, content in {"src/lib.rs": "", **files}.items():
                    path = Path(tmp) / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                members = cargo_workspace_members(Path(tmp))
                self.assertEqual(split_coverage_problems(members, ["[a-i]*", "[j-z]*"]), expected)

    def test_decorator_authority_gate_actually_runs_in_ci(self):
        """A structural gate nobody invokes is a control that cannot fail.

        `verify-decorator-authority.py` exists because a decorator omitting
        `request_attempt_authority` compiles clean, warns nothing, and stranded
        72 identities downstream. If it is ever dropped from `fmt-lint` the
        defect class goes unwatched again with no other signal, so pin the
        invocation here rather than trusting that the file's presence means it
        runs.
        """
        block = job_block("fmt-lint")
        self.assertIn(
            "- run: python3 scripts/verify-decorator-authority.py",
            block,
        )

    def test_the_continuity_export_scripts_run_their_tests_in_ci(self):
        """The standalone continuity export scripts ship to hosts that run
        them once, before they upgrade, with no other check behind them;
        so does the blob restore beside them, which a restore from a
        continuity mirror runs once.
        fmt-lint runs on every pull request and push; ci-changes.py lets
        their changes reach no lane on that promise. Each suite runs twice:
        on the runner's own Python and Node.js, and again, last in the job,
        on the oldest runtimes a 0.9.3 host may run them on, the 0.9.3 SDKs'
        floors (Python 3.10 and Node.js 18)."""
        python_suite = "- run: python3 tools/continuity-export/test_continuity_export.py\n"
        restore_suite = "- run: python3 tools/continuity-export/test_restore_blobs.py\n"
        node_suite = "- run: node --test tools/continuity-export/continuity-export.test.mjs\n"
        lines = job_block("fmt-lint").splitlines()
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
            "      - uses: actions/setup-node@[0-9a-f]{40} # v4\n"
            "        with:\n"
            '          node-version: "18"\n'
            f"      {re.escape(node_suite)}"
            r"\Z", steps)
        self.assertIsNotNone(floor_rows, "fmt-lint must end with the suites on the floor runtimes")
        before = steps[:floor_rows.start()]
        for suite in (python_suite, restore_suite, node_suite):
            self.assertEqual(before.count(suite), 1, suite)
        # The first runs use the runner's own runtimes.
        self.assertNotIn("actions/setup-python@", before)
        self.assertNotIn("actions/setup-node@", before)

    def test_the_workspace_lint_runs_in_its_own_rust_lane_job(self):
        """fmt-lint runs on every pull request and compiles nothing; the
        workspace clippy runs once, in the clippy job, when a Rust change
        reaches it, beside fmt-lint instead of after its contract tests."""
        source = CI_WORKFLOW.read_text()
        command = "- run: scripts/repo-cargo clippy --workspace --all-targets -- -D warnings\n"
        self.assertEqual(source.count(command), 1)
        lint = job_block("clippy")
        self.assertIn(command, lint)
        self.assertIn("    needs: changes\n    if: needs.changes.outputs.rust == 'true'\n", lint)
        self.assertIn("          components: clippy\n", lint)
        fmt_lint = "\n".join(line for line in job_block("fmt-lint").splitlines()
                             if not line.strip().startswith("#"))
        self.assertNotIn("needs:", fmt_lint)
        self.assertNotIn(" if:", fmt_lint)
        self.assertNotIn("repo-cargo clippy", fmt_lint)
        self.assertIn("          components: rustfmt\n", fmt_lint)
        self.assertNotIn("rust-cache", fmt_lint)
        for kept in ("- run: scripts/repo-cargo fmt --all -- --check\n",
                     "- run: node scripts/generate-bazel-rust-builds.mjs --check\n",
                     "- run: scripts/repo-cargo machete --with-metadata\n"):
            self.assertIn(kept, fmt_lint)

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
        block = job_block("test-typescript")
        self.assertIn(
            "- run: npm --prefix sdk/typescript run validate --silent",
            block,
        )

    def test_extension_state_lint_and_tests_run_as_parallel_jobs(self):
        """check and clippy only emit metadata; the tests need full builds.

        Every command of the former serial job still runs exactly once,
        split between the two jobs.
        """
        def runs(name):
            return [
                line.strip()[len("- run: "):]
                for line in job_block(name).splitlines()
                if line.strip().startswith("- run: ")
            ]

        self.assertEqual(
            runs("extension-state-lint"),
            [
                "scripts/repo-cargo check -p meerkat-meta --locked --no-default-features",
                "scripts/repo-cargo clippy -p meerkat-meta --locked --features "
                "extension-state,announcement-test-faults,console-test-faults --lib "
                "--test extension_state --test mob_announcements_delivery_process "
                "--test mob_announcements_remote_process --test console_send_recovery_process "
                "-- -D warnings",
            ],
        )
        self.assertEqual(
            runs("test-extension-state"),
            [
                "scripts/repo-cargo test -p meta-extension-state --locked",
                "scripts/repo-cargo test -p meerkat-meta --locked --features "
                "extension-state,announcement-test-faults,console-test-faults --lib "
                "--test extension_state --test mob_announcements_delivery_process "
                "--test mob_announcements_remote_process --test console_send_recovery_process "
                "--no-run",
                "scripts/repo-cargo test -p meerkat-meta --locked --features "
                "extension-state,announcement-test-faults,console-test-faults "
                "--test extension_state",
                "scripts/repo-cargo test -p meerkat-meta --locked --features "
                "extension-state,announcement-test-faults,console-test-faults "
                "--test mob_announcements_delivery_process --test mob_announcements_remote_process "
                "--test console_send_recovery_process",
                "scripts/repo-cargo test -p meerkat-meta --locked --features "
                "extension-state,announcement-test-faults,console-test-faults "
                "--lib -- extension_ document_mcp_child_supply",
            ],
        )
        for name in ("extension-state-lint", "test-extension-state"):
            self.assertIn('CARGO_INCREMENTAL: "0"', job_block(name))
            self.assertIn("CARGO_PROFILE_DEV_DEBUG: line-tables-only", job_block(name))

    def test_extension_state_test_targets_build_together_before_they_run(self):
        """The integration test and the --lib harness are separate compilations
        of meerkat-meta; one `--no-run` invocation builds both in parallel, so
        it must name every target the later meerkat-meta steps run, with the
        same package, features and flags, and come before them."""
        steps = [
            line.strip()[len("- run: "):]
            for line in job_block("test-extension-state").splitlines()
            if line.strip().startswith("- run: scripts/repo-cargo test -p meerkat-meta ")
        ]
        build, *runs = steps
        self.assertTrue(build.endswith(" --no-run"), build)
        prefix = (
            "scripts/repo-cargo test -p meerkat-meta --locked --features "
            "extension-state,announcement-test-faults,console-test-faults "
        )
        self.assertTrue(all(step.startswith(prefix) for step in steps), steps)
        built = set(re.findall(r"--(lib|test \S+)", build.split(" -- ")[0]))
        ran = {target for step in runs for target in re.findall(r"--(lib|test \S+)", step.split(" -- ")[0])}
        self.assertEqual(built, ran)

    def test_no_job_caps_build_parallelism(self):
        """A CARGO_BUILD_JOBS cap throttled the dependency graph of the four
        jobs that carried it. The dev-profile jobs run uncapped, like the
        rest of the workflow."""
        code = [line for line in CI_WORKFLOW.read_text().splitlines() if not line.strip().startswith("#")]
        self.assertEqual([line for line in code if "CARGO_BUILD_JOBS" in line], [])
        self.assertEqual([line for line in code if re.search(r"repo-cargo .*(\s-j\s*\d|--jobs)", line)], [])

    def test_live_lanes_print_cargo_output_without_color_codes(self):
        """A run that holds a model provider's key is read by grep: its pass
        counts and its summary lines. dtolnay/rust-toolchain sets
        CARGO_TERM_COLOR=always when nothing set it, so Cargo and nextest put
        color codes inside those lines, and a check for a passing summary
        failed a run whose rows had all passed. Each such workflow sets the
        variable to never for all its jobs, and nothing sets color back."""
        live = []
        for path in sorted((ROOT / ".github" / "workflows").glob("*.y*ml")):
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
        self.assertIn("e2e-live.yml", live)

    def test_voice_feature_runs_real_host_tests_and_strict_lint(self):
        block = job_block("test-voice")
        self.assertIn("--locked --features openai-live", block)
        self.assertIn("--test console_route_auth --test console_experience --test never_executed_persistence --test swarm_integration console_voice", block)
        self.assertIn("gateway_openai_live_registration", block)
        self.assertIn('CARGO_INCREMENTAL: "0"', block)
        # The strict lint runs in parallel, in its own job, on the same features.
        runs = [line for line in block.splitlines() if line.strip().startswith("- run:")]
        self.assertFalse([line for line in runs if " clippy " in line], runs)
        lint = job_block("voice-clippy")
        self.assertEqual(
            lint.count(
                "- run: scripts/repo-cargo clippy -p meerkat-meta --locked --features openai-live "
                "--lib --bin rpc_gateway --bin meta_gateway -- -D warnings\n"
            ),
            1,
        )
        self.assertIn('CARGO_INCREMENTAL: "0"', lint)

    def test_voice_lane_runs_the_live_registration_test_from_the_library(self):
        # src/bin/rpc_gateway.rs is a one-line wrapper around
        # meerkat_meta::rpc_gateway::main(), and the registration test lives in
        # the library module. A `--bin rpc_gateway` selector compiles the
        # wrapper and runs zero tests, so the step passed without testing.
        command = (
            "scripts/repo-cargo test -p meerkat-meta --locked "
            "--features openai-live --lib gateway_openai_live_registration"
        )
        block = job_block("test-voice")
        self.assertEqual(block.count(f"- run: {command}\n"), 1)
        self.assertNotIn("--bin rpc_gateway gateway_openai_live_registration", block)
        wrapper = (ROOT / "crates/meerkat-meta/src/bin/rpc_gateway.rs").read_text()
        self.assertNotIn("gateway_openai_live_registration", wrapper)
        library = (ROOT / "crates/meerkat-meta/src/rpc_gateway.rs").read_text()
        self.assertIn("fn gateway_openai_live_registration_is_explicit_and_strict()", library)

    def test_voice_lane_selects_every_persistent_http_restart_case(self):
        # Default workspace tests cannot see the feature-gated voice cases.
        # Select their integration binary and shared filter in the existing
        # voice lane, which already compiles the gateway with openai-live.
        command = (
            "scripts/repo-cargo test -p meerkat-meta --locked "
            "--features openai-live --test meta_gateway_bootstrap "
            "persistent_http_restarts"
        )
        block = job_block("test-voice")
        self.assertEqual(block.count(f"- run: {command}\n"), 1)
        source = (ROOT / "crates/meerkat-meta/tests/meta_gateway_bootstrap.rs").read_text()
        self.assertEqual(
            set(re.findall(r"fn (persistent_http_restarts_\w+)\(", source)),
            {
                "persistent_http_restarts_classic_without_voice",
                "persistent_http_restarts_identity_without_voice",
                "persistent_http_restarts_classic_with_voice",
                "persistent_http_restarts_identity_with_voice",
            },
        )

    def test_console_checks_live_voice_evidence_without_paid_calls(self):
        block = job_block("console")
        self.assertIn("node console/voice-e2e-live.cjs --self-test\n", block)
        self.assertIn("node console/voice-e2e-live.cjs --self-test-audio\n", block)
        self.assertNotIn("run e2e:voice:live", block)

    def test_console_fixture_unit_regressions_run_with_the_shared_backend_build(self):
        block = job_block("console")
        command = "scripts/repo-cargo test -p meerkat-meta --example console_acceptance_fixture"
        self.assertEqual(block.count(command), 1)
        self.assertLess(block.index("Build console acceptance backends"), block.index(command))
        self.assertLess(block.index(command), block.index("node console/api-e2e.cjs"))

    def test_console_browser_and_perf_run_beside_console(self):
        """The two longest console steps run in their own jobs, each exactly once.

        perf:typing:browser bundles its own host and needs no Rust. The
        browser scenarios need the acceptance backends and freshly built
        console bundles, in that order.
        """
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
            "- run: npm --prefix console run build --silent\n",
            "- run: npm --prefix console run build:acceptance --silent\n",
            "playwright install --with-deps chromium",
            "- name: Build console acceptance backends\n",
            "- name: Native MCP Apps rendering and reload\n",
            "- name: Native MCP Apps polling during an active model turn\n",
            "- name: Native MCP Apps on a builder host with identity-first members\n",
            "- run: node console/browser-e2e.cjs --shard=${{ matrix.shard }}/3\n",
            "name: console-browser-${{ github.run_id }}-${{ matrix.shard }}\n",
        ]
        positions = [browser.index(marker) for marker in order]
        self.assertEqual(positions, sorted(positions))
        # Three legs, each dealt its own scenarios by the registry's
        # --shard=i/3; the matrix and the shard count must agree. MCP Apps
        # run once.
        legs = re.search(r"(?m)^        shard: \[([0-9, ]+)\]$", browser).group(1).split(", ")
        self.assertEqual(legs, [str(index) for index in range(1, 4)])
        self.assertIn(f"--shard=${{{{ matrix.shard }}}}/{len(legs)}\n", browser)
        self.assertIn("      fail-fast: false\n", browser)
        self.assertEqual(browser.count("        if: matrix.shard == 1\n"), 3)
        # The gateway lanes take the default backend; only the builder lane
        # selects console_acceptance_fixture.
        self.assertEqual(browser.count("          META_MCP_APPS_BACKEND: builder\n"), 1)
        self.assertIn("META_EXAMPLE_BIN_DIR=${target_dir}/debug/examples", browser)

        console = job_block("console")
        for kept in (
            "- run: node console/api-e2e.cjs\n",
            "- run: node console/voice-e2e.cjs\n",
            "- run: node console/memory-e2e.cjs\n",
            "- run: node console/workgraph-e2e.cjs\n",
            "- run: npm --prefix console run embedded:freshness --silent\n",
            "name: console-${{ github.run_id }}\n",
        ):
            self.assertIn(kept, console)

    def test_gate_requires_every_job_it_lists(self):
        """A job absent from `gate` can fail without failing the check suite.

        `needs` alone does not gate: with `if: always()` the gate runs
        regardless, so a job missing from the result checks is advisory only.

        Correspondence ALONE is not enough, and this test asserted only that
        until review caught it: deleting a job from `needs` and from the
        checks together keeps the two lists in agreement, so the job goes on
        running and gates nothing while this test stays green. The roster
        below is therefore a floor - every gating job must be present - and the
        correspondence check runs on top of it.

        A job in a lane may be skipped only when `changes` set that lane to
        exactly "false"; every such job is one the same lane output gates,
        the lanes and their jobs are the ones scripts/ci-changes.py
        classifies, and every other job must succeed.
        """
        block = job_block("gate")
        needs = block.split("needs: [", 1)[1].split("]", 1)[0]
        listed = {name.strip() for name in needs.split(",")}

        must_gate = {
            "changes",
            "fmt-lint",
            "clippy",
            "test",
            "test-integration",
            "test-extension-state",
            "extension-state-lint",
            "test-voice",
            "voice-clippy",
            "test-python",
            "test-typescript",
            "console",
            "console-browser",
            "console-perf",
            "console-fixtures",
            "console-acceptance",
            "flow-editor",
            "audit",
        }
        missing = must_gate - listed
        self.assertFalse(
            missing,
            f"these jobs must gate the suite and are absent from `needs`: "
            f"{sorted(missing)}. Removing a job from both lists keeps them "
            f"consistent while silently making it advisory.",
        )

        checks = dict(re.findall(r'(?m)^\s+check ([a-z-]+) "\$\{\{ needs\.\1\.result \}\}" (required|[a-z_]+)$', block))
        self.assertEqual(set(checks), listed, "every job in `needs` is checked exactly by name")
        self.assertIn('"$3" != "required" && "$2" == "skipped" && "${lane_value}" == "false"', block)
        self.assertIn('local variable="LANE_${3^^}"', block)
        for lane in ci_changes.LANES:
            self.assertIn(f"LANE_{lane.upper()}: ${{{{ needs.changes.outputs.{lane} }}}}\n", block)
        lanes = {}
        for name, mode in checks.items():
            gate = re.findall(r"(?m)^    if: needs\.changes\.outputs\.([a-z_]+) == 'true'$", job_block(name))
            if mode == "required":
                self.assertEqual(gate, [], name)
                continue
            self.assertEqual(gate, [mode], name)
            self.assertIn("    needs: changes\n", job_block(name))
            lanes.setdefault(mode, set()).add(name)
        # The workflow's lanes are exactly the classifier's, job for job.
        self.assertEqual(lanes, {lane: set(jobs) for lane, jobs in ci_changes.LANE_JOBS.items()})
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
        every = {lane: "true" for lane in ci_changes.LANES}
        console_only = {lane: ("true" if lane in ("console", "console_perf", "flow_editor") else "false")
                        for lane in ci_changes.LANES}
        rust_jobs = {job: "skipped" for job in ci_changes.LANE_JOBS["rust"]}
        cases = (
            ("all green", {}, every, 0),
            ("rust lane skipped when classified false", rust_jobs | {"test-integration": "skipped",
                                                                      "test-python": "skipped"}, console_only, 0),
            ("a skipped job whose lane is true", {"test-voice": "skipped"}, every, 1),
            ("a skipped job of a running lane", {"console-perf": "skipped"}, console_only, 1),
            ("a failed job in a skipped lane", {"test": "failure"}, console_only, 1),
            ("lanes missing: classification failed", rust_jobs, {}, 1),
            ("lanes empty", rust_jobs, {lane: "" for lane in ci_changes.LANES}, 1),
            ("a required job skipped", {"fmt-lint": "skipped"}, console_only, 1),
            ("changes failed", {"changes": "failure"}, every, 1),
            ("feature lanes skipped when classified false",
             {job: "skipped" for lane in ("voice", "extension_state") for job in ci_changes.LANE_JOBS[lane]},
             {**every, "voice": "false", "extension_state": "false"}, 0),
            ("a voice job skipped while its lane runs", {"voice-clippy": "skipped"},
             {**every, "extension_state": "false"}, 1),
            ("the workspace lint skipped while rust runs", {"clippy": "skipped"}, every, 1),
        )
        for name, results, lanes, status in cases:
            with self.subTest(case=name):
                result = self.run_gate(results, lanes)
                self.assertEqual(result.returncode, status, result.stdout + result.stderr)

    def test_every_job_builds_the_commit_its_event_names(self):
        """Main's caches are restored by later main runs, whose result is the
        evidence release.yml accepts, so only main's own code may write them:
        no dispatch input and no checkout ref can point a job at another
        commit."""
        source = CI_WORKFLOW.read_text()
        triggers = source.split("\npermissions:", 1)[0]
        self.assertIn("  workflow_dispatch:\n", triggers)
        self.assertNotIn("inputs:", triggers)
        # Branches get CI through their pull request (or a dispatch); only
        # main's pushes run on push, so no branch runs every lane beside its
        # own pull request run.
        self.assertIn("  push:\n    branches: [main]\n  pull_request:\n", triggers)
        # Any version or pinned SHA of the checkout action.
        uses = re.findall(r"(?m)^\s*-?\s*uses:\s*actions/checkout@\S+", source)
        checkouts = re.findall(r"(?m)^ *- uses: actions/checkout@\S+(?: #[^\n]*)?\n((?: {8,}.*\n)*)", source)
        self.assertTrue(uses)
        self.assertEqual(len(checkouts), len(uses))
        for settings in checkouts:
            self.assertNotIn("ref:", settings)
            self.assertNotIn("repository:", settings)
        # Nor may a run step fetch or move to another commit; reading the
        # base commit's classifier (git show) is the only git use.
        code = "\n".join(line for line in source.splitlines() if not line.strip().startswith("#"))
        self.assertEqual(re.findall(r"\bgit\s+(fetch|checkout|switch|reset|pull|clone|worktree|restore)\b", code), [])

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

    def test_changes_job_classifies_the_pull_request_and_fails_closed(self):
        block = job_block("changes")
        self.assertIn("fetch-depth: 2", block)
        # Every lane is an output.
        outputs = re.findall(r"(?m)^      ([a-z_]+): \$\{\{ steps\.classify\.outputs\.\1 \}\}$", block)
        self.assertEqual(outputs, list(ci_changes.LANES))
        self.assertIn(f'          lanes="{" ".join(ci_changes.LANES)}"\n', block)
        self.assertIn("git show HEAD^1:scripts/ci-changes.py", block)
        self.assertNotIn("python3 scripts/ci-changes.py", block)
        self.assertNotIn("if:", block)
        self.assertIn("- run: python3 scripts/test_ci_changes.py\n", job_block("fmt-lint"))

    def run_classify_step(self, base_classifier: str | None, *, head_classifier: str | None = None,
                          event: str = "pull_request") -> dict[str, str]:
        """Run the classify step in a repository whose base commit holds
        `base_classifier` (None: no classifier at all) and whose head may
        change it; return the step's GITHUB_OUTPUT as a mapping."""
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
            repo.mkdir()
            temp.mkdir()
            output = Path(tmp) / "output"

            def git(*args):
                subprocess.run(["git", *args], cwd=repo, env=env, check=True, capture_output=True)

            git("init", "-q")
            (repo / "scripts").mkdir()
            for index, classifier in enumerate(
                    (base_classifier, head_classifier if head_classifier is not None else base_classifier)):
                if classifier is not None:
                    (repo / "scripts/ci-changes.py").write_text(classifier)
                (repo / "touched.txt").write_text(f"commit {index}\n")
                git("add", "-A")
                git("commit", "-q", "-m", "tree")
            env.update(EVENT=event, RUNNER_TEMP=str(temp), GITHUB_OUTPUT=str(output))
            subprocess.run(["bash", "-e", "-c", "\n".join(body)], cwd=repo, env=env, check=True,
                           capture_output=True, text=True)
            return dict(line.split("=", 1) for line in output.read_text().splitlines())

    def test_the_base_commits_classifier_decides_and_fails_closed(self):
        every = {lane: "true" for lane in ci_changes.LANES}
        nothing_runs = ("import sys\nargs = sys.argv\nopen(args[args.index('--github-output') + 1], 'a')"
                        ".write(''.join(f'{lane}=false\\n' for lane in "
                        f"{list(ci_changes.LANES)!r}))\n")
        # Pushes and dispatches run every lane even if a classifier says none.
        for event in ("push", "workflow_dispatch", "schedule"):
            with self.subTest(event=event):
                self.assertEqual(self.run_classify_step(nothing_runs, event=event), every)

        def printing(values: dict[str, str]) -> str:
            text = "".join(f"{key}={value}\\n" for key, value in values.items())
            return ("import sys\nargs = sys.argv\nopen(args[args.index('--github-output') + 1], 'a')"
                    f".write('{text}')\n")

        console_only = {lane: ("true" if lane == "console" else "false") for lane in ci_changes.LANES}
        self.assertEqual(self.run_classify_step(printing(console_only)), console_only)
        # The pull request's own classifier never decides its lanes.
        nothing = {lane: "false" for lane in ci_changes.LANES}
        self.assertEqual(self.run_classify_step(printing(console_only), head_classifier=printing(nothing)),
                         console_only)
        # The classifier before the voice and extension-state lanes printed
        # the other six; a pull request on such a base runs every lane.
        six = {lane: "false" for lane in ("rust", "integration", "python", "console", "console_perf",
                                          "flow_editor")}
        self.assertEqual(self.run_classify_step(printing(six)), every)
        for case, base in (("predates the lanes", printing({"full": "false"})),
                           ("misses a lane", printing({k: v for k, v in console_only.items() if k != "python"})),
                           ("fails", "raise SystemExit(3)\n"),
                           ("is absent", None)):
            with self.subTest(base=case):
                self.assertEqual(self.run_classify_step(base), every)

if __name__ == "__main__":
    unittest.main()
