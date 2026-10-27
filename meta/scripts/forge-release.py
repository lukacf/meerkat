#!/usr/bin/env python3
"""Meta Forge release protocol: attested packages in their own release.

Forge ships beside Meta, not inside Meta's release: a forge-v<version> tag
(the version in installer/pyproject.toml), its own attested candidate
manifest, and its own GitHub release, published as a prerelease that is never
the repository's latest. Meta's release.yml and release-candidate.py are not
involved. This script loads release-candidate.py read-only for its strict
JSON, GitHub API, artifact, attestation and release helpers, and rebinds the
copy it loaded to this protocol's workflow, .github/workflows/forge-release.yml.

Modes (workflow_dispatch only):
  candidate: build each target's package from the pinned inputs, verify it,
    attest it, and bind the set in an attested forge-candidate-manifest.json.
    Any branch can build one as a dry run. Only a main candidate whose exact
    main CI run is green can be published: publication verifies every run and
    attestation against main.
  publish: release an accepted candidate under its operator-pushed
    forge-v<version> tag. It adds the exact attested bytes to a draft, reads
    them back, then publishes the draft as a prerelease that is not latest.

A candidate does not rerun Forge's own suite. It binds a green dispatched
forge.yml run (backend and frontend) that tested exactly the Forge inputs at
its commit: the
trees of installer, console, packages/console-core, packages/console-components
and sdk/python, forge.yml itself, the lockfiles, installer requirements and
vendored wheels, the gateway source whose shutdown horizon a Forge test pins,
and a toolchain that fits forge.yml's declared versions. A
pull request run never counts: it tests GitHub's merge of its head into the
base (refs/pull/N/merge), not its head commit. If no dispatched run did, it
refuses until forge.yml is dispatched for that commit.

Inputs (installer/release/forge-release-inputs.json, schema 1): per target,
every file the packager reads, each pinned by size and SHA-256 and taken
from this checkout (installer/release/inputs/) or from an allowlisted public
release or PyPI URL, plus the owner's product selection and the guide's
pinned Meta and Core versions and sources. A target's optional
ordinary_gateway names the pinned Meta rpc_gateway release asset of the
selection's Meta version; the package then also builds applications without a
connection setup helper.

Selections are strict JSON objects:
  forge-accepted-candidate: schema_version=1, kind, repository, run_id,
    run_attempt, manifest_artifact={id, digest, manifest_sha256}.
"""

import argparse
import base64
import importlib
import importlib.util
import itertools
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import sys
import tarfile
import time
import tomllib
import urllib.error
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "release_candidate", Path(__file__).with_name("release-candidate.py")
)
assert SPEC is not None and SPEC.loader is not None
rc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rc)

WORKFLOW = ".github/workflows/forge-release.yml"
# release-candidate.py checks runs and attestations against its WORKFLOW.
# This process's copy of it checks Forge's workflow instead; Meta's release
# runs never load this script, so their copy keeps release.yml.
rc.WORKFLOW = WORKFLOW

require, ProtocolError = rc.require, rc.ProtocolError
MAIN = rc.MAIN
INPUTS = "installer/release/forge-release-inputs.json"
INPUTS_DIR = "installer/release/inputs/"
PYPROJECT = "installer/pyproject.toml"
TARGETS = ("linux-x86_64", "macos-arm64")
# Each Forge target's Meta native release asset triple.
GATEWAY_TRIPLES = {"linux-x86_64": "x86_64-unknown-linux-gnu", "macos-arm64": "aarch64-apple-darwin"}
TAG_PREFIX = "forge-v"
MANIFEST = "forge-candidate-manifest.json"
MANIFEST_PROVENANCE = "forge-manifest-provenance.jsonl"
SELECTION = "forge-candidate-selection.json"
PRODUCT_SELECTION_FORMAT = "forge-meta-toolkit-runtime-selection-v1"
SELECTED_ARTIFACTS = ("assembly", "meta_wheel", "toolkit_wheel", "dependency_requirements", "model_catalog")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")
# GitHub refuses a release asset of 2 GiB or more.
MAX_ASSET = 2 * 1024**3 - 1
MAX_INPUT = 2 * 1024**3 - 1
MAX_INPUT_FILES = 512
# The packager's own bounds (launchkit/product_distribution.py), plus its manifest.
MAX_PACKAGE_FILES = 8192 + 1
MAX_PACKAGE_TOTAL = 4 * 1024**3 + 64 * 1024**2
ALLOWED_URLS = (
    "https://github.com/lukacf/meerkat-meta/releases/download/",
    "https://github.com/lukacf/meerkat-connect/releases/download/",
    "https://files.pythonhosted.org/packages/",
)
DOWNLOAD_ATTEMPTS = 3
# A release reuses the green Forge run (forge.yml, backend and frontend) that
# tested exactly the Forge inputs at the release commit: these trees, files,
# lockfiles and vendored wheels, and the toolchain forge.yml declares.
FORGE_CI = ".github/workflows/forge.yml"
EVIDENCE_TREES = ("console", "installer", "packages/console-components", "packages/console-core", "sdk/python")
# installer/tests/test_gateway_shutdown_horizon.py reads the shutdown horizon
# the gateway source advertises, so the bound run must have tested it too.
EVIDENCE_FILES = (FORGE_CI, "console/package-lock.json", "installer/web/package-lock.json",
                  "crates/meerkat-meta/src/rpc_gateway.rs")
EVIDENCE_JOBS = ("backend", "frontend")
TOOLCHAIN_FILE = "forge-toolchain.json"
MAX_EVIDENCE_RUNS = 30
# GitHub's workflow run and artifact listings are eventually consistent: for
# minutes at a time a listing can omit a run or an artifact that exists, and a
# run can report a stale status. A candidate read that misses is read again,
# REREAD_DELAY seconds apart, for up to REREAD_ATTEMPTS reads (10 minutes)
# before it refuses. A refusal that is not a lagging read, such as a red
# exact-main CI run or a mismatched run identity, stays immediate.
REREAD_ATTEMPTS = 40
REREAD_DELAY = 15
# rc.api runs `gh api` once and raises on any failure: inside a re-read, a 5xx
# response or a broken connection is a miss as well.
TRANSIENT_API = re.compile(r"gh api failed: .*(?:\bHTTP 5[0-9][0-9]\b|error connecting to|connection reset|"
                           r"connection refused|timeout|timed out|deadline exceeded|\bEOF\b|GOAWAY|broken pipe)",
                           re.I | re.S)


def package_name(version, target):
    return f"meta-forge-{version}-{target}.zip"


def provenance_name(version, target):
    return f"meta-forge-{version}-{target}-provenance.jsonl"


