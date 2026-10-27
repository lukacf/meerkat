#!/usr/bin/env python3
"""Meta release protocol: immutable candidates and add-only publication.

Schema 1 (candidates and releases up to v0.9.1) binds all five targets in one
candidate. Schema 2 splits them into tiers (verify-release-assets.py
TIER_TARGETS): every candidate run builds and binds exactly one tier, both
tiers come from the same source SHA, promotion publishes only a core
selection, and the trailing tier is added afterwards, add-only, by assets-mode
recovery bound to the published core receipt. Verification and registry
retry require the core tier only.

Selections are strict JSON objects (all hashes are lowercase SHA-256 hex):
  accepted-candidate: schema_version=1, kind, repository, run_id, run_attempt,
    manifest_artifact={id, digest, manifest_sha256}; schema_version=2 adds
    tier ("core" or "trailing").
  original-assets: schema_version=1, kind, repository, source_sha, version, tag,
    run_id, run_attempt, metadata={"index.json": hash, "checksums.sha256": hash},
    artifacts=[{id, name, digest, target, archives=[archive_record, ...]}, ...].
An original-assets selection lists only explicitly authorized archive additions.
Archive records have name, target, binary, size, sha256, and
inner={name, size, sha256}. IDs, attempts and sizes are positive JSON integers.

Candidate manifests have schema_version=1, kind="candidate-manifest", repository,
source_sha, version, tag, run_id, run_attempt, workflow={path, sha, ref},
ci={run_id, run_attempt}, artifacts (the complete matrix), and metadata.
Schema 2 manifests add tier, and their artifacts and metadata are that tier's.
The external selection identifies the manifest artifact, avoiding a self-hash.
Neither a selection nor decoded provenance alone authorizes publication:
original attestations must pass gh's cryptographic verifier and strict policy.
Candidates require retained bundles. Explicit legacy recovery may retrieve an
existing GitHub attestation when its original archive ZIP predates bundle retention.
"""

import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import time
import tomllib
import uuid
import zipfile


SPEC = importlib.util.spec_from_file_location(
    "verify_release_assets", Path(__file__).with_name("verify-release-assets.py")
)
assert SPEC is not None and SPEC.loader is not None
assets = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(assets)

REPOSITORY = "lukacf/meerkat-meta"
WORKFLOW = ".github/workflows/release.yml"
MAIN = "refs/heads/main"
MANIFEST = "candidate-manifest.json"
SELECTION = "candidate-selection.json"
PROVENANCE = "provenance.jsonl"
MANIFEST_PROVENANCE = "manifest-provenance.jsonl"
RELEASE_NOTES = (
    "Original accepted Meta candidate; see the attached selections and provenance. "
    "Linux x86_64 and aarch64 and macOS arm64 archives are listed in index.json and checksums.sha256. "
    "macOS x86_64 and Windows archives may arrive later, listed in index-trailing.json and "
    "checksums-trailing.sha256."
)
SCHEMAS = (1, 2)
# Every release build leg. A schema 2 candidate run builds one tier's legs;
# the resolver emits them as the build_binaries matrix. Each leg names its
# runner image (scripts/test_ci_workflow.py refuses a -latest label).
BUILD_LEGS = (
    {"runs-on": "ubuntu-24.04", "target": "x86_64-unknown-linux-gnu", "archive_ext": "tar.gz"},
    {"runs-on": "ubuntu-24.04-arm", "target": "aarch64-unknown-linux-gnu", "archive_ext": "tar.gz"},
    {"runs-on": "macos-15", "target": "aarch64-apple-darwin", "archive_ext": "tar.gz"},
    {"runs-on": "macos-15", "target": "x86_64-apple-darwin", "archive_ext": "tar.gz"},
    # A GitHub-hosted label offered to public repositories only.
    {"runs-on": "windows-2025-vs2026", "target": "x86_64-pc-windows-msvc", "archive_ext": "zip"},
)
SLSA = "https://slsa.dev/provenance/v1"
BINARY_NAMES = dict(zip(assets.ARCHIVE_PREFIXES, ("meta_gateway", "rpc_gateway", "meta_repair")))
MAX_MEMBER = 2 * 1024**3
# A just-started run can report a stale status for minutes, and under load
# GitHub answers a read with 5xx or drops the connection. Collection reads its
# own run again RUN_REREAD_DELAY seconds apart, for up to RUN_REREAD_ATTEMPTS
# reads (10 minutes), while the run does not report in progress yet or the read
# failed that way, and reads the run's artifact listing again only after such a
# failed read. Any other refusal, such as a run identity mismatch or a listing
# that misses an artifact, stays immediate.
RUN_REREAD_ATTEMPTS = 40
RUN_REREAD_DELAY = 15
# api() runs `gh api` once, and run() reports a failure as "gh api failed: ...".
TRANSIENT_API = re.compile(r"gh api failed: .*(?:\bHTTP 5[0-9][0-9]\b|error connecting to|connection reset|"
                           r"connection refused|timeout|timed out|deadline exceeded|\bEOF\b|GOAWAY|broken pipe)",
                           re.I | re.S)


class ProtocolError(RuntimeError):
    pass


class RunNotInProgress(ProtocolError):
    """The candidate run's own record is right in every other way, but GitHub
    does not report the run in progress yet."""

    def __init__(self, message, status):
        super().__init__(message)
        self.status = status


def require(condition, message):
    if not condition:
        raise ProtocolError(message)


def exact_keys(value, keys, label):
    require(isinstance(value, dict) and set(value) == set(keys.split()), f"invalid {label} fields")


def integer(value, label):
    require(type(value) is int and value > 0, f"{label} must be a positive integer")
    return value


def digest(value, label="digest", length=64):
    require(isinstance(value, str) and re.fullmatch(f"[0-9a-f]{{{length}}}", value),
            f"invalid {label}")
    return value


def env_bool(name):
    value = os.environ.get(name, "false")
    require(value in ("true", "false"), f"{name} must be true or false")
    return value == "true"


def env_int(name):
    value = os.environ.get(name, "")
    require(re.fullmatch("[1-9][0-9]*", value), f"invalid {name}")
    return int(value)


def repository():
    value = os.environ.get("GITHUB_REPOSITORY", REPOSITORY)
    require(value == REPOSITORY, "unexpected repository")
    return value


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, f"duplicate JSON key: {key}")
        value[key] = item
    return value


def parse_json(value):
    return json.loads(value, object_pairs_hook=unique_object)


def read_json(path):
    require(path.is_file() and not path.is_symlink(), f"not a regular file: {path}")
    return parse_json(path.read_text(encoding="utf-8"))


def json_bytes(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def write_new(path, data):
    require(not path.is_symlink(), f"symlink refused: {path}")
    with path.open("xb") as handle:
        handle.write(data)


def hash_bytes(value):
    return hashlib.sha256(value).hexdigest()


def hash_stream(handle):
    value = hashlib.sha256()
    size = 0
    for block in iter(lambda: handle.read(1024 * 1024), b""):
        size += len(block)
        require(size <= MAX_MEMBER, "archive member exceeds policy size limit")
        value.update(block)
    return size, value.hexdigest()


@contextlib.contextmanager
def workspace():
    path = Path.cwd() / (".release-candidate-work-" + uuid.uuid4().hex)
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path)