def package_members(version, target):
    name = package_name(version, target)
    return [name, name + ".sha256", provenance_name(version, target)]


def package_artifact_name(target, attempt):
    return f"forge-package-{target}-{attempt}"


def manifest_artifact_name(attempt):
    return f"forge-candidate-manifest-{attempt}"


def toolchain_artifact_name(job, attempt):
    return f"forge-toolchain-{job}-{attempt}"


def write_outputs(values):
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as handle:
            for key, value in values.items():
                handle.write(f"{key}={value}\n")
    print(json.dumps(values, sort_keys=True))


def safe_name(value, label):
    require(isinstance(value, str) and 0 < len(value) <= 512, f"invalid {label}")
    path = PurePosixPath(value)
    require(not path.is_absolute() and path.as_posix() == value and "\\" not in value
            and all(part not in ("", ".", "..") and not part.startswith(".") for part in path.parts),
            f"unsafe {label}: {value!r}")
    return value


def safe_url(value):
    require(isinstance(value, str) and value.isascii() and value.startswith(ALLOWED_URLS)
            and not any(character in value for character in "?#\\ ")
            and "/../" not in value and not value.endswith("/.."),
            f"input URL is not an allowlisted public release or PyPI file: {value!r}")
    return value


def forge_version(revision="HEAD"):
    value = tomllib.loads(rc.git("show", f"{revision}:{PYPROJECT}"))["project"]["version"]
    require(isinstance(value, str) and VERSION.fullmatch(value), "invalid Forge version in installer/pyproject.toml")
    return value


def validate_pin(pin, name):
    require(isinstance(pin, dict), f"input {name} must be an object")
    if "path" in pin:
        rc.exact_keys(pin, "path size sha256", f"input {name}")
        safe_name(pin["path"], f"input {name} path")
        require(pin["path"].startswith(INPUTS_DIR), f"input {name} must live under {INPUTS_DIR}")
    elif "member" in pin:
        rc.exact_keys(pin, "url archive_size archive_sha256 member size sha256", f"input {name}")
        safe_url(pin["url"])
        require(pin["url"].endswith(".tar.gz"), f"input {name} extracts only from a .tar.gz archive")
        rc.integer(pin["archive_size"], f"input {name} archive size")
        require(pin["archive_size"] <= MAX_INPUT, f"input {name} archive exceeds the input bound")
        rc.digest(pin["archive_sha256"], f"input {name} archive SHA-256")
        safe_name(pin["member"], f"input {name} archive member")
    else:
        rc.exact_keys(pin, "url size sha256", f"input {name}")
        safe_url(pin["url"])
    rc.integer(pin["size"], f"input {name} size")
    require(pin["size"] <= MAX_INPUT, f"input {name} exceeds the input bound")
    rc.digest(pin["sha256"], f"input {name} SHA-256")


def validate_target(target, entry):
    keys = "files gateway selection paths"
    rc.exact_keys(entry, keys + (" ordinary_gateway" if isinstance(entry, dict) and "ordinary_gateway" in entry else ""),
        f"{target} inputs")
    files = entry["files"]
    require(isinstance(files, dict) and 0 < len(files) <= MAX_INPUT_FILES, f"{target} needs pinned input files")
    for name, pin in files.items():
        safe_name(name, f"{target} input name")
        validate_pin(pin, name)
    require(entry["gateway"] in files, f"{target} guide gateway must be a pinned input")
    selection = entry["selection"]
    require(isinstance(selection, dict) and selection.get("format") == PRODUCT_SELECTION_FORMAT
            and selection.get("target") == target, f"{target} needs the owner's product selection for {target}")
    names = SELECTED_ARTIFACTS + (("source_manifest",) if "source_manifest" in selection else ())
    paths = entry["paths"]
    rc.exact_keys(paths, " ".join(("artifact_root", "dependency_wheels") + names), f"{target} paths")
    root = safe_name(paths["artifact_root"], f"{target} artifact root")

    def bound(name, artifact, label):
        require(isinstance(name, str) and name in files and name.startswith(root + "/"),
                f"{target} {label} is not a pinned input under its artifact root")
        require(isinstance(artifact, dict) and artifact.get("size") == files[name]["size"]
                and artifact.get("sha256") == files[name]["sha256"],
                f"{target} {label} differs between the selection and its pinned input")

    for key in names:
        bound(paths[key], selection.get(key), key)
    wheels, selected = paths["dependency_wheels"], selection.get("dependency_wheels")
    require(isinstance(wheels, list) and isinstance(selected, list) and wheels and len(wheels) == len(selected),
            f"{target} dependency wheels differ between the selection and its paths")
    for name, artifact in zip(wheels, selected):
        bound(name, artifact, "dependency wheel")
    require(not entry["gateway"].startswith(root + "/"), f"{target} guide gateway must stay outside the product root")
    if "ordinary_gateway" in entry:
        # The application's ordinary Meta RPC gateway: the release asset of the
        # selected Meta SDK's own version, never the guide's or a product file.
        ordinary = entry["ordinary_gateway"]
        require(isinstance(ordinary, str) and ordinary in files and ordinary != entry["gateway"]
                and not ordinary.startswith(root + "/"),
                f"{target} ordinary gateway must be a separate pinned input outside the product root")
        pin, meta = files[ordinary], selection.get("meta")
        version = meta.get("version") if isinstance(meta, dict) else None
        require(isinstance(version, str) and VERSION.fullmatch(version) and "member" in pin
                and pin["url"] == ("https://github.com/lukacf/meerkat-meta/releases/download/"
                                   f"v{version}/meta-rpc-gateway-{version}-{GATEWAY_TRIPLES[target]}.tar.gz")
                and pin["member"] == "rpc_gateway",
                f"{target} ordinary gateway must be the Meta {version} rpc_gateway release asset for {target}")


def validate_inputs(value):
    rc.exact_keys(value, "schema_version kind guide targets", "Forge inputs")
    require(value["schema_version"] == 1 and value["kind"] == "forge-release-inputs", "unsupported Forge inputs")
    guide = value["guide"]
    rc.exact_keys(guide, "version core_version meta_source core_source", "guide inputs")
    for key in ("version", "core_version"):
        require(isinstance(guide[key], str) and VERSION.fullmatch(guide[key]), f"invalid guide {key}")
    for key in ("meta_source", "core_source"):
        rc.digest(guide[key], f"guide {key}", 40)
    targets = value["targets"]
    require(isinstance(targets, dict) and set(targets) <= set(TARGETS), "unknown Forge target")
    for target, entry in targets.items():
        validate_target(target, entry)
    return value


def load_inputs(revision="HEAD"):
    return validate_inputs(rc.parse_json(rc.git("show", f"{revision}:{INPUTS}")))


def target_inputs(target):
    targets = load_inputs()["targets"]
    require(target in targets, f"no pinned inputs for {target}")
    return targets[target]


def validate_selection(value):
    rc.exact_keys(value, "schema_version kind repository run_id run_attempt manifest_artifact", "selection")
    require(value["schema_version"] == 1 and value["kind"] == "forge-accepted-candidate",
            "forge-accepted-candidate selection required")
    require(value["repository"] == rc.repository(), "selection repository mismatch")
    rc.integer(value["run_id"], "run ID")
    rc.integer(value["run_attempt"], "run attempt")
    rc.exact_keys(value["manifest_artifact"], "id digest manifest_sha256", "manifest artifact")
    rc.integer(value["manifest_artifact"]["id"], "manifest artifact ID")
    rc.digest(value["manifest_artifact"]["digest"], "manifest artifact digest")
    rc.digest(value["manifest_artifact"]["manifest_sha256"], "manifest SHA-256")
    return value


def validate_forge_ci(value):
    rc.exact_keys(value, "run_id run_attempt head_sha event tuple toolchain", "manifest Forge run")
    rc.integer(value["run_id"], "Forge run ID")
    rc.integer(value["run_attempt"], "Forge run attempt")
    rc.digest(value["head_sha"], "Forge run SHA", 40)
    require(value["event"] == "workflow_dispatch", "Forge evidence must be a dispatched forge.yml run")
    rc.exact_keys(value["tuple"], "trees files", "Forge inputs tuple")
    trees, files = value["tuple"]["trees"], value["tuple"]["files"]
    require(isinstance(trees, dict) and set(trees) == set(EVIDENCE_TREES)
            and isinstance(files, dict) and set(EVIDENCE_FILES) <= set(files), "incomplete Forge inputs tuple")
    for path, sha in [*trees.items(), *files.items()]:
        rc.digest(sha, f"{path} object ID", 40)
    toolchain = value["toolchain"]
    rc.exact_keys(toolchain, "declared " + " ".join(EVIDENCE_JOBS), "Forge toolchain")
    for name, versions in toolchain.items():
        rc.exact_keys(versions, "python node", f"{name} toolchain")
    require(toolchain_fits(toolchain["declared"], toolchain), "the Forge run's toolchain differs from forge.yml")
    return value


def validate_manifest(value):
    rc.exact_keys(value, "schema_version kind repository source_sha version tag run_id run_attempt "
                  "workflow ci forge_ci inputs_sha256 packages", "Forge candidate manifest")
    require(value["schema_version"] == 1 and value["kind"] == "forge-candidate-manifest",
            "unsupported Forge candidate manifest")
    require(value["repository"] == rc.repository(), "manifest repository mismatch")
    rc.digest(value["source_sha"], "manifest source", 40)
    require(isinstance(value["version"], str) and VERSION.fullmatch(value["version"])
            and value["tag"] == TAG_PREFIX + value["version"], "manifest version/tag mismatch")
    rc.integer(value["run_id"], "run ID")
    attempt = rc.integer(value["run_attempt"], "run attempt")
    rc.exact_keys(value["workflow"], "path sha ref", "manifest workflow")
    require(value["workflow"]["path"] == WORKFLOW and value["workflow"]["sha"] == value["source_sha"]
            and isinstance(value["workflow"]["ref"], str) and value["workflow"]["ref"].startswith("refs/heads/"),
            "manifest workflow identity mismatch")
    if value["ci"] is not None:
        rc.exact_keys(value["ci"], "run_id run_attempt", "manifest CI run")
        rc.integer(value["ci"]["run_id"], "CI run ID")
        rc.integer(value["ci"]["run_attempt"], "CI run attempt")
    require((value["ci"] is None) == (value["workflow"]["ref"] != MAIN), "a main candidate binds its green CI run")
    validate_forge_ci(value["forge_ci"])
    rc.digest(value["inputs_sha256"], "inputs SHA-256")
    packages = value["packages"]
    require(isinstance(packages, list) and packages, "manifest names no packages")
    targets = []
    for package in packages:
        rc.exact_keys(package, "target name size sha256 artifact", "manifest package")
        target = package["target"]
        require(target in TARGETS and package["name"] == package_name(value["version"], target),
                "manifest package name/target mismatch")
        size = rc.integer(package["size"], "package size")
        require(size <= MAX_ASSET, "package exceeds GitHub's release asset limit")
        rc.digest(package["sha256"], "package SHA-256")
        rc.exact_keys(package["artifact"], "id name digest", "package artifact")
        rc.integer(package["artifact"]["id"], "package artifact ID")
        require(package["artifact"]["name"] == package_artifact_name(target, attempt), "package artifact name mismatch")
        rc.digest(package["artifact"]["digest"], "package artifact digest")
        targets.append(target)
    require(targets == sorted(set(targets)), "manifest packages must be unique and sorted by target")
    return value


def resolve():
    rc.repository()
    event, ref = os.environ.get("GITHUB_EVENT_NAME"), os.environ.get("GITHUB_REF", "")
    require(event == "workflow_dispatch", "Forge releases are dispatched, never pushed")
    mode = os.environ.get("RELEASE_MODE", "")
    require(mode in ("candidate", "publish"), "unknown Forge release mode")
    source, selected = os.environ.get("SOURCE_SHA", ""), os.environ.get("ARTIFACT_SELECTION", "")
    head = rc.digest(rc.git("rev-parse", "HEAD"), "checkout SHA", 40)
    require(head == os.environ.get("GITHUB_SHA") == os.environ.get("GITHUB_WORKFLOW_SHA"),
            "checkout/event/workflow SHA mismatch")
    if mode == "candidate":
        require(ref.startswith("refs/heads/"), "a candidate is dispatched from a branch")
        require(not selected, "a selection is only valid for publish")
        require(rc.digest(source, "SOURCE_SHA", 40) == head, "SOURCE_SHA must be the dispatched commit")
        targets = sorted(load_inputs()["targets"])
        require(targets, f"no Forge target has pinned inputs in {INPUTS}")
        version = forge_version()
        tag = TAG_PREFIX + version
    else:
        # Publication reads only the accepted candidate, never main's current inputs.
        require(ref == MAIN, "publication is dispatched from main")
        require(not source, "SOURCE_SHA is only valid for candidate")
        validate_selection(rc.parse_json(selected))
        targets, version, tag = [], "", ""
    # build_package skips outside candidate mode before reading its matrix;
    # it still receives a valid one.
    matrix = {"include": [{"target": target} for target in targets] or [{"target": "none"}]}
    write_outputs({
        "mode": mode, "sha": head, "version": version, "tag": tag,
        "publishable": str(mode == "candidate" and ref == MAIN).lower(),
        "matrix": json.dumps(matrix, separators=(",", ":"), sort_keys=True),
    })