def run(argv, *, data=None, output=None, env=None):
    result = subprocess.run(argv, input=data, stdout=output or subprocess.PIPE,
                            stderr=subprocess.PIPE, env=env, check=False)
    require(result.returncode == 0,
            f"{argv[0]} {argv[1]} failed: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def git(*args):
    return run(["git", *args]).decode().strip()


def api(endpoint, *, pages=False):
    argv = ["gh", "api", endpoint]
    if pages:
        argv += ["--paginate", "--slurp"]
    return parse_json(run(argv))


def api_download(endpoint, path, *, kind):
    require(kind in ("actions-artifact", "release-asset"), "unknown download kind")
    argv = ["gh", "api", endpoint]
    # Actions ZIP downloads negotiate JSON with the API before its redirect;
    # release assets need octet-stream to request bytes instead of metadata.
    if kind == "release-asset":
        argv += ["--header", "Accept: application/octet-stream"]
    handle = path.open("xb")
    complete = False
    try:
        with handle:
            run(argv, output=handle)
        complete = True
    finally:
        if not complete:
            path.unlink()


def repo_endpoint(suffix):
    return f"repos/{repository()}/{suffix}"


def verify_remote_tag(tag, sha):
    obj = api(repo_endpoint(f"git/ref/tags/{tag}"))["object"]
    for _ in range(8):
        if obj["type"] == "commit":
            require(obj["sha"] == sha, "remote tag source changed or mismatched")
            return
        require(obj["type"] == "tag", "tag does not resolve to a commit")
        obj = api(repo_endpoint(f"git/tags/{digest(obj['sha'], 'tag object', 40)}"))["object"]
    raise ProtocolError("excessively nested annotated tag")


def version_at(sha):
    value = tomllib.loads(git("show", f"{sha}:Cargo.toml"))["workspace"]["package"]["version"]
    require(isinstance(value, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", value),
            "invalid workspace version")
    return value


def valid_tag(tag):
    require(isinstance(tag, str) and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", tag),
            "release tag must be v<workspace version>")
    return tag


def binary_member_name(binary, target):
    require(binary in BINARY_NAMES.values() and target in dict(assets.TARGET_ARCHIVES),
            "unknown binary/target")
    # The unchanged Windows 7z producer retains this build-tree path; Unix tar
    # explicitly changes directory and stores only the executable basename.
    if dict(assets.TARGET_ARCHIVES)[target] == "zip":
        return f"target/{target}/release/{binary}.exe"
    return binary


def tier_proof_names(tier):
    """(receipt, manifest, manifest bundle) names on a release for a tier.

    Schema 1 and the core tier keep the original names; the trailing tier's
    carry its name, so adding them never touches a published core file.
    """
    if tier in (None, "core"):
        return SELECTION, MANIFEST, MANIFEST_PROVENANCE
    require(tier in assets.TIERS, "unknown candidate tier")
    return f"candidate-selection-{tier}.json", f"candidate-manifest-{tier}.json", f"manifest-provenance-{tier}.jsonl"


def tier_targets(tier):
    return tuple(t for t, _ in assets.TARGET_ARCHIVES) if tier is None else assets.TIER_TARGETS[tier]


def schema_tier(value):
    """None for schema 1; the validated tier for schema 2."""
    require(type(value.get("schema_version")) is int and value["schema_version"] in SCHEMAS,
            "unsupported schema version")
    if value["schema_version"] == 1:
        require("tier" not in value, "schema 1 has no tier")
        return None
    require(value.get("tier") in assets.TIERS, "schema 2 requires tier core or trailing")
    return value["tier"]


def build_matrix(tier):
    return {"include": [leg for leg in BUILD_LEGS if leg["target"] in tier_targets(tier)]}


def validate_archive_record(record, version, target):
    exact_keys(record, "name target binary size sha256 inner", "archive")
    require(record["target"] == target, "archive target mismatch")
    extensions = dict(assets.TARGET_ARCHIVES)
    require(target in extensions, "unknown target")
    names = {f"{prefix}-{version}-{target}.{extensions[target]}": binary
             for prefix, binary in BINARY_NAMES.items()}
    require(record["name"] in names and record["binary"] == names[record["name"]],
            "archive/binary mapping mismatch")
    integer(record["size"], "archive size")
    digest(record["sha256"])
    exact_keys(record["inner"], "name size sha256", "inner archive")
    expected = binary_member_name(record["binary"], target)
    require(record["inner"]["name"] == expected, "inner binary name mismatch")
    integer(record["inner"]["size"], "inner size")
    digest(record["inner"]["sha256"])


def validate_artifacts(items, version, *, complete, attempt, tier=None):
    allowed = tier_targets(tier)
    require(isinstance(items, list) and 0 < len(items) <= len(allowed),
            "invalid target artifact inventory")
    ids, names, targets, archives_seen = set(), set(), set(), set()
    for item in items:
        exact_keys(item, "id name digest target archives", "target artifact")
        integer(item["id"], "artifact ID")
        digest(item["digest"])
        target = item["target"]
        require(target in dict(assets.TARGET_ARCHIVES), "unknown target")
        require(target in allowed, "target outside the candidate tier")
        require(isinstance(item["name"], str) and re.fullmatch(r"[A-Za-z0-9_.-]+", item["name"]),
                "invalid artifact name")
        require(item["id"] not in ids and item["name"] not in names and target not in targets,
                "duplicate artifact ID/name/target")
        ids.add(item["id"])
        names.add(item["name"])
        targets.add(target)
        if complete:
            require(item["name"] == f"meta-gateway-binaries-{target}-{attempt}",
                    "artifact name/attempt mismatch")
        require(isinstance(item["archives"], list) and 0 < len(item["archives"]) <= 3,
                "invalid artifact archive inventory")
        for record in item["archives"]:
            validate_archive_record(record, version, target)
            require(record["name"] not in archives_seen, "duplicate archive")
            archives_seen.add(record["name"])
        if complete:
            require(len(item["archives"]) == 3, "missing target archives")
    if complete:
        require(archives_seen == set(assets.expected_archive_names(version, tier)), "missing targets/archives")


def validate_identity(value):
    schema_tier(value)
    require(value["repository"] == repository(), "selection repository mismatch")
    integer(value["run_id"], "run ID")
    integer(value["run_attempt"], "run attempt")


def validate_source(value, tier=None):
    digest(value["source_sha"], "source SHA", 40)
    valid_tag(value["tag"])
    require(value["tag"] == "v" + value["version"], "tag/version mismatch")
    exact_keys(value["metadata"], " ".join(assets.metadata_names(tier)), "metadata")
    for value_hash in value["metadata"].values():
        digest(value_hash)


def validate_selection(value, *, original=False):
    require(isinstance(value, dict), "selection must be an object")
    if value.get("kind") == "accepted-candidate":
        tier = schema_tier(value)
        exact_keys(value, "schema_version kind repository run_id run_attempt manifest_artifact"
                   + ("" if tier is None else " tier"), "selection")
        validate_identity(value)
        exact_keys(value["manifest_artifact"], "id digest manifest_sha256", "manifest artifact")
        integer(value["manifest_artifact"]["id"], "manifest artifact ID")
        digest(value["manifest_artifact"]["digest"])
        digest(value["manifest_artifact"]["manifest_sha256"])
    else:
        require(original and value.get("kind") == "original-assets", "accepted-candidate selection required")
        exact_keys(value, "schema_version kind repository source_sha version tag run_id run_attempt metadata artifacts",
                   "original-assets selection")
        require(value["schema_version"] == 1, "original-assets selections are schema 1")
        validate_identity(value)
        validate_source(value)
        validate_artifacts(value["artifacts"], value["version"], complete=False, attempt=value["run_attempt"])
    return value


def selection_env(*, original=False):
    return validate_selection(parse_json(os.environ.get("ARTIFACT_SELECTION", "")), original=original)


def validate_manifest(value):
    require(isinstance(value, dict), "candidate manifest must be an object")
    tier = schema_tier(value)
    exact_keys(value, "schema_version kind repository source_sha version tag run_id run_attempt workflow ci artifacts metadata"
               + ("" if tier is None else " tier"), "candidate manifest")
    validate_identity(value)
    validate_source(value, tier)
    require(value["kind"] == "candidate-manifest", "invalid manifest kind")
    exact_keys(value["workflow"], "path sha ref", "workflow")
    require(value["workflow"] == {"path": WORKFLOW, "sha": value["source_sha"], "ref": MAIN},
            "candidate workflow identity mismatch")
    exact_keys(value["ci"], "run_id run_attempt", "CI")
    integer(value["ci"]["run_id"], "CI run")
    integer(value["ci"]["run_attempt"], "CI attempt")
    validate_artifacts(value["artifacts"], value["version"], complete=True, attempt=value["run_attempt"], tier=tier)
    return value


def selection_tier(selected):
    return None if selected["schema_version"] == 1 else selected["tier"]


def resolve():
    repository()
    event, ref = os.environ.get("GITHUB_EVENT_NAME"), os.environ.get("GITHUB_REF", "")
    head = digest(git("rev-parse", "HEAD"), "checkout SHA", 40)
    tier = ""
    if event == "push" and ref.startswith("refs/tags/v"):
        mode, tag, publish, dry = "tag", valid_tag(ref.removeprefix("refs/tags/")), False, False
    else:
        require(event == "workflow_dispatch", "unsupported release event")
        mode = os.environ.get("RELEASE_MODE", "existing")
        require(mode in ("candidate", "promote", "existing", "assets"), "unknown release mode")
        tag = os.environ.get("RELEASE_TAG", "")
        source = os.environ.get("SOURCE_SHA", "")
        selected = os.environ.get("ARTIFACT_SELECTION", "")
        tier = os.environ.get("RELEASE_TIER", "")
        publish, dry = env_bool("PUBLISH_RELEASE_PACKAGES"), env_bool("REGISTRY_DRY_RUN")
        if mode == "candidate":
            require(ref == MAIN, "candidate dispatch must use main")
            require(not tag and not selected and not publish and not dry, "contradictory candidate inputs")
            require(tier in assets.TIERS, "candidate dispatch requires tier core or trailing")
            digest(source, "explicit SOURCE_SHA", 40)
            require(source == os.environ.get("GITHUB_SHA") == os.environ.get("GITHUB_WORKFLOW_SHA") == head,
                    "candidate source/event/workflow/checkout SHA mismatch")
        else:
            require(not source, "SOURCE_SHA is only valid for candidate")
            require(not tier, "a candidate tier is only valid for candidate")
            if mode in ("promote", "assets") or tag:
                require(ref == MAIN, "tagged release dispatch must use main")
                valid_tag(tag)
            if mode in ("promote", "assets"):
                chosen = selection_env(original=mode == "assets")
                if mode == "promote":
                    require_promotable(chosen)
            else:
                require(not selected, "selection is not valid for existing mode")
                require(tag or not publish or dry, "untagged real registry publication refused")
            if mode == "assets":
                require(not publish and not dry, "assets recovery requires both publication booleans false")
    if tag:
        sha = digest(git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"), "tag source", 40)
        verify_remote_tag(tag, sha)
    else:
        sha = head
        require(sha == os.environ.get("GITHUB_SHA"), "source checkout/event SHA mismatch")
    version = version_at(sha)
    require(not tag or tag == "v" + version, "tag does not match workspace version")
    result = {
        "mode": mode, "sha": sha, "version": version,
        "tag": "v" + version if mode == "candidate" else tag,
        "publish_packages": str(publish).lower(), "dry_run": str(dry).lower(),
        "ci_bypass": str(mode == "existing" and not tag and publish and dry).lower(),
        "tier": tier,
        # build_binaries skips outside candidate mode before reading its
        # matrix; it still receives a valid one.
        "build_matrix": json.dumps(build_matrix(tier or "core"), separators=(",", ":"), sort_keys=True),
    }
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as handle:
            for key, value in result.items():
                handle.write(f"{key}={value}\n")
    print(json.dumps(result, sort_keys=True))
    return result


def source_context():
    repository()
    sha = digest(os.environ.get("RELEASE_SHA"), "RELEASE_SHA", 40)
    version = os.environ.get("RELEASE_VERSION", "")
    tag = valid_tag(os.environ.get("RELEASE_TAG", ""))
    require(tag == "v" + version, "resolved version/tag mismatch")
    require(git("rev-parse", "HEAD") == sha and version_at(sha) == version,
            "source checkout/version mismatch")
    return sha, version, tag


def verify_run(run_id, attempt, sha, *, ci=False, collecting=False, legacy_tag=None):
    value = api(repo_endpoint(f"actions/runs/{integer(run_id, 'run ID')}"))
    expected_path = ".github/workflows/ci.yml" if ci else WORKFLOW
    expected_event = "push" if ci or legacy_tag else "workflow_dispatch"
    expected_branch = legacy_tag if legacy_tag else "main"
    require(value["id"] == run_id and value["run_attempt"] == attempt, "run identity/attempt changed")
    require(value["repository"]["full_name"] == repository()
            and value["head_repository"]["full_name"] == repository(), "run repository mismatch")
    require(value["head_sha"] == sha and value["head_branch"] == expected_branch
            and value["event"] == expected_event and value["path"] == expected_path,
            "run source/workflow/event/ref mismatch")
    if collecting:
        if value["status"] != "in_progress":
            raise RunNotInProgress("collection requires its in-progress candidate run", value["status"])
    else:
        require(value["status"] == "completed" and value["conclusion"] == "success",
                "run is not completed-success")
    return value


def reread(label, read):
    """The first answer of read(). A read that finds the run not in progress
    yet, or that fails transiently, is read again (see RUN_REREAD_ATTEMPTS);
    each such read logs one line to stderr. Any other refusal propagates at
    once."""
    started = time.monotonic()
    for attempt in range(1, RUN_REREAD_ATTEMPTS + 1):
        try:
            return read()
        except RunNotInProgress as pending:
            reason, refusal = f"status={pending.status}", str(pending)
        except ProtocolError as error:
            if not TRANSIENT_API.match(str(error)):
                raise
            reason, refusal = f"transient API failure: {error}", str(error)
        elapsed = f"{time.monotonic() - started:.0f}"
        print(f"{label}: read {attempt}/{RUN_REREAD_ATTEMPTS} missed at {elapsed} s: "
              + " ".join(reason.split()), file=sys.stderr)
        if attempt < RUN_REREAD_ATTEMPTS:
            time.sleep(RUN_REREAD_DELAY)
    raise ProtocolError(f"{refusal} (after {RUN_REREAD_ATTEMPTS} reads over {elapsed} s)")


def collecting_run(run_id, attempt, sha):
    """This candidate run's own record, once GitHub reports it in progress."""
    return reread(f"candidate run {run_id}", lambda: verify_run(run_id, attempt, sha, collecting=True))


def download_artifact(identity, run_info, directory):
    artifact_id = integer(identity["id"], "artifact ID")
    value = api(repo_endpoint(f"actions/artifacts/{artifact_id}"))
    require(value["id"] == artifact_id and value["name"] == identity["name"], "artifact identity/name mismatch")
    require(value["expired"] is False, f"original artifact {artifact_id} expired")
    require(value["digest"] == "sha256:" + digest(identity["digest"]), "artifact API ZIP digest mismatch")
    owner = value["workflow_run"]
    require(owner["id"] == run_info["id"] and owner["head_sha"] == run_info["head_sha"]
            and owner["repository_id"] == run_info["repository"]["id"]
            and owner["head_repository_id"] == run_info["head_repository"]["id"],
            "artifact run/repository/source mismatch")
    path = directory / f"artifact-{artifact_id}.zip"
    api_download(repo_endpoint(f"actions/artifacts/{artifact_id}/zip"), path, kind="actions-artifact")
    require(assets.sha256(path) == identity["digest"], "downloaded artifact ZIP digest mismatch")
    return path


def zip_members(archive, *, expected_member=None):
    infos = archive.infolist()
    names = [info.filename for info in infos]
    require(len(names) == len(set(names)), "duplicate ZIP members")
    for info in infos:
        allowed_path = (
            info.filename == expected_member if expected_member is not None
            else info.filename and "/" not in info.filename and "\\" not in info.filename
        )
        require(allowed_path and info.filename not in (".", "..") and not info.is_dir(),
                "unsafe ZIP member path")
        mode = info.external_attr >> 16
        require(stat.S_IFMT(mode) in (0, stat.S_IFREG) and not info.external_attr & 0x10,
                "ZIP links/directories/special files refused")
        require(not info.flag_bits & 1 and 0 < info.file_size <= MAX_MEMBER, "invalid ZIP member size/encryption")
    return infos


def unpack_artifact(path, destination, expected):
    destination.mkdir()
    with zipfile.ZipFile(path) as archive:
        infos = zip_members(archive)
        require({info.filename for info in infos} == set(expected), "artifact member inventory mismatch")
        for info in infos:
            with archive.open(info) as source, (destination / info.filename).open("xb") as target:
                shutil.copyfileobj(source, target, 1024 * 1024)
    return destination


def describe_archive(path, version, target):
    extension = dict(assets.TARGET_ARCHIVES)[target]
    mapping = {f"{prefix}-{version}-{target}.{extension}": binary for prefix, binary in BINARY_NAMES.items()}
    require(path.name in mapping, "unexpected archive name")
    binary = mapping[path.name]
    member_name = binary_member_name(binary, target)
    if extension == "zip":
        with zipfile.ZipFile(path) as archive:
            infos = zip_members(archive, expected_member=member_name)
            require(len(infos) == 1 and infos[0].filename == member_name, "unexpected inner ZIP members")
            with archive.open(infos[0]) as member:
                size, inner_hash = hash_stream(member)
    else:
        with tarfile.open(path, "r:gz") as archive:
            infos = archive.getmembers()
            require(len(infos) == 1 and infos[0].name == member_name and infos[0].isfile()
                    and not infos[0].issym() and not infos[0].islnk()
                    and 0 < infos[0].size <= MAX_MEMBER, "unsafe/unexpected inner TAR members")
            with archive.extractfile(infos[0]) as member:
                size, inner_hash = hash_stream(member)
    return {"name": path.name, "target": target, "binary": binary,
            "size": path.stat().st_size, "sha256": assets.sha256(path),
            "inner": {"name": member_name, "size": size, "sha256": inner_hash}}


def verify_provenance(path, bundle, sha, run_id, attempt, *, ref=MAIN, event="workflow_dispatch",
                      allow_remote=False):
    if bundle is None:
        require(allow_remote and event == "push" and ref.startswith("refs/tags/"),
                "missing original attestation bundle")
        bundle_args = []
    else:
        require(bundle.is_file() and not bundle.is_symlink() and bundle.stat().st_size > 0,
                "missing original attestation bundle")
        bundle_args = ["--bundle", str(bundle)]
    repo_url = "https://github.com/" + repository()
    identity = f"{repo_url}/{WORKFLOW}@{ref}"
    invocation = f"{repo_url}/actions/runs/{run_id}/attempts/{attempt}"
    output = parse_json(run([
        "gh", "attestation", "verify", str(path), "--repo", repository(),
        *bundle_args, "--source-digest", sha, "--source-ref", ref,
        "--signer-digest", sha, "--cert-identity", identity,
        "--cert-oidc-issuer", "https://token.actions.githubusercontent.com",
        "--deny-self-hosted-runners", "--predicate-type", SLSA, "--format", "json",
    ]))
    require(isinstance(output, list) and output, "no cryptographically verified attestations")
    for entry in output:
        result = entry["verificationResult"]
        cert = result["signature"]["certificate"]
        expected = {
            "issuer": "https://token.actions.githubusercontent.com",
            "subjectAlternativeName": identity, "buildSignerURI": identity,
            "buildSignerDigest": sha, "sourceRepositoryURI": repo_url,
            "sourceRepositoryDigest": sha, "sourceRepositoryRef": ref,
            "buildConfigURI": identity, "buildConfigDigest": sha,
            "runnerEnvironment": "github-hosted", "buildTrigger": event,
            "runInvocationURI": invocation,
        }
        for key, value in expected.items():
            require(cert.get(key) == value, f"verified certificate {key} mismatch/missing")
        require(result.get("verifiedTimestamps"), "missing verified timestamp")
        statement = result["statement"]
        require(statement["_type"] == "https://in-toto.io/Statement/v1"
                and statement["predicateType"] == SLSA, "wrong attestation statement type")
        matches = [s for s in statement["subject"] if s.get("name") == path.name]
        require(len(matches) == 1 and matches[0]["digest"] == {"sha256": assets.sha256(path)},
                "attestation subject mismatch")
        definition = statement["predicate"]["buildDefinition"]
        require(definition["buildType"] == "https://actions.github.io/buildtypes/workflow/v1",
                "wrong provenance build type")
        require(definition["externalParameters"]["workflow"] ==
                {"repository": repo_url, "path": WORKFLOW, "ref": ref}, "provenance workflow mismatch")
        internal = definition["internalParameters"]["github"]
        require(internal["event_name"] == event and internal["runner_environment"] == "github-hosted",
                "provenance event/runner mismatch")
        require(definition["resolvedDependencies"] ==
                [{"uri": f"git+{repo_url}@{ref}", "digest": {"gitCommit": sha}}],
                "provenance source dependency mismatch")
        details = statement["predicate"]["runDetails"]
        require(details["builder"]["id"] == identity and details["metadata"]["invocationId"] == invocation,
                "provenance invocation/builder mismatch")


def collect(args):
    require(os.environ.get("RELEASE_MODE") == "candidate", "collect is candidate-only")
    tier = args.tier
    require((tier or "") == os.environ.get("RELEASE_TIER", ""),
            "collection tier differs from the resolved candidate tier")
    require(not env_bool("PUBLISH_RELEASE_PACKAGES") and not env_bool("REGISTRY_DRY_RUN"),
            "candidate collection cannot request publication")
    sha, version, tag = source_context()
    require(os.environ.get("GITHUB_REF") == MAIN and os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
            and sha == os.environ.get("GITHUB_SHA") == os.environ.get("GITHUB_WORKFLOW_SHA"),
            "candidate collection source identity mismatch")
    run_id, attempt = env_int("GITHUB_RUN_ID"), env_int("GITHUB_RUN_ATTEMPT")
    info = collecting_run(run_id, attempt, sha)
    verify_run(args.ci_run_id, args.ci_run_attempt, sha, ci=True)
    # Only a transient failure of this read is read again: a listing that
    # misses an artifact is refused below, at once.
    endpoint = repo_endpoint(f"actions/runs/{run_id}/artifacts?per_page=100")
    pages = reread(f"candidate run {run_id} artifacts", lambda: api(endpoint, pages=True))
    listing = [item for page in pages for item in page["artifacts"]]
    expected = {f"meta-gateway-binaries-{target}-{attempt}": target for target in tier_targets(tier)}
    selected = [a for a in listing if a["name"].startswith("meta-gateway-binaries-")]
    require(len(selected) == len(expected) and {a["name"] for a in selected} == set(expected),
            "missing/duplicate/unexpected target artifacts or mixed attempts")
    require(len({a["id"] for a in selected}) == len(expected), "duplicate artifact IDs")
    require(not args.output_dir.exists(), "candidate output directory already exists")
    with workspace() as work:
        downloaded = work / "targets"
        downloaded.mkdir()
        records = []
        for item in sorted(selected, key=lambda a: a["name"]):
            require(isinstance(item["digest"], str) and item["digest"].startswith("sha256:"),
                    "missing artifact ZIP digest")
            identity = {"id": item["id"], "name": item["name"], "digest": item["digest"][7:]}
            target = expected[item["name"]]
            archive_names = [n for n in assets.expected_archive_names(version, tier) if f"-{target}." in n]
            packed = download_artifact(identity, info, work)
            folder = unpack_artifact(packed, downloaded / target, [*archive_names, PROVENANCE])
            archive_records = []
            for name in archive_names:
                verify_provenance(folder / name, folder / PROVENANCE, sha, run_id, attempt)
                archive_records.append(describe_archive(folder / name, version, target))
            records.append({**identity, "target": target, "archives": archive_records})
        prepared = work / "prepared"
        assets.prepare_assets(downloaded, prepared, tag, tier)
        manifest = {
            "schema_version": 1 if tier is None else 2, "kind": "candidate-manifest", "repository": repository(),
            "source_sha": sha, "version": version, "tag": tag,
            "workflow": {"path": WORKFLOW, "sha": sha, "ref": MAIN},
            "run_id": run_id, "run_attempt": attempt,
            "ci": {"run_id": args.ci_run_id, "run_attempt": args.ci_run_attempt},
            "artifacts": records,
            "metadata": {name: assets.sha256(prepared / name) for name in assets.metadata_names(tier)},
        }
        if tier is not None:
            manifest["tier"] = tier
        validate_manifest(manifest)
        args.output_dir.mkdir()
        write_new(args.output_dir / tier_proof_names(tier)[1], json_bytes(manifest))
        for name in manifest["metadata"]:
            write_new(args.output_dir / name, (prepared / name).read_bytes())
    print(f"collected exact {tier or 'schema 1'} candidate {run_id}/{attempt}; no release or tag writes")


def selection(args):
    manifest = validate_manifest(read_json(args.manifest_file))
    tier = schema_tier(manifest)
    require(args.manifest_file.name == tier_proof_names(tier)[1], "manifest file name does not match its tier")
    value = {
        "schema_version": manifest["schema_version"], "kind": "accepted-candidate", "repository": manifest["repository"],
        "run_id": manifest["run_id"], "run_attempt": manifest["run_attempt"],
        "manifest_artifact": {"id": args.artifact_id, "digest": digest(args.artifact_digest),
                              "manifest_sha256": assets.sha256(args.manifest_file)},
    }
    if tier is not None:
        value["tier"] = tier
    validate_selection(value)
    require(args.artifact_id not in {item["id"] for item in manifest["artifacts"]},
            "manifest artifact ID duplicates target artifact")
    encoded = json_bytes(value).decode()
    print(encoded, end="")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as handle:
            handle.write(f"## Candidate selection (save outside the repository)\n\n```json\n{encoded}```\n\n")
            handle.write(f"Run: https://github.com/{repository()}/actions/runs/{value['run_id']}/attempts/{value['run_attempt']}\n\n")
            handle.write(f"Artifact download: `gh api repos/{repository()}/actions/artifacts/{args.artifact_id}/zip > candidate.zip`\n\n")
            handle.write("Acceptance and later promotion authorize these exact bytes, not a rebuild.\n")


def match_manifest_selection(manifest, selected, path, sha, version, tag):
    validate_manifest(manifest)
    require(manifest["schema_version"] == selected["schema_version"]
            and schema_tier(manifest) == selection_tier(selected), "candidate schema/tier mismatch")
    require(manifest["run_id"] == selected["run_id"] and manifest["run_attempt"] == selected["run_attempt"],
            "manifest run/attempt mismatch")
    require(manifest["source_sha"] == sha and manifest["version"] == version and manifest["tag"] == tag,
            "manifest source/version/tag mismatch")
    require(assets.sha256(path) == selected["manifest_artifact"]["manifest_sha256"], "manifest file hash mismatch")
    require(selected["manifest_artifact"]["id"] not in {a["id"] for a in manifest["artifacts"]},
            "manifest/target artifact IDs overlap")


def check_metadata(manifest, folder):
    for name, expected_hash in manifest["metadata"].items():
        require(assets.sha256(folder / name) == expected_hash, f"original {name} hash mismatch")


def verify_prepared(folder, tag, tier):
    """The flat assets of schema 1, of the core tier, or of the trailing tier alone."""
    if tier is None:
        assets.verify_assets(folder, tag)
    elif tier == "core":
        assets.verify_assets(folder, tag, ("core",))
    else:
        assets.verify_tier_assets(folder, tag, tier)


def tier_file_names(version, tier):
    """Every release file a tier owns: archives, metadata and proofs."""
    receipt, manifest_name, bundle = tier_proof_names(tier)
    return (set(assets.expected_archive_names(version, tier)) | set(assets.metadata_names(tier))
            | {receipt, manifest_name, bundle} | {f"target-{t}-provenance.jsonl" for t in tier_targets(tier)})


def manifest_members(tier):
    """The files of a candidate run's manifest artifact for a tier."""
    return {tier_proof_names(tier)[1], PROVENANCE, *assets.metadata_names(tier)}


def stage_candidate(selected, output_dir, proof_dir, work):
    sha, version, tag = source_context()
    tier = selection_tier(selected)
    receipt, manifest_name, manifest_bundle = tier_proof_names(tier)
    require(not output_dir.exists() and not proof_dir.exists(), "staging directories already exist")
    verify_remote_tag(tag, sha)
    info = verify_run(selected["run_id"], selected["run_attempt"], sha)
    identity = {**selected["manifest_artifact"],
                "name": f"meta-candidate-manifest-{selected['run_attempt']}"}
    packed = download_artifact(identity, info, work)
    with zipfile.ZipFile(packed) as archive:
        present = {info.filename for info in zip_members(archive)}
    require(not any(present == manifest_members(other) != manifest_members(tier) for other in (None, *assets.TIERS)),
            "candidate schema/tier mismatch: the selection names another tier than its manifest artifact")
    folder = unpack_artifact(packed, work / "manifest", sorted(manifest_members(tier)))
    manifest = read_json(folder / manifest_name)
    match_manifest_selection(manifest, selected, folder / manifest_name, sha, version, tag)
    verify_provenance(folder / manifest_name, folder / PROVENANCE, sha, selected["run_id"], selected["run_attempt"])
    verify_run(manifest["ci"]["run_id"], manifest["ci"]["run_attempt"], sha, ci=True)
    check_metadata(manifest, folder)
    prepared, proofs = work / "verified-assets", work / "verified-proofs"
    prepared.mkdir()
    proofs.mkdir()
    write_new(proofs / manifest_name, (folder / manifest_name).read_bytes())
    write_new(proofs / receipt, json_bytes(selected))
    write_new(proofs / manifest_bundle, (folder / PROVENANCE).read_bytes())
    for item in manifest["artifacts"]:
        packed = download_artifact(item, info, work)
        names = [record["name"] for record in item["archives"]]
        target_dir = unpack_artifact(packed, work / item["target"], [*names, PROVENANCE])
        for record in item["archives"]:
            path = target_dir / record["name"]
            require(describe_archive(path, version, item["target"]) == record,
                    "original archive outer/inner bytes mismatch")
            verify_provenance(path, target_dir / PROVENANCE, sha, selected["run_id"], selected["run_attempt"])
            shutil.copyfile(path, prepared / path.name)
            require(assets.sha256(prepared / path.name) == record["sha256"], "staged copy hash mismatch")
        write_new(proofs / f"target-{item['target']}-provenance.jsonl", (target_dir / PROVENANCE).read_bytes())
    for name in manifest["metadata"]:
        write_new(prepared / name, (folder / name).read_bytes())
    verify_prepared(prepared, tag, tier)
    check_metadata(manifest, prepared)
    verify_run(selected["run_id"], selected["run_attempt"], sha)
    verify_remote_tag(tag, sha)
    shutil.copytree(prepared, output_dir)
    shutil.copytree(proofs, proof_dir)


def stage(args):
    require(os.environ.get("RELEASE_MODE") in ("promote", "assets"), "stage requires promote/assets mode")
    selected = selection_env()
    if os.environ.get("RELEASE_MODE") == "promote":
        require_promotable(selected)
    with workspace() as work:
        stage_candidate(selected, args.output_dir, args.proof_dir, work)
    print("verified and staged original candidate bytes; no publication")


def require_promotable(selected):
    # Schema 1 selections bind all five targets (candidates collected before
    # tiers); schema 2 promotes the core tier only. The trailing tier is added
    # to the published release by assets-mode recovery.
    require(selection_tier(selected) in (None, "core"),
            "promotion accepts only a core selection; add the trailing tier with assets-mode recovery")


def release_for_tag(tag):
    pages = api(repo_endpoint("releases?per_page=100"), pages=True)
    matching = [release for page in pages for release in page if release["tag_name"] == tag]
    require(len(matching) <= 1, "duplicate releases for tag")
    return matching[0] if matching else None


def newly_created_release_for_tag(tag):
    # Draft creation can precede visibility in the releases-list API. Retry
    # only a successful empty read, never creation or a failed API request.
    for attempt in range(30):
        release = release_for_tag(tag)
        if release is not None:
            return release
        if attempt < 29:
            time.sleep(1)
    return None


def release_inventory(release):
    pages = api(repo_endpoint(f"releases/{integer(release['id'], 'release ID')}/assets?per_page=100"), pages=True)
    items = [item for page in pages for item in page]
    names, ids = set(), set()
    for item in items:
        name = item["name"]
        require(isinstance(name, str) and re.fullmatch("[A-Za-z0-9_.-]+", name)
                and name not in (".", ".."), "unsafe release asset name")
        require(name not in names and item["id"] not in ids, "duplicate release asset name/ID")
        integer(item["id"], "release asset ID")
        require(item["state"] == "uploaded", "incomplete existing release asset")
        names.add(name)
        ids.add(item["id"])
    # Download statistics (and uploader profile data) can change while we read
    # the release. Compare asset identity/integrity and editable metadata only.
    return {
        item["name"]: {
            "id": item["id"], "name": item["name"], "state": item["state"],
            "size": item["size"], "digest": item.get("digest"),
            "label": item.get("label"), "content_type": item.get("content_type"),
        }
        for item in items
    }


def download_release_inventory(inventory, folder):
    folder.mkdir()
    for name, item in inventory.items():
        api_download(repo_endpoint(f"releases/assets/{item['id']}"), folder / name, kind="release-asset")
        actual_hash = assets.sha256(folder / name)
        # Older GitHub release assets legitimately have no API digest.
        if item.get("digest") is not None:
            require(item["digest"] == "sha256:" + actual_hash, "release asset API/download digest mismatch")
        require(item["size"] == (folder / name).stat().st_size, "release asset download size mismatch")


def protocol_source(sha):
    files = git("ls-tree", "-r", "--name-only", sha).splitlines()
    return "scripts/release-candidate.py" in files


def verify_selected_run(selected, sha):
    """The selection's candidate run completed successfully from this source,
    and the manifest artifact it names is that run's, with that digest. The
    artifact may have expired; its record must still match."""
    info = verify_run(selected["run_id"], selected["run_attempt"], sha)
    artifact_id = integer(selected["manifest_artifact"]["id"], "manifest artifact ID")
    value = api(repo_endpoint(f"actions/artifacts/{artifact_id}"))
    require(value["id"] == artifact_id and value["name"] == f"meta-candidate-manifest-{selected['run_attempt']}",
            "selected manifest artifact identity/name mismatch")
    require(value["digest"] == "sha256:" + digest(selected["manifest_artifact"]["digest"]),
            "selected manifest artifact digest mismatch")
    owner = value["workflow_run"]
    require(owner["id"] == info["id"] and owner["head_sha"] == info["head_sha"]
            and owner["repository_id"] == info["repository"]["id"],
            "selected manifest artifact belongs to another run")
    return info


def verify_published_proofs(folder, selected, sha, version, tag):
    _, manifest_name, manifest_bundle = tier_proof_names(selection_tier(selected))
    verify_selected_run(selected, sha)
    manifest_path = folder / manifest_name
    manifest = read_json(manifest_path)
    match_manifest_selection(manifest, selected, manifest_path, sha, version, tag)
    verify_provenance(manifest_path, folder / manifest_bundle, sha,
                      selected["run_id"], selected["run_attempt"])
    verify_run(manifest["ci"]["run_id"], manifest["ci"]["run_attempt"], sha, ci=True)
    check_metadata(manifest, folder)
    for item in manifest["artifacts"]:
        bundle = folder / f"target-{item['target']}-provenance.jsonl"
        for record in item["archives"]:
            path = folder / record["name"]
            require(describe_archive(path, version, item["target"]) == record, "published outer/inner archive mismatch")
            verify_provenance(path, bundle, sha, selected["run_id"], selected["run_attempt"])
    return manifest


def verify_flat_subset(folder, destination, tag, tier=None):
    destination.mkdir()
    names = [*assets.expected_archive_names(tag[1:], tier), *assets.metadata_names(tier)]
    for name in names:
        require((folder / name).is_file(), f"missing published original asset: {name}")
        shutil.copyfile(folder / name, destination / name)
    verify_prepared(destination, tag, tier)


def verify_present_trailing(folder, inventory, sha, version, tag, work):
    """Verify every trailing-tier file a public release carries, however
    many of them there are; only absent files are tolerated.

    The receipt must be a schema 2 trailing selection whose candidate run
    completed successfully from this source and owns the manifest artifact
    it names. The manifest comes from the release when present, otherwise
    from that artifact, the way stage_candidate reads it: it must match the
    receipt and the release's source, version and tag, carry a valid
    attestation, and name a green CI run. Every present attestation bundle
    must be the one the candidate retained (the manifest's verifies the
    manifest; a target's verifies its archives, or equals the target
    artifact's when none of them is present yet). Every present archive must
    match its manifest record and its target's attestation, and every
    present index or checksums file its manifest hash. Returns the number of
    trailing files still absent.
    """
    files = tier_file_names(version, "trailing")
    present = files & set(inventory)
    receipt, manifest_name, manifest_bundle = tier_proof_names("trailing")
    if not present:
        return len(files)
    require(receipt in present, "trailing assets present without the trailing receipt")
    selected = validate_selection(read_json(folder / receipt))
    require(selection_tier(selected) == "trailing", "trailing receipt is not a trailing selection")
    info = verify_selected_run(selected, sha)
    if manifest_name in present:
        require(manifest_bundle in present, "trailing manifest present without its attestation bundle")
        source = folder
    else:
        dependents = present & (set(assets.expected_archive_names(version, "trailing"))
                                | set(assets.metadata_names("trailing")))
        require(not dependents, "trailing assets present without the trailing manifest")
        identity = {**selected["manifest_artifact"], "name": f"meta-candidate-manifest-{selected['run_attempt']}"}
        packed = download_artifact(identity, info, work)
        source = unpack_artifact(packed, work / "trailing-manifest", sorted(manifest_members("trailing")))
        if manifest_bundle in present:
            require(assets.sha256(folder / manifest_bundle) == assets.sha256(source / PROVENANCE),
                    "published trailing manifest bundle is not the candidate's")
    manifest_path = source / manifest_name
    manifest = read_json(manifest_path)
    match_manifest_selection(manifest, selected, manifest_path, sha, version, tag)
    bundle_path = folder / manifest_bundle if source is folder else source / PROVENANCE
    verify_provenance(manifest_path, bundle_path, sha, selected["run_id"], selected["run_attempt"])
    verify_run(manifest["ci"]["run_id"], manifest["ci"]["run_attempt"], sha, ci=True)
    for name, expected_hash in manifest["metadata"].items():
        if name in present:
            require(assets.sha256(folder / name) == expected_hash, f"published trailing {name} hash mismatch")
    for item in manifest["artifacts"]:
        bundle = f"target-{item['target']}-provenance.jsonl"
        archives = [record for record in item["archives"] if record["name"] in present]
        if bundle in present and not archives:
            packed = download_artifact(item, info, work)
            names = [record["name"] for record in item["archives"]]
            unpacked = unpack_artifact(packed, work / f"trailing-{item['target']}", [*names, PROVENANCE])
            require(assets.sha256(folder / bundle) == assets.sha256(unpacked / PROVENANCE),
                    f"published {bundle} is not the candidate's")
        for record in archives:
            require(bundle in present, f"trailing archive {record['name']} present without its attestation bundle")
            path = folder / record["name"]
            require(describe_archive(path, version, item["target"]) == record,
                    f"published trailing archive {record['name']} does not match its manifest record")
            verify_provenance(path, folder / bundle, sha, selected["run_id"], selected["run_attempt"])
    return len(files - present)


def verify_published_trailing(folder, inventory, work, sha, version, tag):
    """The trailing tier may still be absent or partly added: registry retry
    and verification depend on the core tier only, but every trailing file
    that is present verifies, and a complete tier verifies as a set."""
    pending = verify_present_trailing(folder, inventory, sha, version, tag, work)
    if pending == len(tier_file_names(version, "trailing")):
        print("trailing tier not added yet")
    elif pending:
        print(f"trailing tier incomplete: every present file verified, {pending} pending assets-mode recovery")
    else:
        verify_flat_subset(folder, work / "flat-trailing", tag, "trailing")
        print("verified the trailing tier")


def verify_published(_args=None):
    sha, version, tag = source_context()
    verify_remote_tag(tag, sha)
    release = release_for_tag(tag)
    require(release is not None and release["draft"] is False, "complete public release required")
    with workspace() as work:
        folder = work / "published"
        inventory = release_inventory(release)
        download_release_inventory(inventory, folder)
        candidate_era = protocol_source(sha)
        tier = None
        if candidate_era:
            require(SELECTION in inventory, "candidate-era release is missing its selection proof")
            selected = validate_selection(read_json(folder / SELECTION))
            tier = selection_tier(selected)
            require(tier in (None, "core"), "a release's receipt must bind its core tier")
        if candidate_era:
            allowed = tier_file_names(version, tier)
            if tier == "core":
                allowed |= tier_file_names(version, "trailing")
        else:
            allowed = set(assets.expected_archive_names(version)) | set(assets.metadata_names())
        unexpected = sorted(set(inventory) - allowed)
        require(not unexpected, f"unexpected release assets: {', '.join(unexpected)}")
        verify_flat_subset(folder, work / "flat", tag, tier)
        if candidate_era:
            verify_published_proofs(folder, selected, sha, version, tag)
            if tier == "core":
                verify_published_trailing(folder, inventory, work, sha, version, tag)
        verify_remote_tag(tag, sha)
        require(release_inventory(release) == inventory, "published release inventory changed during verification")
    print(f"verified complete public release {tag} at {sha}; no writes")


def flat_local_files(folder):
    require(folder.is_dir() and not folder.is_symlink(), "missing/unsafe staging directory")
    result = {}
    for path in folder.iterdir():
        require(path.is_file() and not path.is_symlink(), "staging must contain only regular flat files")
        result[path.name] = path
    return result


def validate_staging(args, selected, sha, version, tag, work):
    require_promotable(selected)
    tier = selection_tier(selected)
    expected_assets = set(assets.expected_archive_names(version, tier)) | set(assets.metadata_names(tier))
    expected_proofs = {MANIFEST, SELECTION, MANIFEST_PROVENANCE} | {
        f"target-{target}-provenance.jsonl" for target in tier_targets(tier)}
    local_assets, local_proofs = flat_local_files(args.assets_dir), flat_local_files(args.proof_dir)
    require(set(local_assets) == expected_assets and set(local_proofs) == expected_proofs,
            "staged asset/proof inventory mismatch")
    require(read_json(local_proofs[SELECTION]) == selected, "staged acceptance receipt mismatch")
    verify_prepared(args.assets_dir, tag, tier)
    combined = work / "combined"
    combined.mkdir()
    for name, path in {**local_assets, **local_proofs}.items():
        shutil.copyfile(path, combined / name)
    manifest = verify_published_proofs(combined, selected, sha, version, tag)
    verify_run(selected["run_id"], selected["run_attempt"], sha)
    verify_run(manifest["ci"]["run_id"], manifest["ci"]["run_attempt"], sha, ci=True)
    # Pin every upload to the verified copy, not a mutable caller-owned path.
    return {name: combined / name for name in {*local_assets, *local_proofs}}


def check_existing(inventory, downloaded, expected, selected, *, version=None, draft=None,
                   sha=None, tag=None, work=None):
    if inventory:
        require(SELECTION in inventory, "unbound partial release: acceptance receipt missing; operator cleanup required")
        require(read_json(downloaded / SELECTION) == selected, "existing release receipt mismatch")
    # A published schema 2 core release may already carry the trailing tier,
    # added by recovery after promotion; every such file present must verify
    # against the trailing receipt. Promotion makes a draft public, and no
    # trailing file may ride along: the trailing tier is added only to the
    # published release.
    trailing = tier_file_names(version, "trailing") if selection_tier(selected) == "core" else set()
    carried = (trailing & set(inventory)) - set(expected)
    if carried:
        require(draft is False, "trailing files on an unpublished release; operator cleanup required")
        verify_present_trailing(downloaded, inventory, sha, version, tag, work)
    for name in inventory:
        if name in carried:
            continue
        require(name in expected, f"unrecognized existing release asset: {name}")
        require(assets.sha256(downloaded / name) == assets.sha256(expected[name]),
                f"existing release asset conflict: {name}")


def snapshot_hashes(inventory, folder):
    return {name: (item["id"], assets.sha256(folder / name)) for name, item in inventory.items()}


def readback(release, expected, original, work, sha, tag):
    verify_remote_tag(tag, sha)
    inventory = release_inventory(release)
    require(set(inventory) == set(expected), "release readback inventory mismatch")
    folder = work / ("readback-" + uuid.uuid4().hex)
    download_release_inventory(inventory, folder)
    for name, path in expected.items():
        require(assets.sha256(folder / name) == assets.sha256(path), f"release readback byte mismatch: {name}")
    for name, (asset_id, old_hash) in original.items():
        require(inventory[name]["id"] == asset_id and assets.sha256(folder / name) == old_hash,
                "preexisting release asset changed")
    verify_remote_tag(tag, sha)


def upload(tag, path):
    run(["gh", "release", "upload", tag, str(path), "--repo", repository()])


def publish(args):
    require(os.environ.get("RELEASE_MODE") == "promote" and not env_bool("REGISTRY_DRY_RUN"),
            "publish requires real promote mode")
    selected = selection_env()
    sha, version, tag = source_context()
    with workspace() as work:
        expected = validate_staging(args, selected, sha, version, tag, work)
        verify_remote_tag(tag, sha)
        release = release_for_tag(tag)
        inventory, original = {}, {}
        if release is not None:
            inventory = release_inventory(release)
            require(SELECTION in inventory, "unbound partial release: acceptance receipt missing; operator cleanup required")
            downloaded = work / "existing"
            download_release_inventory(inventory, downloaded)
            check_existing(inventory, downloaded, expected, selected, version=version,
                           draft=release["draft"], sha=sha, tag=tag, work=work)
            original = snapshot_hashes(inventory, downloaded)
            # Trailing files already added stay exactly as they are.
            expected.update({name: downloaded / name for name in inventory if name not in expected})
        verify_remote_tag(tag, sha)
        if release is not None:
            require(release_inventory(release) == inventory, "release changed before write")
        else:
            run(["gh", "release", "create", tag, "--repo", repository(), "--verify-tag",
                 "--draft", "--title", tag, "--notes", RELEASE_NOTES])
            release = newly_created_release_for_tag(tag)
            require(release is not None and release["draft"] is True, "new draft release missing")
            require(not release_inventory(release), "new draft unexpectedly contains assets")
        # Bind first. If this upload fails, a later attempt refuses the unbound draft.
        order = [SELECTION] + sorted(set(expected) - {SELECTION})
        for name in order:
            if name not in inventory:
                verify_remote_tag(tag, sha)
                upload(tag, expected[name])
        readback(release, expected, original, work, sha, tag)
        if release["draft"]:
            run(["gh", "release", "edit", tag, "--repo", repository(), "--draft=false"])
        final = release_for_tag(tag)
        require(final is not None and final["id"] == release["id"] and final["draft"] is False,
                "release was not published as expected")
        readback(final, expected, original, work, sha, tag)
    print(f"published verified original candidate {tag}; existing bytes preserved")


def recover_original(selected, release, inventory, existing, work, sha, version, tag):
    require(not protocol_source(sha), "candidate-era recovery requires accepted-candidate selection")
    require(selected["source_sha"] == sha and selected["version"] == version and selected["tag"] == tag,
            "original-assets source/version/tag mismatch")
    for name, expected_hash in selected["metadata"].items():
        require(name in inventory and assets.sha256(existing / name) == expected_hash,
                f"original existing {name} unavailable/mismatched; cannot reconstruct historical metadata")
    index = read_json(existing / "index.json")
    require(index == {"version": version, "tag": tag, "artifacts": assets.expected_archive_names(version),
                      "checksums": "checksums.sha256"}, "legacy index mismatch")
    checksums = {}
    for line in (existing / "checksums.sha256").read_text().splitlines():
        parts = line.split("  ")
        require(len(parts) == 2 and parts[1] not in checksums, "invalid legacy checksum entry")
        checksums[parts[1]] = digest(parts[0])
    require(set(checksums) == set(assets.expected_archive_names(version)), "legacy checksum inventory mismatch")
    for name in set(inventory) & set(checksums):
        require(assets.sha256(existing / name) == checksums[name], "existing legacy archive checksum conflict")
    info = verify_run(selected["run_id"], selected["run_attempt"], sha, legacy_tag=tag)
    additions = {}
    for item in selected["artifacts"]:
        packed = download_artifact(item, info, work)
        # Historical target ZIPs can contain unselected sibling archives; they are
        # verified structurally but never implicitly added to the release.
        target_names = [n for n in assets.expected_archive_names(version) if f"-{item['target']}." in n]
        with zipfile.ZipFile(packed) as archive:
            actual = {i.filename for i in zip_members(archive)}
        selected_names = {record["name"] for record in item["archives"]}
        require(selected_names <= actual <= set(target_names) | {PROVENANCE},
                "original artifact missing selected archives or contains unexpected files")
        folder = unpack_artifact(packed, work / item["target"], actual)
        # Historical uploads predate retained bundles. Only this explicit legacy
        # path may retrieve existing signatures from GitHub; a bad retained bundle
        # never falls back to remote lookup.
        bundle = folder / PROVENANCE if PROVENANCE in actual else None
        for record in item["archives"]:
            path = folder / record["name"]
            require(describe_archive(path, version, item["target"]) == record
                    and record["sha256"] == checksums[path.name], "selected historical outer/inner hash mismatch")
            verify_provenance(path, bundle, sha, selected["run_id"], selected["run_attempt"],
                              ref=f"refs/tags/{tag}", event="push", allow_remote=True)
            additions[path.name] = path
    return additions


def trailing_additions(selected, release, inventory, existing, work, sha, version, tag):
    """Stage the trailing tier for the published release it extends.

    It binds to the published core receipt: the release must be public and
    carry a schema 2 core receipt whose attested manifest names the same
    source, version and tag. Every file already on the release must be a
    core file or a byte-identical trailing file; anything else refuses
    before any write.
    """
    require(release["draft"] is False, "the trailing tier extends a published release only")
    require(SELECTION in inventory, "trailing tier requires the published core receipt")
    core = validate_selection(read_json(existing / SELECTION))
    require(selection_tier(core) == "core", "trailing tier requires a schema 2 core receipt")
    verify_published_proofs(existing, core, sha, version, tag)
    stage_work = work / "stage-work"
    stage_work.mkdir()
    output, proofs = work / "staged", work / "proofs"
    stage_candidate(selected, output, proofs, stage_work)
    full = {**flat_local_files(output), **flat_local_files(proofs)}
    require(set(full) == tier_file_names(version, "trailing"), "staged trailing inventory mismatch")
    core_files = tier_file_names(version, "core")
    receipt = tier_proof_names("trailing")[0]
    bound = ""
    if receipt in inventory:
        try:
            other = read_json(existing / receipt)
            bound = f" (the release's trailing receipt names run {other.get('run_id')}/{other.get('run_attempt')})"
        except (ProtocolError, ValueError, AttributeError):
            bound = " (the release's trailing receipt is unreadable)"
    for name in inventory:
        if name in full:
            require(assets.sha256(existing / name) == assets.sha256(full[name]),
                    f"existing release asset conflict: {name}{bound}; operator cleanup required")
        else:
            require(name in core_files, f"unrecognized existing release asset: {name}; operator cleanup required")
    return full


def trailing_upload_order(names):
    """Receipt first (binds the tier), then each file only after everything
    that verifies it: the manifest's attestation bundle, the manifest, the
    targets' attestation bundles, the archives, and the tier's index and
    checksums last, so a listed archive is always already present. An
    interrupted addition therefore never leaves a present file that cannot
    be verified."""
    receipt, manifest_name, manifest_bundle = tier_proof_names("trailing")
    metadata = set(assets.metadata_names("trailing"))
    order = {receipt: 0, manifest_bundle: 1, manifest_name: 2}
    rank = lambda name: (order.get(name, 5 if name in metadata else 4 if assets.is_archive(Path(name)) else 3), name)
    return sorted(names, key=rank)


def recover(_args=None):
    require(os.environ.get("RELEASE_MODE") == "assets"
            and not env_bool("REGISTRY_DRY_RUN") and not env_bool("PUBLISH_RELEASE_PACKAGES"),
            "recover requires assets mode and both publication booleans false")
    selected = selection_env(original=True)
    sha, version, tag = source_context()
    verify_remote_tag(tag, sha)
    release = release_for_tag(tag)
    require(release is not None, "asset recovery cannot create a release")
    with workspace() as work:
        inventory = release_inventory(release)
        existing = work / "existing"
        download_release_inventory(inventory, existing)
        original = snapshot_hashes(inventory, existing)
        order = sorted
        if selected["kind"] == "accepted-candidate" and selection_tier(selected) == "trailing":
            additions = trailing_additions(selected, release, inventory, existing, work, sha, version, tag)
            order = trailing_upload_order
        elif selected["kind"] == "accepted-candidate":
            stage_work = work / "stage-work"
            stage_work.mkdir()
            output, proofs = work / "staged", work / "proofs"
            stage_candidate(selected, output, proofs, stage_work)
            full = {**flat_local_files(output), **flat_local_files(proofs)}
            check_existing(inventory, existing, full, selected, version=version,
                           draft=release["draft"], sha=sha, tag=tag, work=work)
            require(SELECTION in inventory, "candidate recovery requires the existing selection receipt")
            additions = full
        else:
            additions = recover_original(selected, release, inventory, existing, work, sha, version, tag)
        expected = {name: existing / name for name in inventory}
        for name, path in additions.items():
            if name in expected:
                require(assets.sha256(expected[name]) == assets.sha256(path), f"existing asset conflict: {name}")
            else:
                expected[name] = path
        verify_remote_tag(tag, sha)
        require(release_inventory(release) == inventory, "release changed before recovery write")
        for name in order(set(additions) - set(inventory)):
            verify_remote_tag(tag, sha)
            upload(tag, additions[name])
        readback(release, expected, original, work, sha, tag)
    print(f"recovered explicitly selected original assets for {tag}; existing IDs/bytes/metadata preserved")


def dispatch(args):
    mode = args.mode
    source, tag = os.environ.get("SOURCE_SHA", ""), os.environ.get("RELEASE_TAG", "")
    path = os.environ.get("ARTIFACT_SELECTION_FILE", "")
    publish, dry = env_bool("PUBLISH_RELEASE_PACKAGES"), env_bool("REGISTRY_DRY_RUN")
    tier = os.environ.get("RELEASE_TIER", "")
    selected = ""
    if mode == "candidate":
        digest(source, "SOURCE_SHA", 40)
        require(not tag and not path and not publish and not dry, "contradictory candidate inputs")
        require(tier in assets.TIERS, "TIER must be core or trailing")
    else:
        require(not source, "SOURCE_SHA is only valid for candidate")
        require(not tier, "TIER is only valid for candidate")
        valid_tag(tag)
        if mode in ("promote", "assets"):
            require(path, "ARTIFACT_SELECTION_FILE is required")
            chosen = validate_selection(read_json(Path(path)), original=mode == "assets")
            if mode == "promote":
                require_promotable(chosen)
            selected = json.dumps(chosen, separators=(",", ":"), sort_keys=True)
        else:
            require(not path and publish, "registry retry requires publication enabled and no selection")
        if mode == "assets":
            require(not publish and not dry, "assets recovery requires both publication booleans false")
    inputs = {"release_mode": mode, "source_sha": source, "release_tag": tag,
              "artifact_selection": selected, "publish_release_packages": str(publish).lower(),
              "registry_dry_run": str(dry).lower(), "candidate_tier": tier}
    env = os.environ.copy()
    env.pop("GH_TOKEN", None)
    env.pop("GITHUB_API_TOKEN", None)
    print("Release protocol: tag pushes validate only; publication requires explicit accepted-byte promotion.",
          file=sys.stderr)
    run(["gh", "workflow", "run", "release.yml", "--repo", REPOSITORY, "--ref", "main", "--json"],
        data=json_bytes(inputs), env=env)
    print(f"dispatched {mode} using owner gh authentication on main")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("resolve")
    collect_parser = commands.add_parser("collect")
    collect_parser.add_argument("--output-dir", type=Path, required=True)
    collect_parser.add_argument("--ci-run-id", type=int, required=True)
    collect_parser.add_argument("--ci-run-attempt", type=int, required=True)
    collect_parser.add_argument("--tier", choices=assets.TIERS,
                                help="collect one tier (schema 2); omitted only for schema 1 fixtures")
    select_parser = commands.add_parser("selection")
    select_parser.add_argument("--manifest-file", type=Path, required=True)
    select_parser.add_argument("--artifact-id", type=int, required=True)
    select_parser.add_argument("--artifact-digest", required=True)
    stage_parser = commands.add_parser("stage")
    stage_parser.add_argument("--output-dir", type=Path, required=True)
    stage_parser.add_argument("--proof-dir", type=Path, required=True)
    publish_parser = commands.add_parser("publish")
    publish_parser.add_argument("--assets-dir", type=Path, required=True)
    publish_parser.add_argument("--proof-dir", type=Path, required=True)
    commands.add_parser("verify-published")
    commands.add_parser("recover")
    dispatch_parser = commands.add_parser("dispatch")
    dispatch_parser.add_argument("--mode", choices=("candidate", "promote", "existing", "assets"), required=True)
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    handlers = {"resolve": lambda _: resolve(), "collect": collect, "selection": selection,
                "stage": stage, "publish": publish, "verify-published": verify_published,
                "recover": recover, "dispatch": dispatch}
    try:
        handlers[args.command](args)
    except (ProtocolError, assets.ReleaseAssetError, OSError, ValueError, KeyError, TypeError,
            tarfile.TarError, zipfile.BadZipFile) as error:
        print(f"release protocol error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