class Miss(Exception):
    """A read GitHub may not have settled yet. reread() reads again, and after
    its last read refuses with `refusal`, the text a single read refused with."""

    def __init__(self, reason, refusal):
        super().__init__(reason)
        self.reason, self.refusal = reason, refusal


def reread(label, endpoint, read):
    """The first read() that does not miss: a miss, or a transient gh api
    failure, is read again REREAD_DELAY seconds later, for up to
    REREAD_ATTEMPTS reads. Each miss logs one line to stderr. Any other
    refusal propagates at once."""
    started = time.monotonic()
    for attempt in range(1, REREAD_ATTEMPTS + 1):
        try:
            return read()
        except Miss as miss:
            reason, refusal = miss.reason, miss.refusal
        except ProtocolError as error:
            if not TRANSIENT_API.match(str(error)):
                raise
            reason, refusal = f"transient API failure: {error}", str(error)
        elapsed = f"{time.monotonic() - started:.0f}"
        print(f"{label}: {endpoint}: read {attempt}/{REREAD_ATTEMPTS} missed at {elapsed} s: "
              + " ".join(reason.split()), file=sys.stderr)
        if attempt < REREAD_ATTEMPTS:
            time.sleep(REREAD_DELAY)
    raise ProtocolError(f"{refusal} (after {REREAD_ATTEMPTS} reads over {elapsed} s)")


def ci_green():
    sha = rc.digest(os.environ.get("RELEASE_SHA"), "RELEASE_SHA", 40)
    endpoint = rc.repo_endpoint(f"actions/workflows/ci.yml/runs?branch=main&event=push&head_sha={sha}&per_page=100")

    def read():
        runs = [run for run in rc.api(endpoint)["workflow_runs"] if run.get("head_sha") == sha
                and run.get("head_branch") == "main" and run.get("event") == "push"]
        if not runs:
            raise Miss("no exact-main ci.yml run listed",
                       f"no exact-main CI run exists for {sha}; push it to main and let ci.yml finish")
        green = [run for run in runs if run.get("status") == "completed" and run.get("conclusion") == "success"]
        if green:
            return green
        states = ", ".join(f"{run.get('id')}/{run.get('status')}/{run.get('conclusion')}" for run in runs)
        refusal = f"exact-main CI for {sha} is not green: {states}"
        # Every listed run finished and none succeeded: a red CI, not a
        # lagging listing, so it refuses at once.
        require(any(run.get("status") != "completed" for run in runs), refusal)
        raise Miss(f"no listed exact-main run is green yet: {states}", refusal)

    green = reread("exact-main CI", endpoint, read)
    chosen = max(green, key=lambda run: (run["id"], run["run_attempt"]))
    rc.verify_run(chosen["id"], chosen["run_attempt"], sha, ci=True)
    write_outputs({"run_id": chosen["id"], "run_attempt": chosen["run_attempt"]})


class GitObjects:
    """Read-only tree walks through the git data API, for any commit in the
    repository (pull request heads included), cached by object SHA."""

    def __init__(self):
        self.commits, self.trees = {}, {}

    def entries(self, tree):
        if tree not in self.trees:
            value = rc.api(rc.repo_endpoint(f"git/trees/{rc.digest(tree, 'tree SHA', 40)}"))
            require(value.get("truncated") is False, f"git tree {tree} listing truncated")
            self.trees[tree] = {item["path"]: (item["type"], item["sha"]) for item in value["tree"]}
        return self.trees[tree]

    def lookup(self, commit, path):
        if commit not in self.commits:
            value = rc.api(rc.repo_endpoint(f"git/commits/{rc.digest(commit, 'commit SHA', 40)}"))
            self.commits[commit] = rc.digest(value["tree"]["sha"], "commit tree", 40)
        kind, sha = "tree", self.commits[commit]
        for part in PurePosixPath(path).parts:
            if kind != "tree" or part not in self.entries(sha):
                return None
            kind, sha = self.entries(sha)[part]
        return kind, sha

    def blob_text(self, sha):
        value = rc.api(rc.repo_endpoint(f"git/blobs/{rc.digest(sha, 'blob SHA', 40)}"))
        require(value.get("encoding") == "base64", "unexpected git blob encoding")
        return base64.b64decode(value["content"]).decode()


def evidence_tuple(objects, commit):
    """The Forge inputs at one commit, as git object IDs. The trees already
    contain the lockfiles, requirements and wheels; they are listed as well
    so a mismatch names the file that changed."""
    trees, files = {}, {}
    for path in EVIDENCE_TREES:
        found = objects.lookup(commit, path)
        require(found is not None and found[0] == "tree", f"{path} is not a directory at {commit}")
        trees[path] = found[1]
    for path in EVIDENCE_FILES:
        found = objects.lookup(commit, path)
        require(found is not None and found[0] == "blob", f"{path} is not a file at {commit}")
        files[path] = found[1]
    for name, (kind, sha) in objects.entries(trees["installer"]).items():
        if kind == "blob" and re.fullmatch(r"requirements[A-Za-z0-9_.-]*\.txt", name):
            files[f"installer/{name}"] = sha
    pending = [("installer/vendor", objects.lookup(commit, "installer/vendor"))]
    while pending:
        path, found = pending.pop()
        if found is None or found[0] != "tree":
            continue
        for name, (kind, sha) in objects.entries(found[1]).items():
            if kind == "tree":
                pending.append((f"{path}/{name}", (kind, sha)))
            elif kind == "blob" and name.endswith(".whl"):
                files[f"{path}/{name}"] = sha
    return {"trees": trees, "files": dict(sorted(files.items()))}


def tuple_differences(wanted, observed):
    return sorted(f"{group}:{path}" for group in ("trees", "files")
                  for path in set(wanted[group]) | set(observed[group])
                  if wanted[group].get(path) != observed[group].get(path))


def declared_toolchain(objects, files):
    text = objects.blob_text(files[FORGE_CI])
    pythons = set(re.findall(r"python-version:\s*['\"]?([0-9][0-9.]*)['\"]?", text))
    nodes = set(re.findall(r"node-version:\s*['\"]?([0-9][0-9.]*)['\"]?", text))
    require(len(pythons) == 1 and len(nodes) == 1, "forge.yml must declare exactly one Python and one Node version")
    return {"python": pythons.pop(), "node": nodes.pop()}


def version_fits(observed, pattern, declared):
    match = re.fullmatch(pattern, observed) if isinstance(observed, str) else None
    return match is not None and (match.group(1) == declared or match.group(1).startswith(declared + "."))


def run_toolchain(run_info, work):
    """The versions each job of a Forge run recorded, from its toolchain artifacts."""
    run_id, attempt = run_info["id"], run_info["run_attempt"]
    artifacts = run_artifacts(run_id)
    recorded = {}
    for job in EVIDENCE_JOBS:
        name = toolchain_artifact_name(job, attempt)
        item = artifacts.get(name)
        if item is None or not str(item.get("digest", "")).startswith("sha256:"):
            return None
        identity = {"id": item["id"], "name": name, "digest": rc.digest(item["digest"].removeprefix("sha256:"))}
        packed = rc.download_artifact(identity, run_info, work)
        folder = rc.unpack_artifact(packed, work / f"toolchain-{run_id}-{attempt}-{job}", [TOOLCHAIN_FILE])
        value = rc.read_json(folder / TOOLCHAIN_FILE)
        rc.exact_keys(value, "python node", f"{job} toolchain")
        recorded[job] = value
    return recorded


def toolchain_fits(declared, recorded):
    return (version_fits(recorded["backend"]["python"], r"Python ([0-9]+\.[0-9]+\.[0-9]+)", declared["python"])
            and all(version_fits(recorded[job]["node"], r"v([0-9]+\.[0-9]+\.[0-9]+)", declared["node"])
                    for job in EVIDENCE_JOBS))


def run_jobs_green(run_id, attempt):
    jobs = rc.api(rc.repo_endpoint(f"actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100"))["jobs"]
    conclusions = {}
    for job in jobs:
        conclusions.setdefault(job["name"], []).append(job["conclusion"])
    return all(conclusions.get(name) == ["success"] for name in EVIDENCE_JOBS)


def forge_ci_evidence(sha, work, objects=None):
    """The newest green dispatched Forge run that tested exactly this
    commit's Forge inputs, preferring a run of this commit itself. A pull
    request run tested a merge commit, not its head_sha, so it never counts.
    Refuses when no dispatched run did, after re-listing the runs: the
    listing can omit a run for minutes."""
    objects = objects or GitObjects()
    wanted = evidence_tuple(objects, sha)
    declared = declared_toolchain(objects, wanted["files"])
    endpoint = rc.repo_endpoint(
        f"actions/workflows/forge.yml/runs?event=workflow_dispatch&status=success&per_page={MAX_EVIDENCE_RUNS}")
    listing = itertools.count(1)

    def read():
        # Every listing reads the same object cache, and downloads toolchain
        # records into a folder of its own.
        folder = work / f"forge-evidence-{next(listing)}"
        folder.mkdir()
        runs = [run for run in rc.api(endpoint)["workflow_runs"] if run.get("event") == "workflow_dispatch"
                and run.get("path") == FORGE_CI and run.get("status") == "completed"
                and run.get("conclusion") == "success" and run["repository"]["full_name"] == rc.repository()]
        nearest = None
        for run in sorted(runs, key=lambda run: (run["head_sha"] != sha, -run["id"])):
            observed = evidence_tuple(objects, rc.digest(run["head_sha"], "Forge run SHA", 40))
            differences = tuple_differences(wanted, observed)
            if differences:
                nearest = nearest or (run["id"], differences)
                continue
            if not run_jobs_green(run["id"], run["run_attempt"]):
                continue
            toolchain = run_toolchain(run, folder)
            if toolchain is None or not toolchain_fits(declared, toolchain):
                continue
            return {"run_id": run["id"], "run_attempt": run["run_attempt"], "head_sha": run["head_sha"],
                    "event": run["event"], "tuple": wanted, "toolchain": {"declared": declared, **toolchain}}
        detail = (f"; the newest green run {nearest[0]} differs in {', '.join(nearest[1])}" if nearest else "")
        raise Miss(f"none of {len(runs)} listed green dispatched run(s) tested these Forge inputs",
                   f"no green dispatched Forge run (backend and frontend) tested the Forge inputs at "
                   f"{sha}{detail}. "
                   "Dispatch forge.yml for this commit, then rerun the candidate")

    return reread("Forge evidence", endpoint, read)


def recheck_forge_ci(manifest):
    """Publication rechecks the reused run against the candidate's commit."""
    record, objects = manifest["forge_ci"], GitObjects()
    wanted = evidence_tuple(objects, manifest["source_sha"])
    require(record["tuple"] == wanted and evidence_tuple(objects, record["head_sha"]) == wanted,
            "the bound Forge run no longer tested this candidate's Forge inputs")
    run = rc.api(rc.repo_endpoint(f"actions/runs/{record['run_id']}"))
    require(run["id"] == record["run_id"] and run["run_attempt"] == record["run_attempt"]
            and run["head_sha"] == record["head_sha"] and run["path"] == FORGE_CI
            and run["event"] == "workflow_dispatch"
            and run["status"] == "completed" and run["conclusion"] == "success"
            and run_jobs_green(record["run_id"], record["run_attempt"]), "the bound Forge run is no longer green")


def check_bytes(path, size, sha256, label):
    require(path.is_file() and not path.is_symlink(), f"{label} is not a regular file")
    require(path.stat().st_size == size and rc.assets.sha256(path) == sha256,
            f"{label} differs from its pinned size/SHA-256")


def download(url, destination, limit):
    """Fetch one pinned public file; the caller checks its bytes."""
    for attempt in range(1, DOWNLOAD_ATTEMPTS + 1):
        try:
            request = urllib.request.Request(safe_url(url), headers={"User-Agent": "meta-forge-release"})
            with urllib.request.urlopen(request, timeout=120) as response, destination.open("xb") as handle:
                received = 0
                while block := response.read(1024 * 1024):
                    received += len(block)
                    require(received <= limit, f"{url} exceeds its pinned size")
                    handle.write(block)
            return
        except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
            destination.unlink(missing_ok=True)
            require(attempt < DOWNLOAD_ATTEMPTS, f"download failed after {attempt} attempts: {url}: {error}")
            time.sleep(5 * attempt)


def extract_member(archive, member, destination, limit):
    with tarfile.open(archive, "r:gz") as source:
        found = [info for info in source.getmembers() if info.name == member]
        require(len(found) == 1 and found[0].isfile() and 0 < found[0].size <= limit,
                f"archive member {member} is missing, duplicated or not a bounded regular file")
        with source.extractfile(found[0]) as stream, destination.open("xb") as handle:
            shutil.copyfileobj(stream, handle, 1024 * 1024)


def copy_checkout_file(name, destination, limit):
    source = ROOT / name
    require(source.is_file() and not source.is_symlink() and source.resolve().is_relative_to(ROOT)
            and source.stat().st_size <= limit, f"checkout input {name} is not a bounded regular file")
    with source.open("rb") as stream, destination.open("xb") as handle:
        shutil.copyfileobj(stream, handle, 1024 * 1024)


def fetch_inputs(args):
    entry = target_inputs(args.target)
    output = args.output_dir
    require(not output.exists() and not output.is_symlink(), "the inputs directory must be new")
    output.mkdir(mode=0o700)
    downloads = output.with_name(output.name + "-downloads")
    downloads.mkdir(mode=0o700)
    try:
        for name, pin in sorted(entry["files"].items()):
            destination = output.joinpath(*PurePosixPath(name).parts)
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            if "path" in pin:
                copy_checkout_file(pin["path"], destination, pin["size"])
            elif "member" in pin:
                archive = downloads / pin["archive_sha256"]
                if not archive.exists():
                    download(pin["url"], archive, pin["archive_size"])
                    check_bytes(archive, pin["archive_size"], pin["archive_sha256"], pin["url"])
                extract_member(archive, pin["member"], destination, pin["size"])
            else:
                download(pin["url"], destination, pin["size"])
            check_bytes(destination, pin["size"], pin["sha256"], name)
            destination.chmod(0o700 if name in {entry["gateway"], entry.get("ordinary_gateway")} else 0o600)
    finally:
        shutil.rmtree(downloads)
    print(f"fetched {len(entry['files'])} pinned inputs for {args.target} into {output}")


def launchkit_module(name):
    installer = str(ROOT / "installer")
    if installer not in sys.path:
        sys.path.insert(0, installer)
    return importlib.import_module(f"launchkit.{name}")


def package(args):
    """Compose the package through the Forge owner's own selection and packager."""
    inputs = load_inputs()
    require(args.target in inputs["targets"], f"no pinned inputs for {args.target}")
    entry, guide = inputs["targets"][args.target], inputs["guide"]
    root = args.inputs_dir.resolve(strict=True)
    target = launchkit_module("packaged_runtime").relocate_selection(entry["selection"], entry["paths"], root)
    gateway = launchkit_module("connect_installer_caller").ExactFile(
        root.joinpath(*PurePosixPath(entry["gateway"]).parts), entry["files"][entry["gateway"]]["sha256"])
    selected_guide = launchkit_module("bootstrap").SelectedGuideRuntime(
        gateway=gateway, version=guide["version"], core_version=guide["core_version"],
        meta_source=guide["meta_source"], core_source=guide["core_source"])
    ordinary = None
    if "ordinary_gateway" in entry:
        ordinary = launchkit_module("connect_installer_caller").ExactFile(
            root.joinpath(*PurePosixPath(entry["ordinary_gateway"]).parts),
            entry["files"][entry["ordinary_gateway"]]["sha256"])
    require(args.output.name == package_name(forge_version(), args.target), "unexpected package name")
    result = launchkit_module("product_distribution").package(ROOT / "installer", target, selected_guide, args.output,
        **({"ordinary_gateway": ordinary} if ordinary is not None else {}))
    require(result["native_target"] == args.target, "the package was built for another target")
    print(json.dumps(result, sort_keys=True))


def safe_member(value):
    """A package member path. Checker dependencies may hold dotfiles, so only
    absolute, empty, dot and parent parts are refused."""
    path = PurePosixPath(value)
    require(isinstance(value, str) and 0 < len(value) <= 4096 and not path.is_absolute()
            and path.as_posix() == value and "\\" not in value
            and all(part not in ("", ".", "..") for part in path.parts), f"unsafe package member: {value!r}")
    return value


def unpack_package(archive, destination):
    """Extract a package without trusting its member names or modes."""
    destination.mkdir(mode=0o700)
    with zipfile.ZipFile(archive) as source:
        infos = source.infolist()
        names = [info.filename for info in infos]
        require(0 < len(infos) <= MAX_PACKAGE_FILES and len(names) == len(set(names)), "invalid package inventory")
        require(sum(info.file_size for info in infos) <= MAX_PACKAGE_TOTAL, "package exceeds its expanded bound")
        for info in infos:
            safe_member(info.filename)
            mode = info.external_attr >> 16
            require(not info.is_dir() and stat.S_IFMT(mode) in (0, stat.S_IFREG) and not info.flag_bits & 1,
                    f"package member {info.filename} is not a plain file")
            target = destination.joinpath(*PurePosixPath(info.filename).parts)
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with source.open(info) as stream, target.open("xb") as handle:
                shutil.copyfileobj(stream, handle, 1024 * 1024)


def check_package(args):
    archive = args.package
    require(archive.name == package_name(args.version, args.target), "unexpected package name")
    require(archive.is_file() and not archive.is_symlink() and 0 < archive.stat().st_size <= MAX_ASSET,
            "the package must be a regular file under GitHub's 2 GiB release asset limit")
    extracted = archive.with_name(archive.name + ".verify")
    try:
        unpack_package(archive, extracted)
        data = launchkit_module("product_distribution").verify(extracted)
        require(data["support"]["native_target"] == args.target, "the package was built for another target")
    finally:
        shutil.rmtree(extracted, ignore_errors=True)
    checksum = archive.with_name(archive.name + ".sha256")
    rc.write_new(checksum, f"{rc.assets.sha256(archive)}  {archive.name}\n".encode())
    print(f"verified {archive.name} ({archive.stat().st_size} bytes) for {args.target}")


def verify_package_files(folder, version, target, sha, run_id, attempt, ref):
    archive = folder / package_name(version, target)
    require(archive.is_file() and not archive.is_symlink(), "missing package file")
    size, hashed = archive.stat().st_size, rc.assets.sha256(archive)
    require(0 < size <= MAX_ASSET, "the package exceeds GitHub's release asset limit")
    checksum = folder / (archive.name + ".sha256")
    require(checksum.read_bytes() == f"{hashed}  {archive.name}\n".encode(), "package checksum file mismatch")
    rc.verify_provenance(archive, folder / provenance_name(version, target), sha, run_id, attempt, ref=ref)
    return {"target": target, "name": archive.name, "size": size, "sha256": hashed}


def candidate_context():
    rc.repository()
    sha = rc.digest(os.environ.get("RELEASE_SHA"), "RELEASE_SHA", 40)
    version, tag = os.environ.get("RELEASE_VERSION", ""), os.environ.get("RELEASE_TAG", "")
    require(rc.git("rev-parse", "HEAD") == sha and forge_version() == version and tag == TAG_PREFIX + version,
            "resolved source/version/tag differs from the checkout")
    ref = os.environ.get("GITHUB_REF", "")
    require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch" and ref.startswith("refs/heads/")
            and os.environ.get("GITHUB_SHA") == sha == os.environ.get("GITHUB_WORKFLOW_SHA"),
            "collection runs in its own dispatched candidate run")
    return sha, version, tag, ref


def current_run(run_id, attempt, sha, ref):
    endpoint = rc.repo_endpoint(f"actions/runs/{run_id}")

    def read():
        value = rc.api(endpoint)
        require(value["id"] == run_id and value["run_attempt"] == attempt, "run identity/attempt changed")
        require(value["repository"]["full_name"] == rc.repository()
                and value["head_repository"]["full_name"] == rc.repository(), "run repository mismatch")
        require(value["head_sha"] == sha and value["head_branch"] == ref.removeprefix("refs/heads/")
                and value["event"] == "workflow_dispatch" and value["path"] == WORKFLOW,
                "run source/workflow/event/ref mismatch")
        # The reported status can lag behind the running job.
        if value["status"] != "in_progress":
            raise Miss(f"status={value['status']}", "collection requires its in-progress candidate run")
        return value

    return reread("candidate run", endpoint, read)


def artifacts_endpoint(run_id):
    return rc.repo_endpoint(f"actions/runs/{run_id}/artifacts?per_page=100")


def run_artifacts(run_id):
    pages = rc.api(artifacts_endpoint(run_id), pages=True)
    items = [item for page in pages for item in page["artifacts"]]
    names = [item["name"] for item in items]
    require(len(names) == len(set(names)), "duplicate artifact names in the candidate run")
    return {item["name"]: item for item in items}


def candidate_artifacts(run_id, names):
    """The candidate run's artifacts, once the listing names every package.
    Every build_package leg has finished before collect starts, so a name
    still missing is a listing that has not caught up."""
    def read():
        artifacts = run_artifacts(run_id)
        missing = [name for name in names if name not in artifacts]
        if missing:
            raise Miss(f"{', '.join(missing)} not listed", f"missing package artifact {missing[0]}")
        return artifacts

    return reread("candidate artifacts", artifacts_endpoint(run_id), read)


def ci_run_from_env():
    run_id, attempt = os.environ.get("CI_RUN_ID", ""), os.environ.get("CI_RUN_ATTEMPT", "")
    if not run_id and not attempt:
        return None
    return {"run_id": rc.env_int("CI_RUN_ID"), "run_attempt": rc.env_int("CI_RUN_ATTEMPT")}


def collect(args):
    sha, version, tag, ref = candidate_context()
    run_id, attempt = rc.env_int("GITHUB_RUN_ID"), rc.env_int("GITHUB_RUN_ATTEMPT")
    run_info = current_run(run_id, attempt, sha, ref)
    ci = ci_run_from_env()
    if ref == MAIN:
        require(ci is not None, "a main candidate binds its green exact-main CI run")
        rc.verify_run(ci["run_id"], ci["run_attempt"], sha, ci=True)
    else:
        require(ci is None, "a branch dry run binds no CI run")
    targets = sorted(load_inputs()["targets"])
    require(targets, "no Forge target has pinned inputs")
    artifacts = candidate_artifacts(run_id, [package_artifact_name(target, attempt) for target in targets])
    packages = []
    with rc.workspace() as work:
        forge_ci = forge_ci_evidence(sha, work)
        for target in targets:
            name = package_artifact_name(target, attempt)
            item = artifacts[name]
            require(isinstance(item.get("digest"), str) and item["digest"].startswith("sha256:"),
                    f"package artifact {name} has no digest")
            identity = {"id": item["id"], "name": name, "digest": rc.digest(item["digest"].removeprefix("sha256:"))}
            packed = rc.download_artifact(identity, run_info, work)
            folder = rc.unpack_artifact(packed, work / target, package_members(version, target))
            record = verify_package_files(folder, version, target, sha, run_id, attempt, ref)
            packages.append({**record, "artifact": identity})
    manifest = validate_manifest({
        "schema_version": 1, "kind": "forge-candidate-manifest", "repository": rc.repository(),
        "source_sha": sha, "version": version, "tag": tag, "run_id": run_id, "run_attempt": attempt,
        "workflow": {"path": WORKFLOW, "sha": sha, "ref": ref}, "ci": ci, "forge_ci": forge_ci,
        "inputs_sha256": rc.hash_bytes(rc.run(["git", "show", f"{sha}:{INPUTS}"])),
        "packages": packages,
    })
    args.output_dir.mkdir()
    rc.write_new(args.output_dir / MANIFEST, rc.json_bytes(manifest))
    state = "publishable main candidate" if ref == MAIN else f"dry run from {ref} (never publishable)"
    print(f"collected {len(packages)} attested Forge package(s) for {tag}: {state}; no release or tag writes")


def selection(args):
    require(args.manifest_file.name == MANIFEST, "unexpected manifest file name")
    manifest = validate_manifest(rc.read_json(args.manifest_file))
    value = validate_selection({
        "schema_version": 1, "kind": "forge-accepted-candidate", "repository": manifest["repository"],
        "run_id": manifest["run_id"], "run_attempt": manifest["run_attempt"],
        "manifest_artifact": {"id": args.artifact_id, "digest": rc.digest(args.artifact_digest),
                              "manifest_sha256": rc.assets.sha256(args.manifest_file)},
    })
    require(args.artifact_id not in {package["artifact"]["id"] for package in manifest["packages"]},
            "manifest artifact ID duplicates a package artifact")
    encoded = rc.json_bytes(value).decode()
    print(encoded, end="")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as handle:
            if manifest["workflow"]["ref"] == MAIN:
                handle.write("## Forge candidate selection\n\n")
            else:
                handle.write(f"## Forge dry run from {manifest['workflow']['ref']}: not publishable\n\n")
            handle.write(f"```json\n{encoded}```\n\n")
            handle.write("Publication authorizes these exact attested bytes, not a rebuild.\n")


def release_notes(manifest):
    targets = ", ".join(package["target"] for package in manifest["packages"])
    return (
        f"Meta Forge {manifest['version']} developer alpha ({targets}), built from {manifest['source_sha']}. "
        "Unpack a package into a new folder and run `bash start.sh` there. "
        "Each package has a .sha256 file and a GitHub build attestation: "
        f"`gh attestation verify <package> --repo {rc.repository()} "
        f"--signer-workflow {rc.repository()}/{WORKFLOW} --source-ref {MAIN} "
        f"--source-digest {manifest['source_sha']}`. "
        f"{MANIFEST} binds the packages; {SELECTION} records the accepted candidate. "
        "This prerelease is separate from Meta's own releases."
    )


def meta_latest():
    """The repository's latest release, which must stay one of Meta's."""
    tag = rc.api(rc.repo_endpoint("releases/latest")).get("tag_name")
    require(isinstance(tag, str) and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", tag),
            f"the repository's latest release is not a Meta release: {tag!r}")
    return tag


def publish(_args):
    require(os.environ.get("RELEASE_MODE") == "publish" and os.environ.get("GITHUB_REF") == MAIN
            and os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch",
            "publication runs from main in publish mode")
    selected = validate_selection(rc.parse_json(os.environ.get("ARTIFACT_SELECTION", "")))
    run_id, attempt = selected["run_id"], selected["run_attempt"]
    sha = rc.digest(rc.api(rc.repo_endpoint(f"actions/runs/{run_id}")).get("head_sha"), "candidate SHA", 40)
    run_info = rc.verify_run(run_id, attempt, sha)
    with rc.workspace() as work:
        identity = {"id": selected["manifest_artifact"]["id"], "name": manifest_artifact_name(attempt),
                    "digest": selected["manifest_artifact"]["digest"]}
        packed = rc.download_artifact(identity, run_info, work)
        folder = rc.unpack_artifact(packed, work / "manifest", [MANIFEST, MANIFEST_PROVENANCE])
        manifest_path = folder / MANIFEST
        require(rc.assets.sha256(manifest_path) == selected["manifest_artifact"]["manifest_sha256"],
                "manifest bytes differ from the accepted selection")
        manifest = validate_manifest(rc.read_json(manifest_path))
        require(manifest["source_sha"] == sha and manifest["run_id"] == run_id and manifest["run_attempt"] == attempt
                and manifest["workflow"]["ref"] == MAIN, "only a main candidate can be published")
        rc.verify_provenance(manifest_path, folder / MANIFEST_PROVENANCE, sha, run_id, attempt)
        rc.verify_run(manifest["ci"]["run_id"], manifest["ci"]["run_attempt"], sha, ci=True)
        recheck_forge_ci(manifest)
        version, tag = manifest["version"], manifest["tag"]
        receipt = work / SELECTION
        rc.write_new(receipt, rc.json_bytes(selected))
        expected = {SELECTION: receipt, MANIFEST: manifest_path, MANIFEST_PROVENANCE: folder / MANIFEST_PROVENANCE}
        for item in manifest["packages"]:
            packed = rc.download_artifact(item["artifact"], run_info, work)
            members = package_members(version, item["target"])
            unpacked = rc.unpack_artifact(packed, work / item["target"], members)
            record = verify_package_files(unpacked, version, item["target"], sha, run_id, attempt, MAIN)
            require(record == {key: item[key] for key in ("target", "name", "size", "sha256")},
                    f"package {item['name']} differs from its manifest record")
            expected.update({name: unpacked / name for name in members})
        rc.verify_remote_tag(tag, sha)
        latest = meta_latest()
        release = rc.release_for_tag(tag)
        inventory, original = {}, {}
        if release is not None:
            require(release["draft"] is True, f"{tag} is already published; nothing is replaced")
            inventory = rc.release_inventory(release)
            unexpected = sorted(set(inventory) - set(expected))
            require(not unexpected, f"unrecognized assets on the {tag} draft: {', '.join(unexpected)}; "
                    "operator cleanup required")
            existing = work / "existing"
            rc.download_release_inventory(inventory, existing)
            for name in inventory:
                require(rc.assets.sha256(existing / name) == rc.assets.sha256(expected[name]),
                        f"existing draft asset conflict: {name}; operator cleanup required")
            original = rc.snapshot_hashes(inventory, existing)
            require(rc.release_inventory(release) == inventory, "release changed before write")
        else:
            rc.run(["gh", "release", "create", tag, "--repo", rc.repository(), "--verify-tag", "--draft",
                    "--prerelease", "--latest=false", "--title", f"Meta Forge {version}",
                    "--notes", release_notes(manifest)])
            release = rc.newly_created_release_for_tag(tag)
            require(release is not None and release["draft"] is True, "new draft release missing")
            require(not rc.release_inventory(release), "new draft unexpectedly contains assets")
        # Bind first: a later attempt refuses a draft that carries another receipt.
        for name in [SELECTION] + sorted(set(expected) - {SELECTION}):
            if name not in inventory:
                rc.verify_remote_tag(tag, sha)
                rc.upload(tag, expected[name])
        rc.readback(release, expected, original, work, sha, tag)
        rc.run(["gh", "api", "--method", "PATCH", rc.repo_endpoint(f"releases/{rc.integer(release['id'], 'release ID')}"),
                "-F", "draft=false", "-F", "prerelease=true", "-f", "make_latest=false"])
        final = rc.release_for_tag(tag)
        require(final is not None and final["id"] == release["id"] and final["draft"] is False
                and final["prerelease"] is True, "release was not published as a prerelease")
        rc.readback(final, expected, original, work, sha, tag)
        require(meta_latest() == latest, f"the repository's latest release changed from {latest} while publishing")
    print(f"published {tag} from {sha} as a prerelease; {latest} stays the latest release")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("resolve")
    commands.add_parser("ci-green")
    fetch_parser = commands.add_parser("fetch-inputs")
    fetch_parser.add_argument("--target", choices=TARGETS, required=True)
    fetch_parser.add_argument("--output-dir", type=Path, required=True)
    package_parser = commands.add_parser("package")
    package_parser.add_argument("--target", choices=TARGETS, required=True)
    package_parser.add_argument("--inputs-dir", type=Path, required=True)
    package_parser.add_argument("--output", type=Path, required=True)
    check_parser = commands.add_parser("check-package")
    check_parser.add_argument("--target", choices=TARGETS, required=True)
    check_parser.add_argument("--version", required=True)
    check_parser.add_argument("package", type=Path)
    collect_parser = commands.add_parser("collect")
    collect_parser.add_argument("--output-dir", type=Path, required=True)
    select_parser = commands.add_parser("selection")
    select_parser.add_argument("--manifest-file", type=Path, required=True)
    select_parser.add_argument("--artifact-id", type=int, required=True)
    select_parser.add_argument("--artifact-digest", required=True)
    commands.add_parser("publish")
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    handlers = {"resolve": lambda _: resolve(), "ci-green": lambda _: ci_green(), "fetch-inputs": fetch_inputs,
                "package": package, "check-package": check_package, "collect": collect,
                "selection": selection, "publish": publish}
    try:
        handlers[args.command](args)
    except (ProtocolError, rc.assets.ReleaseAssetError, OSError, ValueError, KeyError, TypeError,
            tarfile.TarError, zipfile.BadZipFile) as error:
        print(f"forge release error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
