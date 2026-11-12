#!/usr/bin/env python3
"""Compose Connect candidates and publish their exact accepted asset bytes.

Candidate construction still refuses unbound inputs before product execution.
Acceptance has no write credentials. Publication only adds the selected bytes
of an accepted installed preview to an existing public release; it never creates
a release/tag, rebuilds a product, or rewrites producer records.

Release mode scans the exact release payload for private text before attestation
or upload: privacy-scan, and assemble-cli for its own product, run the pinned
engine through every nested container with the private rules and exemptions
given on the command line, and refuse without them.
"""
from pathlib import Path, PurePosixPath
import argparse
import email.parser
import gzip
import hashlib
import io
import json
import os
import platform
import re
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.parse
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {"linux-x86_64": ("Linux", "x86_64"), "macos-arm64": ("Darwin", "arm64")}
SCOPE = "installed_gateway_schema4_resource_startup_and_shutdown_preview"
MAX_FILE = 2 * 1024**3 - 1
HEX = re.compile(r"[0-9a-f]{64}\Z")
SHA = re.compile(r"[0-9a-f]{40}\Z")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\Z")
# Meta 0.9.6 requires Core 0.9.7, and Meta 0.9.7 requires Core 0.9.8.
# Select supported pairs explicitly;
# matching version numbers alone do not establish native compatibility.
NATIVE_RELEASE_PAIRS = {("0.9.4", "0.9.4"), ("0.9.5", "0.9.5"), ("0.9.7", "0.9.6"), ("0.9.8", "0.9.7")}
FORGE_READERS = {"installer/launchkit/runtime_selection.py",
                 "installer/launchkit/runtime_candidates.py",
                 "installer/launchkit/sdk_wheel_integrity.py",
                 "installer/launchkit/templates/install_candidate.py"}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def checked_target(value):
    require(isinstance(value, str) and value in TARGETS, "explicit supported target required")
    return value


def selected_target():
    return checked_target(os.environ.get("CONNECT_TARGET"))


def input_path(target):
    checked_target(target)
    name = "connect-release-inputs.json" if target == "linux-x86_64" else "connect-release-inputs-macos-arm64.json"
    return ROOT / "release" / name


def publication_files(target):
    checked_target(target)
    return (f"connect-{target}-candidate.tar.gz", f"connect-{target}-candidate.json",
            f"connect-{target}-provenance.jsonl")


def require_native_host(target):
    require((platform.system(), platform.machine()) == TARGETS[checked_target(target)],
            "candidate target differs from native build host")


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def read_json(path):
    return json.loads(path.read_bytes(), object_pairs_hook=unique)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def binding(path):
    return {"path": str(path), "size": path.stat().st_size, "sha256": digest(path)}


def write_new(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True, ensure_ascii=True, allow_nan=False)
        stream.write("\n")
    path.chmod(0o644)


def relative(value):
    require(isinstance(value, str) and value and "\\" not in value, "invalid relative locator")
    path = PurePosixPath(value)
    require(not path.is_absolute() and str(path) == value and ".." not in path.parts,
            "noncanonical relative locator")
    return value


def unbound(value):
    if value is None:
        return True
    if isinstance(value, dict):
        return any(unbound(item) for item in value.values())
    if isinstance(value, list):
        return any(unbound(item) for item in value)
    return False


def public_url(value):
    parsed = urllib.parse.urlsplit(value)
    require(parsed.scheme == "https" and not parsed.username and not parsed.password
            and not parsed.query and not parsed.fragment, "public immutable HTTPS input required")
    github = (parsed.hostname == "github.com" and re.fullmatch(
        r"/(lukacf/(meerkat|meerkat-meta|meerkat-connect)|astral-sh/python-build-standalone)"
        r"/releases/download/[^/]+/[^/]+", parsed.path))
    pypi = parsed.hostname == "files.pythonhosted.org" and parsed.path.startswith("/packages/")
    node = parsed.hostname == "nodejs.org" and re.fullmatch(r"/dist/v[0-9.]+/[^/]+", parsed.path)
    # Existing resource-owner pins, exposed at their official versioned URLs.
    # Keep these bounded to the selected artifacts; byte pins are checked below.
    npm = (parsed.netloc == "registry.npmjs.org" and parsed.path == "/npm/-/npm-12.2.0.tgz")
    pip = (parsed.netloc == "bootstrap.pypa.io" and parsed.path == "/pip/zipapp/pip-26.2.1.pyz")
    require(github or pypi or node or npm or pip, "input URL is outside the selected public release hosts")


def source_state():
    def git(*args):
        return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()
    require(git("status", "--porcelain", "--untracked-files=normal") == "", "source checkout is dirty")
    revision = git("rev-parse", "HEAD")
    expected = os.environ.get("CONNECT_SOURCE_SHA", "")
    require(SHA.fullmatch(expected) and revision == expected == os.environ.get("GITHUB_SHA")
            == os.environ.get("GITHUB_WORKFLOW_SHA"), "dispatch, workflow and selected source differ")
    return revision, git("rev-parse", "HEAD^{tree}")


def checked_native_releases(value):
    require(isinstance(value, dict) and set(value) == {"core", "meta"},
            "exact Core/Meta selection required")
    for name, repository in (("core", "meerkat"), ("meta", "meerkat-meta")):
        selected = value[name]
        require(isinstance(selected, dict)
                and set(selected) == {"repository", "version", "tag", "source_sha"}
                and selected["repository"] == "lukacf/" + repository
                and isinstance(selected["version"], str) and VERSION.fullmatch(selected["version"])
                and isinstance(selected["source_sha"], str) and SHA.fullmatch(selected["source_sha"]),
                "exact Core/Meta commit selection required")
        tag = selected["tag"]
        # A conservative single-component Git tag name. Empty is an explicit
        # commit-only selection, not an unbound value or a mutable-ref fallback.
        require(isinstance(tag, str) and (tag == "" or (
            re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", tag)
            and ".." not in tag and not tag.endswith((".", ".lock")))),
            "native tag must be empty or a safe single-component Git tag")
    require((value["core"]["version"], value["meta"]["version"]) in NATIVE_RELEASE_PAIRS,
            "supported Core/Meta release pair required")
    return value


def checked_sdk_native_dependency(value, sdk):
    selected = value["native_release"]["meta"]["version"]
    require(value["sdks"]["meta"]["version"] == selected, "Meta application SDK must match native release")
    require("meerkat-meta==" + selected in sdk.get("optional-dependencies", {}).get("native-host", []),
            "selected Connect SDK must declare the matching native-host dependency")


def checked_forge_reader_source(value, *, allow_legacy=False):
    """Select reader code independently of the native and Meta SDK releases."""
    pins = value.get("forge_reader_source_pins")
    require(isinstance(pins, dict) and set(pins) == FORGE_READERS
            and all(isinstance(pin, str) and HEX.fullmatch(pin) for pin in pins.values()),
            "exact Forge reader source inventory required")
    if allow_legacy and "forge_reader_source" not in value:
        # Older attested inputs selected their reader from the exact Meta
        # release. Preserve that selection, never infer a newer Forge reader.
        selected = checked_native_releases(value["native_release"])["meta"]
        return {key: selected[key] for key in ("repository", "source_sha", "tag")}
    selected = value.get("forge_reader_source")
    require(isinstance(selected, dict) and set(selected) == {"repository", "source_sha", "tag"}
            and selected["repository"] == "lukacf/meerkat-meta"
            and isinstance(selected["source_sha"], str) and SHA.fullmatch(selected["source_sha"]),
            "explicit exact Forge reader source required")
    tag = selected["tag"]
    require(isinstance(tag, str) and (tag == "" or (
        re.fullmatch(r"forge-v[A-Za-z0-9][A-Za-z0-9._-]{0,120}", tag)
        and ".." not in tag and not tag.endswith((".", ".lock")))),
        "Forge reader tag must be empty or a safe forge-v tag")
    return selected


def checked_forge_reader_files(reader, value):
    for name, expected in value["forge_reader_source_pins"].items():
        require(checked_file(reader / name)["sha256"] == expected,
                "selected Forge reader source differs")


def checked_native_family(families, value):
    require({package["name"] for package in families} == set(value["native_family_packages"]),
            "native family is not the complete selected graph")
    for package in families:
        domain = "meta" if package["name"] == "meerkat-meta" else "core"
        release = value["native_release"][domain]
        require(package["version"] == release["version"], "native family version differs from selection")
        source = package["source"]
        require(isinstance(source, str), "path-sourced native family package is not a selected registry/Git source")
        if source == "registry+https://github.com/rust-lang/crates.io-index":
            continue  # Cargo verifies the exact selected lock's registry checksum.
        repository = "git+https://github.com/" + release["repository"]
        # Cargo retains either exact repository spelling in its source ID.
        require(source.startswith((repository + "?", repository + ".git?"))
                and source.endswith("#" + release["source_sha"]),
                "native Git source is not the selected commit")


def preflight(*, native=False):
    require(os.environ.get("GITHUB_ACTIONS") == "true"
            and os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
            and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted",
            "manual clean GitHub-hosted execution required")
    target = selected_target()
    if native:
        require_native_host(target)
    inputs = input_path(target)
    require(os.environ.get("GITHUB_REPOSITORY") == PUBLICATION_REPO, "wrong candidate repository")
    revision, tree = source_state()
    expected = os.environ.get("CONNECT_INPUTS_SHA256", "")
    require(HEX.fullmatch(expected) and digest(inputs) == expected, "release input selection differs")
    value = read_json(inputs)
    require(not unbound(value), "unbound release inputs: no build or old-default fallback")
    require(value["schema_version"] == 1 and value["kind"] == "connect-candidate-inputs"
            and value["target"] == target and value["scope"] == SCOPE, "wrong candidate scope")
    require(VERSION.fullmatch(value["connect_version"]), "explicit Connect version required")
    native = tomllib.loads((ROOT / "native-candidate/Cargo.toml").read_text())
    sdk = tomllib.loads((ROOT / "sdk/python/pyproject.toml").read_text())["project"]
    require(native["workspace"]["package"]["version"] == value["connect_version"],
            "Connect version differs from source metadata")
    require(sdk["name"] == value["sdks"]["toolkit"]["distribution"]
            and sdk["version"] == value["sdks"]["toolkit"]["version"], "Connect SDK metadata differs")
    require(value["rust_toolchain"] == "1.97.0", "unreviewed Rust toolchain")
    checked_native_releases(value["native_release"])
    checked_sdk_native_dependency(value, sdk)
    checked_forge_reader_source(value)
    files = value["files"]
    require(isinstance(files, dict) and 0 < len(files) <= 512, "bounded complete public file selection required")
    for name, row in files.items():
        relative(name)
        require(set(row) == {"url", "size", "sha256"} and type(row["size"]) is int
                and 0 < row["size"] <= MAX_FILE and HEX.fullmatch(row["sha256"]), "invalid public file pin")
        public_url(row["url"])
    references = list(value["resources"]["upstream"])
    references += [value["resources"][key] for key in ("library", "acquisition_node", "npm", "node_prebuilt_policy")]
    references += [value["dependencies"]["requirements"], *value["dependencies"]["wheels"]]
    references += [value["sdk_build"]["requirements"], *value["sdk_build"]["wheels"]]
    references += [value["sdks"]["meta"]["wheel"]]
    if value["resources"].get("node_build_profile"):
        references.append(value["resources"]["node_build_profile"])
    require(set(references) == set(files), "input inventory is incomplete or contains unselected files")
    require(0 < len(value["dependencies"]["wheels"]) <= 256, "offline wheel closure required")
    require(0 < len(value["sdk_build"]["wheels"]) <= 256, "offline SDK build-tool closure required")
    for path in ("native-candidate/Cargo.toml", "native-candidate/Cargo.lock"):
        require(value["source_pins"][path] == digest(ROOT / path), "selected native source pins differ")
    require(value["expected_package_ids"] and value["expected_features"]
            and value["expected_resolve"] and value["native_family_packages"], "exact actual native graph selection required")
    return value, revision, tree


class Recorder:
    # Ordinary command deadlines are unchanged. Cleanup observes the same owned
    # child in bounded slices, retaining it until reap after escalation.
    TERM_GRACE_SECONDS = 15
    CLEANUP_WAIT_SECONDS = 0.1

    def __init__(self, work):
        self.work = work
        self.records = work / "records"
        self.records.mkdir()

    def _stop_owned(self, process, state):
        """Reap the direct child and signal only its original new-session group.

        A leader exit does not imply its descendants exited. After TERM grace,
        send KILL to that group even if the leader was already reaped. KILL
        delivery is not descendant reap; zombie group visibility is not a join
        gate. No signal handler or process-wide signal disposition is changed.
        """
        def observed(error):
            if state["error"] is None:
                state["error"] = error
            name = type(error).__name__
            counts = state["errors"]
            key = name if name in counts or len(counts) < 16 else "other"
            counts[key] = counts.get(key, 0) + 1

        def send(sig):
            try:
                os.killpg(process.pid, sig)
                return "sent"
            except ProcessLookupError:
                return "absent"
            except OSError as error:
                observed(error)
                return "unconfirmed"

        while True:
            try:
                if state["deadline"] is None:
                    state["deadline"] = time.monotonic() + self.TERM_GRACE_SECONDS
                if state["term"] is None:
                    state["term"] = send(signal.SIGTERM)
                    if state["term"] == "absent":
                        state["group"] = "absent"
                if state["group"] is None:
                    try:
                        os.killpg(process.pid, 0)
                    except ProcessLookupError:
                        state["group"] = "absent"
                    except OSError as error:
                        observed(error)  # Still attempt KILL after grace.
                    if state["group"] is None and time.monotonic() >= state["deadline"]:
                        result = send(signal.SIGKILL)
                        if result != "unconfirmed":
                            state["group"] = result
                # poll() also permits recovery from an unexpected wait() error.
                # Both methods reap through the same Popen owner, never waitpid
                # on an unrelated process or an inferred cached PID.
                if process.poll() is None:
                    try:
                        process.wait(timeout=self.CLEANUP_WAIT_SECONDS)
                    except subprocess.TimeoutExpired:
                        pass
                if process.returncode is not None and state["group"] is not None:
                    return
                # A reaped leader with a live group must still receive its TERM
                # grace without a busy loop; repeated interrupts do not restart it.
                time.sleep(self.CLEANUP_WAIT_SECONDS)
            except BaseException as error:
                observed(error)
                # Cleanup interruption must not abandon the child or replace the
                # original exception. Persistent OS wait errors retain ownership.
                try:
                    time.sleep(self.CLEANUP_WAIT_SECONDS)
                except BaseException as repeated:
                    observed(repeated)

    def _wait_owned(self, process, seconds):
        # A direct guarded call after Popen assignment avoids an uncovered
        # CPython nested-try entry NOP in the caller's ownership region.
        try:
            return process.wait(timeout=seconds), False, None, None
        except subprocess.TimeoutExpired:
            return None, True, None, None
        except BaseException as error:
            return None, False, error, error.__traceback__

    def run(self, name, argv, *, cwd=ROOT, env=None, seconds=600, inherit=True):
        argv = [str(item) for item in argv]
        # inherit=False: env is the complete environment of the child. Nothing of the caller's reaches it.
        selected_env = {**(os.environ if inherit else {}), **(env or {})}
        log = self.records / (name + ".log")
        started = time.time()
        expired = False
        code = None
        failure = None
        traceback = None
        cleanup = {"deadline": None, "term": None, "group": None, "error": None, "errors": {}}
        process = None
        stream = log.open("xb")
        try:
            # Construction and all subsequent owned work share one guard. Once
            # the handle is assigned, an interruption before wait still joins it.
            process = subprocess.Popen(argv, cwd=cwd, env=selected_env,
                                       stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
            code, expired, failure, traceback = self._wait_owned(process, seconds)
            if expired or failure is not None:
                self._stop_owned(process, cleanup)
                code = process.returncode
                if failure is None and cleanup["error"] is not None:
                    failure = cleanup["error"]
                    traceback = failure.__traceback__
        except BaseException as error:
            if failure is None:
                failure, traceback = error, error.__traceback__
            else:
                failure.add_note("cleanup also failed: " + type(error).__name__)
            # A failure inside Popen may have no accessible handle. Only the
            # actual stored handle grants cleanup authority; the exception's
            # location does not imply whether construction already returned.
            if process is not None:
                # Retry with the same state/deadline, including interruptions
                # at cleanup entry. Never restart the TERM grace period.
                while True:
                    # Keep the guarded call on the try line: a standalone
                    # nested try NOP is outside CPython's exception table.
                    try: self._stop_owned(process, cleanup); code = process.returncode; break
                    except BaseException as repeated:
                        failure.add_note("cleanup also failed: " + type(repeated).__name__)
        finally:
            # The log can only be closed/bound after owned-child cleanup. The
            # caller's original interruption remains primary over close failures.
            try:
                stream.close()
            except BaseException as close_error:
                if failure is None:
                    failure, traceback = close_error, close_error.__traceback__
                else:
                    failure.add_note("log close also failed: " + type(close_error).__name__)
        if process is None:
            # No returned handle, no inferred PID/group and no child-reap claim.
            # Preserve the constructor failure, including any log-close note.
            raise failure.with_traceback(traceback)
        try:
            record_env = {key: selected_env[key] for key in (
                "RUSTUP_TOOLCHAIN", "XDG_CACHE_HOME", "RUST_LANE_ID", "CARGO_INCREMENTAL",
                "CARGO_TERM_COLOR", "CARGO_BUILD_JOBS", "MEERKAT_CONNECT_RESOURCES_SHA256", "TMPDIR", "OPENSSL_DIR",
                "HOME", "PATH", "RUST_TEST_THREADS", "RUST_MIN_STACK",
                "PYTHONNOUSERSITE", "PYTHONDONTWRITEBYTECODE")
                if key in selected_env}
            if not inherit:
                # The producer wrote this complete environment itself: every name and value is its own.
                record_env = dict(sorted(selected_env.items()))
            record = {"argv": argv, "cwd": str(cwd), "env": record_env,
                      "exit_code": code if failure is None else None,
                      "timed_out": expired, "elapsed_seconds": time.time() - started, "log": binding(log)}
            if expired or failure is not None:
                record["cleanup"] = {"direct_child_reaped": process.returncode is not None,
                    "group_term": cleanup["term"], "group_after_grace": cleanup["group"],
                    "errors": cleanup["errors"], "descendant_reap_observed": False}
            if failure is not None:
                # Legacy exit_code == 0 consumers must refuse an interrupted
                # command even if its process concurrently exited zero.
                record.update(process_exit_code=code, failure=type(failure).__name__)
            write_new(self.records / (name + ".json"), record)
        except BaseException as record_error:
            if failure is None:
                raise
            failure.add_note("command record also failed: " + type(record_error).__name__)
        if failure is not None:
            raise failure.with_traceback(traceback)
        require(code == 0 and not expired, "command did not succeed: " + name)
        return record, log


def download(path, row):
    path.parent.mkdir(parents=True, exist_ok=True)
    # GitHub release downloads redirect to its release-asset CDN. The original
    # URL and all resulting bytes remain explicitly selected; no API credentials.
    with urllib.request.urlopen(row["url"], timeout=60) as response, path.open("xb") as stream:
        size = 0
        while block := response.read(1024 * 1024):
            size += len(block)
            require(size <= row["size"], "public input exceeds selected size")
            stream.write(block)
    require(size == row["size"] and digest(path) == row["sha256"], "public input bytes differ")
    path.chmod(0o644)


def inspect_prebuilt(path):
    """Parse bounded retained metadata; producer paths remain unchanged data."""
    if path.name.endswith((".tar.gz", ".tar.zst")):
        # Only gzip libraries carry nested Connect provenance; zstd Python
        # runtime archives are verified by the existing resource packager.
        if path.name.endswith(".tar.zst"):
            return
        with tarfile.open(path, "r:gz") as archive:
            for member in archive:
                if member.isfile() and (Path(member.name).name in {
                        "CONTENTS.json", "source-manifest.json", "build-record.json",
                        "resource-build-record.json", "direct_url.json"}
                        or member.name.endswith(".json")
                        or ("provenance" in PurePosixPath(member.name).parts
                            and Path(member.name).suffix in {
                                ".log", ".txt", ".stdout", ".stderr", ".sha256",
                                ".py", ".sh", ".toml", ".yml", ".yaml", ".md"})):
                    require(member.size <= 32 * 1024**2, "embedded provenance exceeds bound")
                    raw = archive.extractfile(member).read()
                    if member.name.endswith(".json"):
                        json.loads(raw, object_pairs_hook=unique)
                    else:
                        raw.decode("utf-8")
    elif path.suffix == ".json":
        require(path.stat().st_size <= 32 * 1024**2, "metadata exceeds bound")
        read_json(path)
    elif path.suffix == ".whl":
        with zipfile.ZipFile(path) as archive:
            for item in archive.infolist():
                if not item.is_dir() and ".dist-info/" in item.filename:
                    require(item.file_size <= 32 * 1024**2, "wheel provenance exceeds bound")
                    raw = archive.read(item)
                    if item.filename.endswith(".json"):
                        json.loads(raw, object_pairs_hook=unique)
                    else:
                        raw.decode("utf-8")


def wheel_expectation(path, selected, source):
    with zipfile.ZipFile(path) as wheel:
        names = [item.filename for item in wheel.infolist() if not item.is_dir()]
        require(len(names) == len(set(names)) and len(names) <= 4096, "invalid wheel inventory")
        for name in names:
            relative(name)
        roots = {name.split("/", 1)[0] for name in names if ".dist-info/" in name}
        require(len(roots) == 1, "one wheel metadata owner required")
        prefix = next(iter(roots)) + "/"
        metadata = email.parser.BytesParser().parsebytes(wheel.read(prefix + "METADATA"))
        require(metadata["Name"] == selected["distribution"] and metadata["Version"] == selected["version"]
                and VERSION.fullmatch(selected["version"]), "wheel metadata differs from selection")
        payload = sorted(name for name in names if not name.startswith(prefix))
        require(set(payload) == set(selected["source_files"]), "complete wheel-to-source map required")
        for name in payload:
            source_name = relative(selected["source_files"][name])
            require(wheel.read(name) == (source / source_name).read_bytes(), "wheel payload differs from selected source")
        def rows(items, strip=""):
            return [[name.removeprefix(strip), hashlib.sha256(wheel.read(name)).hexdigest(), len(wheel.read(name))]
                    for name in sorted(items)]
        return {"distribution": selected["distribution"], "version": selected["version"],
                "sha256": digest(path), "size": path.stat().st_size,
                "source_payload": rows(payload),
                "metadata": rows([name for name in names if name.startswith(prefix) and name != prefix + "RECORD"], prefix)}


def copy_exact(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(0o644)
    require(digest(destination) == digest(source), "retained file differs")


def archive_tree(directory, destination):
    with destination.open("xb") as stream, gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for path in [directory, *sorted(directory.rglob("*"))]:
                if path.is_symlink():
                    require(not Path(os.readlink(path)).is_absolute()
                            and path.resolve(strict=True).is_relative_to(directory),
                            "candidate symlink escapes its physical closure")
                info = archive.gettarinfo(str(path), arcname=path.relative_to(directory.parent).as_posix())
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ""
                if path.is_file() and not path.is_symlink():
                    with path.open("rb") as payload:
                        archive.addfile(info, payload)
                else:
                    archive.addfile(info)


def fetch_native_source(record, work, domain, selected):
    checkout = work / (domain + "-source")
    repository = "https://github.com/" + selected["repository"] + ".git"
    record.run(domain + "-init", ["git", "init", checkout])
    record.run(domain + "-fetch", ["git", "fetch", "--no-tags", "--depth=1",
        repository, selected["source_sha"]], cwd=checkout)
    _, identity = record.run(domain + "-identity",
        ["git", "rev-parse", "--verify", "FETCH_HEAD^{commit}"], cwd=checkout)
    require(identity.read_text().strip() == selected["source_sha"], "fetched native commit differs")
    if selected["tag"]:
        tag = "refs/tags/" + selected["tag"]
        record.run(domain + "-tag-fetch", ["git", "fetch", "--no-tags", "--depth=1",
            repository, tag + ":" + tag], cwd=checkout)
        _, identity = record.run(domain + "-tag-identity",
            ["git", "rev-parse", "--verify", tag + "^{commit}"], cwd=checkout)
        require(identity.read_text().strip() == selected["source_sha"], "selected native tag points elsewhere")
    record.run(domain + "-checkout", ["git", "checkout", "--detach", selected["source_sha"]], cwd=checkout)
    return checkout


def build():
    value, revision, tree = preflight(native=True)
    target = value["target"]
    inputs_path = input_path(target)
    subjects = publication_files(target)[:2]
    work = Path(os.environ["RUNNER_TEMP"]) / "connect-candidate"
    require(not work.exists() and not (ROOT / "dist").exists(), "fresh output roots required")
    work.mkdir()
    (work / "tmp").mkdir()
    os.environ["TMPDIR"] = str(work / "tmp")
    record = Recorder(work)
    inputs = work / "inputs"
    artifacts = work / "artifacts"
    artifacts.mkdir()
    copy_exact(inputs_path, work / "inputs-selection.json")
    for name, row in value["files"].items():
        download(inputs / name, row)
        inspect_prebuilt(inputs / name)
    # Fetch only the selected commits into fresh checkouts. An optional tag
    # adds a binding check; neither a default branch nor a prior cache is used.
    releases = {}
    for domain in ("core", "meta"):
        releases[domain] = fetch_native_source(record, work, domain, value["native_release"][domain])
    reader = fetch_native_source(record, work, "forge-reader", checked_forge_reader_source(value))
    checked_forge_reader_files(reader, value)
    # --no-index alone does not forbid direct URLs or included requirements.
    # The selected owner's existing reader validates the exact single-hash
    # grammar and each original wheel/RECORD before either pip installation.
    for batch_name in ("sdk_build", "dependencies"):
        batch = value[batch_name]
        batch_selection = work / (batch_name + "-selection.json")
        write_new(batch_selection, {"requirements": binding(inputs / batch["requirements"]),
            "wheels": [binding(inputs / name) for name in batch["wheels"]]})
        record.run(batch_name + "-read", [sys.executable, "-I", "-c",
            "import json,sys; from pathlib import Path; "
            "sys.path.insert(0,sys.argv[1]); "
            "from launchkit.runtime_candidates import ArtifactFile; "
            "from launchkit.templates.install_candidate import _read_selected_dependency_wheels; "
            "selected=json.loads(Path(sys.argv[2]).read_text()); "
            "pin=lambda row: ArtifactFile(Path(row['path']),row['size'],row['sha256']); "
            "pending=_read_selected_dependency_wheels(pin(selected['requirements']),"
            "tuple(pin(row) for row in selected['wheels'])); "
            "print(json.dumps({'status':'exact_batch_verified','wheels':len(pending)}))",
            reader / "installer", batch_selection], seconds=300)
    # Build the selected Connect SDK through its existing PEP 517 backend. The
    # scratch copy avoids source-checkout mutations; no prior Connect publication
    # is needed. The frontend/backend dependency wheels are explicit offline inputs.
    sdk_source = work / "sdk-source"
    shutil.copytree(ROOT / "sdk/python", sdk_source, symlinks=True)
    sdk_venv = Path(os.environ["RUNNER_TEMP"]) / "connect-sdk-build-venv"
    require(not sdk_venv.exists(), "fresh SDK build environment required")
    record.run("sdk-build-venv", [sys.executable, "-m", "venv", sdk_venv])
    sdk_python = sdk_venv / "bin/python"
    sdk_tools = work / "sdk-build-wheelhouse"
    sdk_tools.mkdir()
    for name in value["sdk_build"]["wheels"]:
        copy_exact(inputs / name, sdk_tools / Path(name).name)
    record.run("sdk-build-tools", [sdk_python, "-m", "pip", "install", "--no-index", "--no-deps", "--require-hashes",
        "--find-links", sdk_tools, "-r", inputs / value["sdk_build"]["requirements"]])
    record.run("sdk-build-inventory", [sdk_python, "-m", "pip", "list", "--format=json"])
    sdk_output = work / "sdk-built"
    sdk_output.mkdir()
    record.run("sdk-wheel", [sdk_python, "-m", "build", "--wheel", "--no-isolation",
        "--outdir", sdk_output, sdk_source], seconds=600)
    wheels = list(sdk_output.glob("*.whl"))
    require(len(wheels) == 1 and list(sdk_output.iterdir()) == wheels, "one original SDK wheel required")
    sdk_wheels = {"toolkit": wheels[0], "meta": inputs / value["sdks"]["meta"]["wheel"]}
    expectations = {name: wheel_expectation(sdk_wheels[name],
        value["sdks"][name], ROOT if name == "toolkit" else releases["meta"]) for name in ("meta", "toolkit")}
    inspect_prebuilt(sdk_wheels["toolkit"])
    write_new(artifacts / "SDK-BYTE-EXPECTATIONS.json", expectations)
    record.run("rust-version", ["rustc", "+1.97.0", "--version", "--verbose"])
    record.run("cargo-version", ["cargo", "+1.97.0", "--version", "--verbose"])
    record.run("python-version", [sys.executable, "--version"])
    if target == "linux-x86_64":
        record.run("system-packages", ["dpkg-query", "-W"])
    else:
        record.run("system-packages", ["brew", "list", "--versions"])
        record.run("system-sdk", ["xcrun", "--show-sdk-path"])
    record.run("rust-sysroot", ["rustc", "+1.97.0", "--print", "sysroot"])
    record.run("cargo-paths", [ROOT / "scripts/repo-cargo", "--print-env"])
    if target == "macos-arm64":
        record.run("system-prefix", ["brew", "--prefix"])
    record.run("system", ["uname", "-a"])
    resource = value["resources"]
    upstream = work / "upstream"
    starter = work / "starter"
    upstream.mkdir()
    for name in resource["upstream"]:
        copy_exact(inputs / name, upstream / Path(name).name)
    # Project only the actual selected library bytes into the existing starter
    # manifest shape. This is a candidate input selector, not a release receipt.
    library = inputs / resource["library"]
    starter_manifest = work / "starter-library-selection.json"
    write_new(starter_manifest, {"schema_version": 1, "status": "candidate_input_not_published",
        "assets": [{"upload_name": library.name, "bytes": library.stat().st_size, "sha256": digest(library)}]})
    record.run("starter-export", [sys.executable, ROOT / "scripts/export-setup-starter.py",
        "--destination", starter, "--manifest", starter_manifest, "--manifest-sha256", digest(starter_manifest)])
    args = [sys.executable, ROOT / "scripts/package-connect-resources.py",
        "--destination", work / "resources", "--upstream", upstream, "--zstd", shutil.which("zstd"),
        "--target", target, "--starter", starter,
        "--release-manifest-sha256", digest(starter / "release-manifest.json")]
    for key, option in (("library", "library"), ("acquisition_node", "acquisition-node"), ("npm", "npm")):
        row = value["files"][resource[key]]
        locator = option + "-archive" if key in ("acquisition_node", "npm") else option
        pin_option = locator if key == "npm" else option
        args += ["--" + locator, inputs / resource[key], "--" + pin_option + "-sha256", row["sha256"],
                 "--" + pin_option + "-bytes", str(row["size"])]
    for key, option in (("node_prebuilt_policy", "node-prebuilt-policy"), ("node_build_profile", "node-build-profile")):
        if resource.get(key):
            args += ["--" + option, inputs / resource[key], "--" + option + "-sha256",
                     value["files"][resource[key]]["sha256"]]
    record.run("resources", args, seconds=900)
    resource_record = work / "resources/build.json"
    resource_value = read_json(resource_record)
    wrapper = ROOT / "scripts/repo-cargo"
    manifest = ROOT / "native-candidate/Cargo.toml"
    _, metadata_log = record.run("metadata", [wrapper, "+1.97.0", "metadata", "--locked", "--format-version", "1",
        "--manifest-path", manifest, "--features", "console,toolkit-mobkit/native-host"], seconds=900)
    # Cargo may print progress before JSON; only the single actual metadata row counts.
    metadata_rows = [json.loads(line) for line in metadata_log.read_text().splitlines() if line.startswith('{')]
    require(len(metadata_rows) == 1, "one actual Cargo metadata result required")
    metadata = metadata_rows[0]
    connect_packages = [package for package in metadata["packages"] if package["name"] == "meerkat-connect"]
    require(len(connect_packages) == 1 and connect_packages[0]["version"] == value["connect_version"]
            and Path(connect_packages[0]["manifest_path"]) == ROOT / "crates/meerkat-connect/Cargo.toml",
            "actual Connect package does not match selected source/version")
    def normalized(identity):
        # Only the selected workspace path is made portable for comparison;
        # all public registry/Git identities remain exact. Raw metadata is kept.
        return identity.replace("path+file://" + str(ROOT) + "/", "path+workspace:///").replace(
            "path+file://" + str(ROOT) + "#", "path+workspace:///#")
    require(sorted(normalized(package["id"]) for package in metadata["packages"])
            == sorted(value["expected_package_ids"]), "actual package graph differs")
    features = {normalized(node["id"]): sorted(node["features"]) for node in metadata["resolve"]["nodes"]}
    require(features == value["expected_features"], "actual feature graph differs")
    resolve = []
    for node in metadata["resolve"]["nodes"]:
        deps = [{"name": dep["name"], "pkg": normalized(dep["pkg"]),
                 "dep_kinds": sorted([{"kind": kind["kind"] or "normal", "target": kind["target"] or ""}
                                      for kind in dep["dep_kinds"]], key=lambda item: (item["kind"], item["target"]))}
                for dep in node["deps"]]
        resolve.append({"id": normalized(node["id"]), "dependencies": sorted(normalized(item) for item in node["dependencies"]),
                        "deps": sorted(deps, key=lambda item: (item["name"], item["pkg"]))})
    normalized_resolve = {"root": normalized(metadata["resolve"]["root"]) if metadata["resolve"]["root"] else "",
                          "nodes": sorted(resolve, key=lambda item: item["id"])}
    require(normalized_resolve == value["expected_resolve"],
            "actual dependency edges or dependency kinds differ")
    families = []
    for package in metadata["packages"]:
        name = package["name"]
        if name in {"meerkat-connect", "meerkat-toolkit"}:
            require(package["source"] is None and package["id"] in metadata["workspace_members"]
                    and Path(package["manifest_path"]) == ROOT / "crates" / name / "Cargo.toml",
                    "Connect-owned package is not the exact selected workspace member")
        elif name == "meerkat" or name.startswith("meerkat-"):
            families.append(package)
    checked_native_family(families, value)
    # Exact IDs distinguish registry versions from Git commit sources; the
    # registry checksums remain pinned by the unchanged selected lock bytes.
    source_files = []
    tracked = subprocess.check_output(["git", "-C", str(ROOT), "ls-files", "-z"]).decode().split('\0')
    for name in filter(None, tracked):
        path = ROOT / name
        require(not path.is_symlink(), "source census requires explicit symlink support before selection")
        source_files.append({"path": name, "sha256": digest(path), "size": path.stat().st_size,
                             "mode": oct(stat.S_IMODE(path.stat().st_mode))})
    source_manifest = artifacts / "source-manifest.json"
    write_new(source_manifest, {"schema_version": 1, "repository": os.environ["GITHUB_REPOSITORY"],
        "revision": revision, "tree": tree, "files": source_files,
        "native_release": value["native_release"], "metadata": binding(metadata_log),
        "input_selection": binding(work / "inputs-selection.json")})
    bound_env = {"MEERKAT_CONNECT_RESOURCES_SHA256": resource_value["resources_sha256"]}
    cargo, cargo_log = record.run("native-build", [wrapper, "+1.97.0", "build", "--locked", "--release",
        "--manifest-path", manifest, "-p", "meerkat-connect", "--features", "console,toolkit-mobkit/native-host",
        "--bin", "toolkit_gateway", "--message-format=json-render-diagnostics"], env=bound_env, seconds=5400)
    emitted = [json.loads(line) for line in cargo_log.read_text().splitlines() if line.startswith('{')]
    binaries = [Path(row["executable"]) for row in emitted if row.get("reason") == "compiler-artifact"
                and row.get("target", {}).get("name") == "toolkit_gateway" and row.get("executable")]
    require(len(binaries) == 1, "one actual emitted gateway executable required")
    binary = work / "binary/toolkit_gateway"
    copy_exact(binaries[0], binary)
    binary.chmod(0o755)
    native_record = work / "build-record.json"
    write_new(native_record, {"schema_version": 1, "target": target, "binary_kind": "toolkit_gateway",
        "resources_sha256": resource_value["resources_sha256"], "binary": binding(binary), "cargo": cargo,
        "source_manifest": binding(source_manifest), "resource_build_record_sha256": digest(resource_record)})
    assembly = [sys.executable, ROOT / "scripts/assemble-connect-product.py", "--binary", binary,
        "--binary-sha256", digest(binary), "--build-record", native_record,
        "--resource-build-record", resource_record, "--resource-build-record-sha256", digest(resource_record),
        "--binary-kind", "toolkit_gateway", "--target", target, "--release-version", value["connect_version"]]
    record.run("stage", [*assembly, "--stage-only", "--destination", work / "stage"], seconds=600)
    # Install the original selected Meta wheel and the just-built Connect wheel.
    # There is no network dependency resolution or second SDK build here.
    venv = Path(os.environ["RUNNER_TEMP"]) / "connect-verifier-venv"
    require(not venv.exists(), "fresh verifier environment required")
    record.run("venv", [sys.executable, "-m", "venv", venv])
    python = venv / "bin/python"
    dependency = value["dependencies"]
    wheelhouse = work / "wheelhouse"
    wheelhouse.mkdir()
    for name in dependency["wheels"]:
        copy_exact(inputs / name, wheelhouse / Path(name).name)
    record.run("dependencies", [python, "-m", "pip", "install", "--no-index", "--no-deps", "--require-hashes",
        "--find-links", wheelhouse, "-r", inputs / dependency["requirements"]])
    record.run("sdks", [python, "-m", "pip", "install", "--no-index", "--no-deps",
        sdk_wheels["meta"], sdk_wheels["toolkit"]])
    _, site_log = record.run("site", [python, "-c", "import sysconfig; print(sysconfig.get_path('purelib'))"])
    site = Path(site_log.read_text().strip())
    staged = work / "stage/staging.json"
    record.run("installed-product", [python, "-I", ROOT / "scripts/verify_native_gateway_subprocess.py",
        "--toolkit-gateway", work / ("stage/meerkat-connect-" + target + "/bin/toolkit_gateway"),
        "--toolkit-python", site, "--meta-python", site, "--case", "installed-product",
        "--installed-staging", staged, "--installed-staging-sha256", digest(staged),
        "--output", work / "verification"], seconds=900)
    verification = work / "verification/manifest.json"
    report = read_json(verification)
    require(report["status"] == "passed", "actual installed verification failed")
    record.run("assemble", [*assembly, "--destination", work / "assembled",
        "--gateway-verification", verification, "--gateway-verification-sha256", digest(verification)], seconds=600)
    final = read_json(work / "assembled/assembly.json")
    require(final["scope"] == SCOPE and final["release_version"] == value["connect_version"],
            "final assembly scope or version differs")
    for name in ("assembly.json", "build-record.json", "resource-build-record.json", "gateway-verification.json", final["archive"], final["archive"] + ".sha256"):
        copy_exact(work / "assembled" / name, artifacts / name)
    for name in ("meta", "toolkit"):
        copy_exact(sdk_wheels[name], artifacts / sdk_wheels[name].name)
    for name in dependency["wheels"]:
        copy_exact(inputs / name, artifacts / "dependencies" / Path(name).name)
    copy_exact(inputs / dependency["requirements"], artifacts / "dependency-requirements.txt")
    catalog = Path(report["cases"][0]["model_catalog"]["path"])
    require(digest(catalog) == report["cases"][0]["model_catalog"]["sha256"], "actual catalog changed")
    copy_exact(catalog, artifacts / "model-catalog.json")
    # Exact public reader shape. The producer's absolute locators stay untouched;
    # Forge owns only their later physical relocation, not record rewriting.
    selection = {"format": "forge-meta-toolkit-runtime-selection-v1", "artifact_root": str(artifacts), "target": target,
        "assembly": binding(artifacts / "assembly.json"), "source_manifest": binding(source_manifest),
        "meta_wheel": binding(artifacts / sdk_wheels["meta"].name),
        "toolkit_wheel": binding(artifacts / sdk_wheels["toolkit"].name),
        "dependency_requirements": binding(artifacts / "dependency-requirements.txt"),
        "dependency_wheels": [binding(artifacts / "dependencies" / Path(name).name) for name in dependency["wheels"]],
        "model_catalog": binding(artifacts / "model-catalog.json"), **expectations}
    write_new(artifacts / "RUNTIME-SELECTION.json", selection)
    # Use the selected Forge source's real reader, not a Connect-side reimplementation
    # of the installer acceptance contract. An absent/incompatible reader refuses.
    record.run("forge-reader", [python, "-I", "-c",
        "import json,sys; from pathlib import Path; "
        "sys.path.insert(0,sys.argv[1]); "
        "from launchkit.runtime_selection import parse_candidate_selection; "
        "from launchkit.runtime_candidates import read_meta_toolkit_artifacts; "
        "target=parse_candidate_selection(json.loads(Path(sys.argv[2]).read_text())); "
        "observed=read_meta_toolkit_artifacts(target); "
        "assert observed.origin=='installed-gateway-assembly'; "
        "print(json.dumps({'origin':observed.origin,'status':'verified','published':False}))",
        reader / "installer", artifacts / "RUNTIME-SELECTION.json"], seconds=300)
    require(source_state() == (revision, tree), "selected source changed during production")
    for path in ("native-candidate/Cargo.toml", "native-candidate/Cargo.lock"):
        require(digest(ROOT / path) == value["source_pins"][path], "Cargo changed selected source")
    # Retain raw records and input provenance, but not tool caches, checkouts or
    # the installed venv. The original source/SDK identities remain in records.
    closure = Path(os.environ["RUNNER_TEMP"]) / "connect-export"
    closure.mkdir()
    for name in ("artifacts", "records", "inputs", "upstream", "starter", "resources", "stage", "verification", "binary", "tmp", "sdk-built"):
        shutil.copytree(work / name, closure / name, symlinks=True)
    for name in ("inputs-selection.json", "build-record.json", "starter-library-selection.json",
                 "sdk_build-selection.json", "dependencies-selection.json"):
        copy_exact(work / name, closure / name)
    rows = []
    for path in sorted(closure.rglob("*")):
        if path.is_symlink():
            raw = os.readlink(path).encode()
            rows.append({"path": path.relative_to(closure).as_posix(), "kind": "symlink",
                         "size": len(raw), "sha256": hashlib.sha256(raw).hexdigest()})
        elif path.is_file() and not path.is_symlink():
            if path.suffix == ".json":
                read_json(path)
            elif path.suffix in {".log", ".txt", ".sha256"} or path.name == "gateway.stderr":
                path.read_text(encoding="utf-8", errors="strict")
            rows.append({"path": path.relative_to(closure).as_posix(), "size": path.stat().st_size, "sha256": digest(path)})
    write_new(closure / "CONTENTS.json", {"schema_version": 1, "files": rows})
    dist = ROOT / "dist"
    dist.mkdir()
    archive = dist / subjects[0]
    archive_tree(closure, archive)
    require(0 < archive.stat().st_size <= MAX_FILE, "candidate wrapper exceeds Forge public input/archive limit")
    write_new(dist / subjects[1], {"schema_version": 1, "kind": "connect-installed-candidate",
        "repository": os.environ["GITHUB_REPOSITORY"], "source_sha": revision, "source_tree": tree,
        "workflow_sha": os.environ["GITHUB_WORKFLOW_SHA"], "run_id": os.environ["GITHUB_RUN_ID"],
        "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"], "target": target, "scope": SCOPE,
        "connect_version": value["connect_version"], "native_release": value["native_release"],
        "inputs_sha256": digest(inputs_path), "archive": {"name": archive.name, "size": archive.stat().st_size, "sha256": digest(archive)},
        "runtime_selection_sha256": digest(artifacts / "RUNTIME-SELECTION.json"), "publication": "not_performed"})


CLI_COMPOSITION = "installed_product_native_catalog_preparation_and_help"
CLI_SHUTDOWN = "installed_product_native_shutdown_joins_active_worker"


def cli_producer_inputs(work):
    """Reuse one retained producer, without relocating its execution records."""
    require(work.is_absolute() and work.resolve(strict=True) == work,
            "canonical retained candidate work directory required")
    source_path = work / "artifacts/source-manifest.json"
    source = read_json(source_path)
    selection = work / "inputs-selection.json"
    require(binding(selection) == source["input_selection"], "retained input selection changed")
    value = read_json(selection)
    require(source["revision"] == subprocess.check_output(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip(), "CLI producer source revision differs")
    for row in source["files"]:
        path = ROOT / relative(row["path"])
        require(not path.is_symlink() and binding(path)["sha256"] == row["sha256"]
                and path.stat().st_size == row["size"], "CLI producer source bytes differ")
    metadata = source["metadata"]
    require(binding(Path(metadata["path"])) == metadata, "retained native graph evidence changed")
    require(value["native_release"] == source["native_release"], "CLI native selection differs")
    resources = work / "resources/build.json"
    native = read_json(work / "build-record.json")
    require(native["source_manifest"] == binding(source_path)
            and native["resource_build_record_sha256"] == digest(resources)
            and native["resources_sha256"] == read_json(resources)["resources_sha256"],
            "CLI inputs differ from the prepared native candidate")
    return value, source_path, resources


def cli_assembly_args(work, build_record):
    value = read_json(work / "inputs-selection.json")
    resources = work / "resources/build.json"
    binary = work / "cli/binary/meerkat-connect"
    return [sys.executable, ROOT / "scripts/assemble-connect-product.py", "--binary", binary,
        "--binary-sha256", digest(binary), "--build-record", build_record,
        "--resource-build-record", resources, "--resource-build-record-sha256", digest(resources),
        "--binary-kind", "meerkat-connect", "--target", value["target"],
        "--release-version", value["connect_version"]]


def emitted_executable(log, name, *, test):
    rows = [json.loads(line) for line in log.read_text().splitlines() if line.startswith('{')]
    paths = [Path(row["executable"]) for row in rows if row.get("reason") == "compiler-artifact"
             and row.get("target", {}).get("name") == name and row.get("executable")
             and row.get("profile", {}).get("test") is test]
    require(len(paths) == 1, "one exact emitted executable required: " + name)
    return paths[0]


def cli_test_evidence(record, installed, name, directory, prefix):
    directory.mkdir()
    for leaf in ("home", "tmp", "empty-path"):
        (directory / leaf).mkdir()
    execution, log = record.run(name, [installed, "--exact", "--ignored", name, "--nocapture"],
        cwd=directory, env={"HOME": str(directory / "home"), "TMPDIR": str(directory / "tmp"),
            "PATH": str(directory / "empty-path"), "RUST_TEST_THREADS": "1", "RUST_MIN_STACK": "8388608"},
        seconds=900)
    # libtest exits successfully when a filter selects zero tests, too.
    require(re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", log.read_text()),
            "the selected installed test did not actually run once")
    roots = list((directory / "tmp").glob(prefix + "*"))
    require(len(roots) == 1 and (roots[0] / "native-dispatch.jsonl").is_file(),
            "owned installed test evidence is missing")
    evidence = [binding(record.records / (name + ".json")), binding(log)]
    # The existing tests retain their raw trace, descriptor, fixture and process
    # markers at this root. The realm/runtime directories remain in place too.
    evidence += [binding(path) for path in sorted(roots[0].iterdir()) if path.is_file()]
    return execution, evidence


def stage_cli(work):
    selected, revision, _ = preflight(native=True)
    value, source, resources = cli_producer_inputs(work)
    require(value == selected and read_json(source)["revision"] == revision,
            "CLI stage differs from the selected candidate")
    cli = work / "cli"
    cli.mkdir()  # Each producer attempt is retained; no in-place rerun.
    record = Recorder(cli)
    resource = read_json(resources)
    bound_env = {"MEERKAT_CONNECT_RESOURCES_SHA256": resource["resources_sha256"]}
    common = ["--locked", "--release", "--manifest-path", ROOT / "native-candidate/Cargo.toml",
              "-p", "meerkat-connect", "--features", "console,toolkit-mobkit/native-host",
              "--message-format=json-render-diagnostics"]
    # Copy production bytes before test compilation can unify dev features or
    # replace Cargo's ordinary output. The fixture is a separate executable.
    cargo, log = record.run("cli-build", [ROOT / "scripts/repo-cargo", "+1.97.0", "build",
        *common, "--bin", "meerkat-connect"], env=bound_env, seconds=5400)
    binary = cli / "binary/meerkat-connect"
    copy_exact(emitted_executable(log, "meerkat-connect", test=False), binary)
    binary.chmod(0o755)
    _, log = record.run("cli-test-build", [ROOT / "scripts/repo-cargo", "+1.97.0", "test",
        *common, "--test", "packaged_setup", "--no-run"], env=bound_env, seconds=5400)
    tested = cli / "binary/packaged_setup"
    copy_exact(emitted_executable(log, "packaged_setup", test=True), tested)
    tested.chmod(0o755)
    build_path = cli / "build-record.json"
    write_new(build_path, {"schema_version": 1, "target": value["target"], "binary_kind": "meerkat-connect",
        "resources_sha256": resource["resources_sha256"], "binary": binding(binary), "cargo": cargo,
        "source_manifest": binding(source), "resource_build_record_sha256": digest(resources),
        "test_executable": binding(tested)})
    record.run("cli-stage", [*cli_assembly_args(work, build_path), "--stage-only",
        "--destination", cli / "stage"], seconds=600)
    staged = cli / "stage/staging.json"
    candidate = read_json(staged)["candidate"]
    installed = cli / ("stage/meerkat-connect-" + value["target"] + "/bin/packaged_setup")
    copy_exact(tested, installed)
    installed.chmod(0o755)
    execution, composition_evidence = cli_test_evidence(record, installed, CLI_COMPOSITION,
        cli / "composition-run", "connect-product-acceptance-")
    write_new(cli / "installed-composition.json", {"schema_version": 1,
        "kind": "meerkat_connect_installed_composition", "scope": CLI_COMPOSITION,
        "status": "passed", "candidate": candidate, "test_executable": binding(installed),
        "execution": execution, "evidence": composition_evidence})
    execution, evidence = cli_test_evidence(record, installed, CLI_SHUTDOWN,
        cli / "shutdown-run", "connect-active-worker-acceptance-")
    evidence += [binding(path) for path in (staged, build_path, resources, source,
        cli / "records/cli-build.json", cli / "records/cli-test-build.json",
        cli / "installed-composition.json")]
    # This is deliberately not accepted. An independent reviewer assesses the
    # retained observations and emits the existing accepted review separately.
    write_new(cli / "native-shutdown-review.pending.json", {"schema_version": 1,
        "kind": "meerkat_connect_native_shutdown_review", "scope": "active_setup_worker_shutdown",
        "disposition": "pending", "candidate": candidate, "test_executable": binding(installed),
        "execution": execution, "evidence": evidence})
    cli_producer_inputs(work)


def assemble_cli(work, review, review_sha256, output, *, rules=None, exemptions=None, accepted_findings=None):
    privacy = privacy_inputs(rules, exemptions, accepted_findings)
    value, _, resources = cli_producer_inputs(work)
    cli = work / "cli"
    staged = read_json(cli / "stage/staging.json")
    build_path = cli / "build-record.json"
    require(digest(build_path) == staged["build_record_sha256"]
            and digest(resources) == staged["resource_build_record_sha256"], "CLI staged inputs changed")
    require(HEX.fullmatch(review_sha256) and digest(review) == review_sha256,
            "exact independent shutdown review required")
    # Invoke the existing validators, not a second definition of acceptance.
    import runpy
    assembler = runpy.run_path(str(ROOT / "scripts/assemble-connect-product.py"))
    helpers = runpy.run_path(str(assembler["PREPARE"]))
    candidate = staged["candidate"]
    binary = cli / "binary/meerkat-connect"
    _, observed = assembler["build_record"](build_path, binary, digest(binary), helpers,
        target=value["target"], resources_sha=candidate["resources_sha256"],
        binary_kind="meerkat-connect", resource_record_sha=digest(resources), stage_only=True)
    require(observed["candidate"] == candidate, "CLI staged candidate changed")
    composition = cli / "installed-composition.json"
    assembler["composition_record"](binding(composition), helpers, candidate)
    assembler["shutdown_record"](binding(review), helpers, candidate)
    pending = read_json(cli / "native-shutdown-review.pending.json")
    accepted = read_json(review)
    require(pending["candidate"] == candidate and all(row in accepted["evidence"] for row in pending["evidence"]),
            "shutdown review does not cover this producer's exact observations")
    require(output.is_absolute() and not output.exists(), "fresh CLI assembly output required")
    output.mkdir()
    record = Recorder(output)
    final_build = read_json(build_path)
    final_build.update(installed_composition=binding(composition), native_shutdown_review=binding(review))
    final_path = output / "qualified-build-record.json"
    write_new(final_path, final_build)
    record.run("cli-assemble", [*cli_assembly_args(work, final_path), "--destination", output / "assembled"], seconds=600)
    final = read_json(output / "assembled/assembly.json")
    require(final["scope"] == "conversation_catalog_local_preparation_and_help_preview"
            and final["release_version"] == value["connect_version"]
            and final["qualification"]["candidate"] == candidate
            and final["qualification"]["cli_tty_qualified"] is False, "CLI assembly scope or candidate differs")
    # Release mode: the assembled product counts as assembled only after its privacy scan. A refused
    # product is moved aside, so no product or assembly record stays at the release location.
    archive = output / "assembled" / final["archive"]
    try:
        privacy_scan([archive, archive.with_name(archive.name + ".sha256")], privacy, output / "privacy-scan")
    except ValueError:
        (output / "assembled").rename(output / "assembled.refused")
        raise


# The release privacy scan: the pinned engine (release-evidence-privacy-scan.py, unchanged)
# through every nested container of the exact release payload. The rules and exemptions are
# private and never committed, so release mode takes them on the command line and refuses
# without them. An accepted-findings file kept beside the rules may accept findings in exact
# file digests; without one nothing is accepted.
PRIVACY_SCAN = ROOT / "scripts/release-payload-privacy-scan.py"


def privacy_inputs(rules, exemptions, accepted_findings):
    require(rules is not None and exemptions is not None,
            "release mode requires --rules and --exemptions (private files, never committed)")
    for path in (rules, exemptions, accepted_findings):
        require(path is None or (path.is_file() and not path.is_symlink()),
                "privacy scan rules, exemptions and accepted findings must be regular files")
    return rules, exemptions, accepted_findings


def privacy_scan(payload, privacy, output):
    """Scan the exact payload files. The report holds counts only; the detail holds the matched
    strings and stays private (mode 0600). Refuses unless the scan is clean."""
    rules, exemptions, accepted_findings = privacy
    require(payload and all(path.is_file() and not path.is_symlink() for path in payload),
            "regular payload files required")
    require(output.is_absolute() and not output.exists(), "fresh privacy scan output required")
    output.mkdir(mode=0o700)
    command = [sys.executable, "-I", PRIVACY_SCAN, "--rules", rules, "--exemptions", exemptions,
               "--report", output / "report.json", "--detail", output / "detail.json", "--work", output]
    if accepted_findings is not None:
        command += ["--accepted-findings", accepted_findings]
    code = subprocess.run([str(item) for item in [*command, *payload]], stdin=subprocess.DEVNULL).returncode
    require(code == 0 and read_json(output / "report.json")["status"] == "clean",
            f"the release privacy scan refused the payload (exit {code}); counts in {output / 'report.json'}")
    return read_json(output / "report.json")


def fetch_built():
    value, _, _ = preflight()
    target = value["target"]
    subjects = publication_files(target)[:2]
    identifier = os.environ.get("CONNECT_ARTIFACT_ID", "")
    expected = os.environ.get("CONNECT_ARTIFACT_DIGEST", "").removeprefix("sha256:")
    require(identifier.isdigit() and HEX.fullmatch(expected), "exact build artifact ID and digest required")
    endpoint = "repos/" + os.environ["GITHUB_REPOSITORY"] + "/actions/artifacts/" + identifier
    metadata = json.loads(subprocess.check_output(["gh", "api", endpoint]))
    expected_name = "connect-" + target + "-build-" + os.environ["GITHUB_RUN_ID"] + "-" + os.environ["GITHUB_RUN_ATTEMPT"]
    require(metadata["name"] == expected_name and str(metadata["workflow_run"]["id"]) == os.environ["GITHUB_RUN_ID"]
            and metadata["workflow_run"]["head_sha"] == os.environ["CONNECT_SOURCE_SHA"]
            and not metadata["expired"] and metadata["digest"].removeprefix("sha256:") == expected,
            "artifact run, attempt, source or digest differs")
    archive = Path(os.environ["RUNNER_TEMP"]) / "connect-built.zip"
    with archive.open("xb") as stream:
        subprocess.run(["gh", "api", endpoint + "/zip"], stdout=stream, check=True)
    require(digest(archive) == expected, "downloaded artifact digest differs")
    dist = ROOT / "dist"
    dist.mkdir()
    with zipfile.ZipFile(archive) as zipped:
        entries = zipped.infolist()
        require(len(entries) == 2 and {entry.filename for entry in entries}
                == set(subjects), "unexpected attestation subject set")
        for entry in entries:
            require(0 < entry.file_size <= MAX_FILE and not stat.S_ISLNK(entry.external_attr >> 16),
                    "invalid attestation subject")
            with zipped.open(entry) as source, (dist / entry.filename).open("xb") as destination:
                shutil.copyfileobj(source, destination)
    manifest = read_json(dist / subjects[1])
    checked_candidate_binding(manifest, dist / subjects[0], {
        "target": target,
        "source_sha": os.environ["CONNECT_SOURCE_SHA"], "workflow_sha": os.environ["GITHUB_WORKFLOW_SHA"],
        "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "inputs_sha256": os.environ["CONNECT_INPUTS_SHA256"]})


# Publication consumes this producer's installed preview, never a rebuilt or
# relabeled staging product. This local acceptance record is not a new runtime
# selection: the original v1 document and Forge-owned reader remain authoritative.
# Connect releases from the monorepo (lukacf/meerkat) on its own version line:
# candidates build in connect-release.yml, and releases are connect-vX.Y.Z.
# Releases up to 0.9.4 stay in lukacf/meerkat-connect.
PUBLICATION_REPO = "lukacf/meerkat"
PUBLICATION_WORKFLOW = ".github/workflows/connect-release.yml"
SLSA = "https://slsa.dev/provenance/v1"
MAX_METADATA = 32 * 1024**2


def checked_file(path, maximum=MAX_FILE):
    require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= maximum,
            "missing, linked or oversized publication input")
    return {"name": path.name, "size": path.stat().st_size, "sha256": digest(path)}


def checked_publication_request(value):
    require(set(value) == {"source_sha", "source_tree", "source_ref", "run_id", "run_attempt",
                          "inputs_sha256", "connect_version", "release_id", "tag", "target"},
            "exact publication request fields required")
    checked_target(value["target"])
    require(SHA.fullmatch(value["source_sha"]) and SHA.fullmatch(value["source_tree"])
            and HEX.fullmatch(value["inputs_sha256"]) and VERSION.fullmatch(value["connect_version"]),
            "unbound publication source, version or inputs")
    require(re.fullmatch(r"refs/heads/[A-Za-z0-9][A-Za-z0-9._/-]*", value["source_ref"])
            and ".." not in value["source_ref"] and "//" not in value["source_ref"], "invalid source ref")
    require(all(type(value[key]) is int and value[key] > 0 for key in ("run_id", "run_attempt", "release_id"))
            and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", value["tag"]),
            "explicit existing release and positive run identity required")
    return value


def read_only_environment(work):
    # No inherited GitHub authentication/config or product credentials enter
    # the verifier/reader. Bundle verification needs no artifact-download token.
    home = work / "reader-home"
    home.mkdir()
    return {"PATH": os.environ.get("PATH", ""), "HOME": str(home),
            "GH_CONFIG_DIR": str(home), "LC_ALL": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1"}


def command_json(argv, *, env=None, stdin=None):
    return json.loads(subprocess.check_output([str(x) for x in argv], env=env, stdin=stdin, timeout=300),
                      object_pairs_hook=unique)


def verify_publication_subject(path, bundle, request, *, env=None):
    repo = "https://github.com/" + PUBLICATION_REPO
    sha, ref = request["source_sha"], request["source_ref"]
    identity = f"{repo}/{PUBLICATION_WORKFLOW}@{ref}"
    invocation = f"{repo}/actions/runs/{request['run_id']}/attempts/{request['run_attempt']}"
    # This is policy over gh's cryptographically verified result. Never decode
    # the unsigned bundle as evidence or implement signature verification here.
    verified = command_json(["gh", "attestation", "verify", path, "--bundle", bundle, "--hostname", "github.com",
        "--repo", PUBLICATION_REPO, "--source-digest", sha, "--source-ref", ref,
        "--signer-digest", sha, "--cert-identity", identity,
        "--cert-oidc-issuer", "https://token.actions.githubusercontent.com",
        "--deny-self-hosted-runners", "--predicate-type", SLSA, "--format", "json"], env=env)
    require(isinstance(verified, list) and verified, "no verified provenance")
    expected = {"issuer": "https://token.actions.githubusercontent.com",
        "subjectAlternativeName": identity, "buildSignerURI": identity, "buildSignerDigest": sha,
        "sourceRepositoryURI": repo, "sourceRepositoryDigest": sha, "sourceRepositoryRef": ref,
        "buildConfigURI": identity, "buildConfigDigest": sha, "runnerEnvironment": "github-hosted",
        "buildTrigger": "workflow_dispatch", "runInvocationURI": invocation}
    for entry in verified:
        result = entry["verificationResult"]
        certificate = result["signature"]["certificate"]
        require(all(certificate.get(k) == v for k, v in expected.items())
                and result.get("verifiedTimestamps"), "verified certificate identity differs")
        statement = result["statement"]
        require(statement["_type"] == "https://in-toto.io/Statement/v1"
                and statement["predicateType"] == SLSA, "wrong verified statement type")
        subjects = [s for s in statement["subject"] if s.get("name") == path.name]
        require(len(subjects) == 1 and subjects[0]["digest"] == {"sha256": digest(path)},
                "verified subject differs")
        definition = statement["predicate"]["buildDefinition"]
        require(definition["buildType"] == "https://actions.github.io/buildtypes/workflow/v1"
                and definition["externalParameters"]["workflow"] ==
                    {"repository": repo, "path": PUBLICATION_WORKFLOW, "ref": ref}
                and definition["internalParameters"]["github"]["event_name"] == "workflow_dispatch"
                and definition["internalParameters"]["github"]["runner_environment"] == "github-hosted"
                and definition["resolvedDependencies"] ==
                    [{"uri": f"git+{repo}@{ref}", "digest": {"gitCommit": sha}}],
                "verified workflow/source differs")
        details = statement["predicate"]["runDetails"]
        require(details["builder"]["id"] == identity and details["metadata"]["invocationId"] == invocation,
                "verified invocation differs")
    return verified


def checked_candidate_binding(manifest, payload, expected):
    require(all(manifest[key] == expected[key] for key in
                ("source_sha", "workflow_sha", "run_id", "run_attempt", "inputs_sha256", "target")),
            "candidate binding differs")
    require(manifest["archive"] == {"name": payload.name, "size": payload.stat().st_size,
                                    "sha256": digest(payload)}, "candidate closure differs")


def checked_publication_candidate(directory, request):
    files = publication_files(request["target"])
    subjects = files[:2]
    rows = [checked_file(directory / name, MAX_FILE if name == subjects[0] else MAX_METADATA)
            for name in files]
    manifest = read_json(directory / subjects[1])
    checked_candidate_binding(manifest, directory / subjects[0], {
        "target": request["target"], "source_sha": request["source_sha"], "workflow_sha": request["source_sha"],
        "run_id": str(request["run_id"]), "run_attempt": str(request["run_attempt"]),
        "inputs_sha256": request["inputs_sha256"]})
    require(manifest["schema_version"] == 1 and manifest["kind"] == "connect-installed-candidate"
            and manifest["repository"] == PUBLICATION_REPO and manifest["source_tree"] == request["source_tree"]
            and manifest["target"] == request["target"] and manifest["scope"] == SCOPE
            and manifest["connect_version"] == request["connect_version"]
            and manifest["publication"] == "not_performed", "candidate scope or source differs")
    checked_native_releases(manifest["native_release"])
    return manifest, rows


def unpack_publication_candidate(archive_path, destination):
    """Data-only bounded extraction; retained links are checked but not created."""
    import posixpath
    destination.mkdir()
    inventory, names, leaves, parents, expanded = [], set(), set(), set(), 0
    with tarfile.open(archive_path, "r:gz") as archive:
        for member in archive:
            name = relative(member.name)
            parts = PurePosixPath(name).parts
            require(parts[0] == "connect-export" and all(p not in (".", "..") for p in parts)
                    and len(name) <= 4096 and len(parts) <= 64 and name not in names and len(names) < 32768, "invalid candidate archive inventory")
            require(not any(str(parent) in leaves for parent in PurePosixPath(name).parents),
                    "candidate member traverses an archive leaf")
            names.add(name)
            parents.update(str(parent) for parent in PurePosixPath(name).parents)
            if member.isdir():
                continue
            require(len(parts) > 1 and (member.isfile() or member.issym()), "unsupported archive member")
            require(name not in parents, "candidate leaf shadows descendants")
            leaves.add(name)
            rel = PurePosixPath(*parts[1:]).as_posix()
            if member.issym():
                link = member.linkname
                resolved = posixpath.normpath(str(PurePosixPath(name).parent / link))
                require(link and not link.startswith("/") and "\\" not in link
                        and resolved.startswith("connect-export/"), "escaping candidate link")
                raw = link.encode()
                inventory.append({"path": rel, "kind": "symlink", "size": len(raw),
                                  "sha256": hashlib.sha256(raw).hexdigest()})
                continue
            require(0 <= member.size <= MAX_FILE, "candidate member exceeds bound")
            expanded += member.size
            require(expanded <= 16 * 1024**3, "candidate expanded closure exceeds bound")
            path = destination / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as source, path.open("xb") as output:
                shutil.copyfileobj(source, output)
            require(path.stat().st_size == member.size, "truncated candidate member")
            if path.suffix == ".json":
                require(member.size <= MAX_METADATA, "candidate metadata exceeds bound")
                read_json(path)
            elif path.suffix in {".log", ".txt", ".sha256"} or path.name == "gateway.stderr":
                require(member.size <= MAX_METADATA, "candidate text exceeds bound")
                path.read_text()
            if rel != "CONTENTS.json":
                inventory.append({"path": rel, "size": member.size, "sha256": digest(path)})
    contents = read_json(destination / "CONTENTS.json")
    require(contents == {"schema_version": 1, "files": sorted(inventory, key=lambda row: row["path"])},
            "original candidate inventory differs")
    return {row["path"]: row for row in inventory}


def read_publication_product(root, reader, manifest, request, work, env):
    inputs = read_json(root / "inputs-selection.json")
    require(digest(root / "inputs-selection.json") == request["inputs_sha256"]
            and inputs["native_release"] == manifest["native_release"]
            and inputs["connect_version"] == request["connect_version"]
            and inputs["target"] == request["target"], "original input selection differs")
    def git(*args):
        return subprocess.check_output(["git", "-C", str(reader), *args], env=env, timeout=30, text=True).strip()
    selected = checked_forge_reader_source(inputs, allow_legacy=True)
    require(git("rev-parse", "--verify", "HEAD^{commit}") == selected["source_sha"]
            and (not selected["tag"] or git("rev-parse", "--verify",
                "refs/tags/" + selected["tag"] + "^{commit}") == selected["source_sha"])
            and git("status", "--porcelain", "--untracked-files=all", "--ignored", "--", "installer") == "",
            "reader is not the exact clean selected Forge checkout")
    require(not any(reader.joinpath("installer").rglob("*.pyc")), "reader checkout contains cached Python code")
    checked_forge_reader_files(reader, inputs)
    document = root / "artifacts/RUNTIME-SELECTION.json"
    require(digest(document) == manifest["runtime_selection_sha256"], "original selection differs")
    selection = read_json(document)
    require(selection["target"] == request["target"], "runtime selection target differs")
    # This is the existing Forge fetch-layout map. The owner alone performs
    # physical relocation on its in-memory target; no serialized bytes change.
    artifact_root = PurePosixPath(selection["artifact_root"])
    def local(item):
        return "artifacts/" + relative(PurePosixPath(item["path"]).relative_to(artifact_root).as_posix())
    paths = {name: local(selection[name]) for name in
             ("assembly", "source_manifest", "meta_wheel", "toolkit_wheel", "dependency_requirements", "model_catalog")}
    paths.update(artifact_root="artifacts", dependency_wheels=[local(x) for x in selection["dependency_wheels"]])
    layout = work / "layout.json"
    write_new(layout, paths)
    output = command_json([sys.executable, "-I", "-B", "-c",
        "import json,sys; from pathlib import Path; sys.path.insert(0,sys.argv[1]); "
        "from launchkit.runtime_selection import parse_candidate_selection,selected_runtime_artifacts; "
        "from launchkit.packaged_runtime import relocate_selection; "
        "source=json.loads(Path(sys.argv[2]).read_bytes()); parse_candidate_selection(source); "
        "root=Path(sys.argv[4]); target=relocate_selection(source,json.loads(Path(sys.argv[3]).read_bytes()),root); "
        "observed=selected_runtime_artifacts(target); "
        "assert observed.origin=='installed-gateway-assembly'; "
        "print(json.dumps({'origin':observed.origin,'evidence':[{'path':str(f.path.relative_to(root)),"
        "'size':f.size,'sha256':f.sha256} for f in observed.evidence]}))",
        reader / "installer", document, layout, root], env=env)
    require(output["origin"] == "installed-gateway-assembly" and output["evidence"], "Forge did not accept installed preview")
    assembly = read_json(root / paths["assembly"])
    require(assembly["scope"] == SCOPE and assembly["release_version"] == request["connect_version"],
            "assembly is not the selected installed preview")
    evidence = list(output["evidence"])
    for extra in ("artifacts/RUNTIME-SELECTION.json", "artifacts/" + assembly["archive"] + ".sha256"):
        row = checked_file(root / extra)
        evidence.append({"path": extra, "size": row["size"], "sha256": row["sha256"]})
    return selection, paths, evidence


def accept_publication(request_path, candidate, reader, output):
    checked_file(request_path, MAX_METADATA)
    request = checked_publication_request(read_json(request_path))
    files = publication_files(request["target"])
    subjects = files[:2]
    require(not output.exists(), "fresh acceptance directory required")
    output.mkdir()
    env = read_only_environment(output)
    # Retain each input once before verifier/reader awaits. Recheck these exact
    # copies before committing acceptance; no write credential enters children.
    assets = output / "assets"
    assets.mkdir()
    for name in files:
        checked_file(candidate / name, MAX_FILE if name == subjects[0] else MAX_METADATA)
        copy_exact(candidate / name, assets / name)
    manifest, rows = checked_publication_candidate(assets, request)
    for name in subjects:
        verified = verify_publication_subject(assets / name, assets / files[2], request, env=env)
        write_new(output / (name + ".verification.json"), verified)
    root = output / "unpacked"
    inventory = unpack_publication_candidate(assets / subjects[0], root)
    selection, paths, evidence = read_publication_product(root, reader, manifest, request, output, env)
    require(0 < len(evidence) <= 512 and len({r["path"] for r in evidence}) == len(evidence),
            "bounded distinct owner evidence required")
    for row in evidence:
        relative(row["path"])
        require(row == inventory.get(row["path"]), "owner evidence differs from original closure")
    require(checked_publication_candidate(assets, request)[1] == rows, "accepted assets changed")
    accepted = {"schema_version": 1, "kind": "connect-publication-acceptance", "request": request,
        "scope": SCOPE, "assets": rows, "selection": selection, "paths": paths, "evidence": evidence,
        "publication": "not_performed"}
    write_new(output / "acceptance.json", accepted)
    return accepted


def publication_api(endpoint, *, upload=None):
    argv = ["gh", "api", endpoint, "--hostname", "github.com"]
    if upload is not None:
        argv += ["--method", "POST", "--header", "Content-Type: application/octet-stream", "--input", "-"]
    return command_json(argv, stdin=upload)


def release_state(request):
    endpoint = "repos/" + PUBLICATION_REPO
    repository = publication_api(endpoint)
    require(repository.get("full_name") == PUBLICATION_REPO and repository.get("private") is False
            and repository.get("visibility") == "public", "selected repository is not public")
    release = publication_api(endpoint + "/releases/" + str(request["release_id"]))
    require(release["id"] == request["release_id"] and release["tag_name"] == request["tag"]
            and type(release["draft"]) is bool, "selected existing release differs")
    ref = publication_api(endpoint + "/git/ref/tags/" + request["tag"])["object"]
    for _ in range(4):
        if ref["type"] == "commit":
            break
        require(ref["type"] == "tag" and SHA.fullmatch(ref["sha"]), "invalid release tag target")
        ref = publication_api(endpoint + "/git/tags/" + ref["sha"])["object"]
    require(ref == {"type": "commit", "sha": request["source_sha"], "url": ref.get("url")},
            "release tag does not select accepted source")
    assets = []
    for page in range(1, 7):
        batch = publication_api(endpoint + f"/releases/{request['release_id']}/assets?per_page=100&page={page}")
        require(isinstance(batch, list) and len(batch) <= 100, "invalid release asset page")
        assets.extend(batch)
        require(len(assets) <= 512, "release asset inventory exceeds bound")
        if len(batch) < 100:
            break
    require(len({x["name"] for x in assets}) == len(assets), "ambiguous release asset names")
    # Download counters and other presentation fields change during our own
    # public reads. Keep only the actual asset identity/state in the fence.
    selected = {}
    for row in assets:
        require(type(row["id"]) is int and row["id"] > 0 and type(row["size"]) is int,
                "invalid release asset identity")
        selected[row["name"]] = {key: row[key] for key in
            ("id", "name", "size", "state", "browser_download_url")}
        selected[row["name"]]["digest"] = row.get("digest")
    return release, selected


def readback_asset(asset, expected, request, output):
    url = f"https://github.com/{PUBLICATION_REPO}/releases/download/{request['tag']}/{expected['name']}"
    require(asset["name"] == expected["name"] and asset["size"] == expected["size"]
            and asset["state"] == "uploaded" and asset["browser_download_url"] == url,
            "public asset identity differs")
    # Public GET never receives the write token. download checks both size and hash.
    download(output, {"url": url, "size": expected["size"], "sha256": expected["sha256"]})
    return url


def copy_publication_input(path, output, maximum):
    # Open the selected input once. Replacement of its pathname cannot change
    # this read; modification during the copy must still match the accepted hash.
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as source:
        info = os.fstat(source.fileno())
        require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= maximum,
                "bounded regular publication input required")
        size, hashed = 0, hashlib.sha256()
        while chunk := source.read(1024 * 1024):
            size += len(chunk)
            require(size <= maximum, "publication input grew beyond bound")
            output.write(chunk)
            hashed.update(chunk)
        require(size == info.st_size, "publication input size changed")
    return {"name": path.name, "size": size, "sha256": hashed.hexdigest()}


def forge_public_selection(selection):
    # Only physical locators are omitted. The original attested document stays
    # unchanged; Forge inserts local locators before its native parser runs.
    projected = json.loads(json.dumps(selection, allow_nan=False))
    del projected["artifact_root"]
    names = ("assembly", "meta_wheel", "toolkit_wheel", "dependency_requirements", "model_catalog")
    if "source_manifest" in projected:
        names += ("source_manifest",)
    for name in names:
        del projected[name]["path"]
    for wheel in projected["dependency_wheels"]:
        del wheel["path"]
    return projected


# Pure delivery predicates retained from the selected Forge fill helper. They
# inspect data only; Connect never imports or runs the fill tool here.
FORGE_HYGIENE_SOURCE_SHA256 = "73d9a50e5f7644aa5fa05008b47a25259543a079fde1f8232f95ca31846ec3a4"
FORGE_HYGIENE_RULE = "forge-fill-030-f1-f1b-f2-v1"
# Private names to refuse in delivered evidence (case-insensitive). They are
# publication configuration, never public source: the publishing environment
# supplies them as one regular expression in CONNECT_PUBLICATION_PRIVATE_NAMES.
# Without it no name is refused here; the user-home and absolute-path checks
# still apply.
PRIVATE_NAMES_ENV = "CONNECT_PUBLICATION_PRIVATE_NAMES"


def private_pattern(names=None):
    """The supplied private-name pattern; an empty supply matches nothing."""
    names = os.environ.get(PRIVATE_NAMES_ENV, "") if names is None else names
    return re.compile(names or r"(?!)", re.I)


PRIVATE = private_pattern()
ABSOLUTE = re.compile(r"^(/|~[/\\]|[A-Za-z]:[\\/]|\\\\)")
# A user's home directory. The generic hosted-runner accounts (/Users/runner/, /home/runner/) and the bare
# /home/.cargo/ name no user; /Users/.cargo/ is not exempt.
USER_HOME = re.compile(r"/(?:Users/(?!runner/)|home/(?!runner/)(?!\.cargo/))[^/\s\"'\\]+/")
WHEELS, WHEEL_LISTS = ("meta", "toolkit"), ("files", "metadata", "source_payload")
TEXT_SUFFIXES = (".json", ".jsonl", ".md", ".txt", ".sha256")



def wheel_file_names(selection):
    """F1: the exact file names listed in the selection's own wheel file lists."""
    names = set()
    for wheel in WHEELS:
        value = selection.get(wheel) if isinstance(selection, dict) else None
        for key in WHEEL_LISTS if isinstance(value, dict) else ():
            for row in value.get(key) or ():
                name = row[0] if isinstance(row, list) and row else row
                if isinstance(name, str):
                    names.add(name)
    return names


def hygiene(value, label, selection_at=None):
    """Problems in value: absolute paths, /Users/ or /home/ anywhere, and private names outside the F1 lists.

    selection_at is the number of path parts before a selection's own keys: 0 when value is a selection,
    3 when value is a document (targets.<name>.selection). None exempts nothing."""
    strings, problems = 0, []

    def exempt(path):
        # A wheel list row is [name, sha256, size] (exempt part 0) or a bare name string.
        if selection_at is None or len(path) < selection_at:
            return False
        if selection_at == 3 and not (path[0] == "targets" and path[2] == "selection"):
            return False
        rest = path[selection_at:]
        return (len(rest) in (3, 4) and rest[0] in WHEELS and rest[1] in WHEEL_LISTS
                and isinstance(rest[2], int) and (len(rest) == 3 or rest[3] == 0))

    def walk(item, path):
        nonlocal strings
        if isinstance(item, dict):
            for key, child in item.items():
                check(key, path + ("<key>",))
                walk(child, path + (key,))
        elif isinstance(item, list):
            for index, child in enumerate(item):
                walk(child, path + (index,))
        elif isinstance(item, str):
            check(item, path)

    def check(text, path):
        nonlocal strings
        strings += 1
        where = label + "".join(f"[{part}]" if isinstance(part, int) else f".{part}" for part in path)
        if ABSOLUTE.match(text):
            problems.append(f"{where}: absolute path")
        if "/Users/" in text or "/home/" in text:
            problems.append(f"{where}: user home prefix")
        if PRIVATE.search(text) and not exempt(path):
            problems.append(f"{where}: private name")

    walk(value, ())
    return strings, problems


def text_hits(name, text, wheel_names):
    """(user home paths, private names) in one fetched text input.

    JSON is decoded and every key and string value is checked, so an escaped path cannot evade the path
    check. The F1 and F1b exact-entry exemptions apply to the private-name check only."""
    if name.endswith(".json"):
        try:
            value = json.loads(text)
        except ValueError:
            value = None
        if value is not None:
            source_manifest, homes, private = name.endswith("source-manifest.json"), 0, 0

            def walk(item, path):
                nonlocal homes, private
                if isinstance(item, dict):
                    for key, child in item.items():
                        homes += len(USER_HOME.findall(key))
                        private += len(PRIVATE.findall(key))
                        walk(child, path + (key,))
                elif isinstance(item, list):
                    for index, child in enumerate(item):
                        walk(child, path + (index,))
                elif isinstance(item, str):
                    homes += len(USER_HOME.findall(item))
                    listed = (source_manifest and len(path) == 3 and path[0] == "files"
                              and isinstance(path[1], int) and path[2] == "path")
                    if item not in wheel_names and not listed:
                        private += len(PRIVATE.findall(item))

            walk(value, ())
            return homes, private
    remainder = text
    for exempt in sorted(wheel_names, key=len, reverse=True):
        remainder = remainder.replace(exempt, "")
    return len(USER_HOME.findall(text)), len(PRIVATE.findall(remainder))


def forge_public_files(accepted):
    archive = accepted["assets"][0]
    request = accepted["request"]
    url = f"https://github.com/{PUBLICATION_REPO}/releases/download/{request['tag']}/{archive['name']}"
    return {row["path"]: {"url": url, "archive_size": archive["size"], "archive_sha256": archive["sha256"],
            "member": "connect-export/" + row["path"], "size": row["size"], "sha256": row["sha256"]}
            for row in accepted["evidence"]}


def checked_forge_delivery(directory, accepted, fragment):
    # This is the entire Connect contribution to the fill document. Guide and
    # ordinary gateway pins remain Forge-owned. Nothing is fetched or rewritten.
    _, problems = hygiene({"targets": {accepted["request"]["target"]: fragment}}, "Connect fragment", 3)
    require(not problems, "Forge fragment hygiene refused: " + "; ".join(problems[:20]))
    evidence = accepted["evidence"]
    require(isinstance(evidence, list) and 0 < len(evidence) <= 512
            and len({row["path"] for row in evidence}) == len(evidence), "distinct accepted evidence required")
    exempt = wheel_file_names(fragment["selection"])
    observations = []
    for expected in sorted(evidence, key=lambda row: row["path"]):
        require(set(expected) == {"path", "size", "sha256"}, "invalid accepted evidence pin")
        name = relative(expected["path"])
        require(type(expected["size"]) is int and 0 < expected["size"] <= MAX_FILE
                and isinstance(expected["sha256"], str) and HEX.fullmatch(expected["sha256"]),
                "invalid accepted evidence bytes")
        if not name.endswith(TEXT_SUFFIXES):
            continue
        # Original acceptance already tied this exact row to its attested
        # archive. Re-read its unpacked copy once, verifying the retained bytes
        # before scanning. Path substitution cannot change the selected content.
        with io.BytesIO() as retained:
            row = copy_publication_input(directory / "unpacked" / name, retained, MAX_METADATA)
            require(row["size"] == expected["size"] and row["sha256"] == expected["sha256"],
                    "accepted text evidence changed: " + name)
            homes, private = text_hits(name, retained.getvalue().decode("utf-8", errors="replace"), exempt)
        observations.append({**expected, "user_home_paths": homes, "private_names": private})
    require(observations, "accepted pinned text evidence required")
    require(not any(row["user_home_paths"] or row["private_names"] for row in observations),
            "Forge pinned text hygiene refused: " + ", ".join(row["path"] for row in observations
                if row["user_home_paths"] or row["private_names"]))
    return {"rule": FORGE_HYGIENE_RULE, "helper_sha256": FORGE_HYGIENE_SOURCE_SHA256,
            "text_files": observations}


def publish_accepted(directory, expected_sha256, output):
    # This command deliberately has no extraction or Forge-reader call. The
    # independently selected acceptance hash is the handoff into write custody.
    acceptance_path = directory / "acceptance.json"
    with io.BytesIO() as retained:
        row = copy_publication_input(acceptance_path, retained, MAX_METADATA)
        require(HEX.fullmatch(expected_sha256) and row["sha256"] == expected_sha256,
                "selected acceptance changed")
        retained.seek(0)
        accepted = json.loads(retained.read(), object_pairs_hook=unique)
    require(set(accepted) == {"schema_version", "kind", "request", "scope", "assets", "selection", "paths", "evidence", "publication"}
            and accepted["schema_version"] == 1 and accepted["kind"] == "connect-publication-acceptance"
            and accepted["scope"] == SCOPE and accepted["publication"] == "not_performed", "invalid acceptance")
    request = checked_publication_request(accepted["request"])
    # Project the already selected immutable acceptance before write custody.
    # Neither this projection nor attribution grants publication permission.
    projected_selection = forge_public_selection(accepted["selection"])
    files = publication_files(request["target"])
    subjects = files[:2]
    require(isinstance(accepted["assets"], list) and len(accepted["assets"]) == len(files)
            and [row["name"] for row in accepted["assets"]] == list(files),
            "accepted asset allowlist differs")
    require(not output.exists(), "fresh publication observation directory required")
    output.mkdir(mode=0o700)
    assets = output / "assets"
    assets.mkdir(mode=0o700)
    report = {"status": "refused_or_partial", "write_attempted": [], "verified_assets": [], "scope": SCOPE}
    verified_rows = {}
    try:
        # Take write-phase custody before any verifier/API call. Original
        # handoff paths are never reopened after these checked private copies.
        for expected in accepted["assets"]:
            name = expected["name"]
            with (assets / name).open("xb") as retained:
                row = copy_publication_input(directory / "assets" / name, retained,
                    MAX_FILE if name == subjects[0] else MAX_METADATA)
            require(row == expected, "accepted allowlist bytes differ")
            (assets / name).chmod(0o400)
        require(checked_publication_candidate(assets, request)[1] == accepted["assets"],
                "accepted allowlist bytes differ")
        fragment = {"files": forge_public_files(accepted), "selection": projected_selection,
                    "paths": accepted["paths"]}
        report["forge_delivery"] = checked_forge_delivery(directory, accepted, fragment)
        verifier_env = read_only_environment(output)
        for name in subjects:
            verify_publication_subject(assets / name, assets / files[2], request, env=verifier_env)
        release, original = release_state(request)
        # Drafts remain draft under their release owner's control. This command
        # neither creates nor publishes a release/tag, nor invents public URLs.
        require(not release["draft"], "release owner has not published this selected release")
        # Detect an existing conflict anywhere in the selected set before the
        # first write. Otherwise a later conflicting manifest or bundle would
        # leave newly uploaded assets on a release we already cannot complete.
        for expected in accepted["assets"]:
            name = expected["name"]
            if name in original:
                readback_asset(original[name], expected, request, output / name)
        for expected in accepted["assets"]:
            name = expected["name"]
            current_release, current = release_state(request)
            require(not current_release["draft"] and all(current.get(n) == row for n, row in original.items()),
                    "existing release assets changed")
            if name in current:
                # Equal replay is safe only after reading the existing public bytes.
                pass
            else:
                # The anonymous file stays open through gh's stdin read. No
                # pathname is reopened by the write-capable child process.
                with tempfile.TemporaryFile("w+b", dir=output) as upload:
                    row = copy_publication_input(assets / name, upload,
                        MAX_FILE if name == subjects[0] else MAX_METADATA)
                    require(row == expected, "allowlist changed before upload")
                    upload.seek(0)
                    report["write_attempted"].append(name)
                    publication_api(f"https://uploads.github.com/repos/{PUBLICATION_REPO}/releases/{request['release_id']}/assets?name={name}",
                                    upload=upload)
                current_release, current = release_state(request)
                require(not current_release["draft"], "release became private during upload")
            require(name in current, "uploaded asset was not observed")
            if name not in original:
                readback_asset(current[name], expected, request, output / name)
            report["verified_assets"].append(name)
            verified_rows[name] = current[name]
        final_release, final = release_state(request)
        require(not final_release["draft"] and all(final.get(n) == row for n, row in original.items())
                and all(final.get(n) == row for n, row in verified_rows.items())
                and checked_publication_candidate(assets, request)[1] == accepted["assets"],
                "publication changed during readback")
        files = fragment["files"]
        fragment_path = output / "forge-fragment.json"
        write_new(fragment_path, fragment)
        attribution = {"schema_version": 1,
            "rule": "original-selection-minus-physical-locators-v1",
            "acceptance_sha256": expected_sha256,
            "original_selection": files["artifacts/RUNTIME-SELECTION.json"],
            "fragment": {"path": fragment_path.name, "size": fragment_path.stat().st_size,
                         "sha256": digest(fragment_path)}}
        write_new(output / "forge-fragment-attribution.json", attribution)
        report.update(status="verified_public_inventory", acceptance_sha256=expected_sha256,
                      release_id=request["release_id"], tag=request["tag"], **fragment,
                      selection_projection=attribution)
        return report
    finally:
        write_new(output / "publication.json", report)


# ---------------------------------------------------------------------------------------------------
# The complete macOS arm64 installation: `connect-candidate.py macos <step> --work ABS`.
#
# Stage one runs on a GitHub-hosted macos-15 runner, one step per workflow step, and ends in a stage
# record: every check's evidence, the finished gateway product and a PENDING shutdown review. Stage two
# (`macos assemble`) runs after the independent review accepted that record and assembles the CLI
# product; it executes no product, no Cargo and no signing. `macos release-check` then requires a clean
# private-text scan of exactly the two product archives before anything may be published.
# Everything comes from one commit: this checkout builds Connect, both SDK wheels and the library
# tools. Pinned public files are read from the fetch step's directory; nothing is downloaded here.
# The resource packager, the library packagers and the consumer exporter are called by command line.
# No GNU-only tool is used: hashing, sizes, free space, archives and Mach-O facts are read in Python.
MACOS_TARGET = "macos-arm64"
MACOS_TRIPLE = "aarch64-apple-darwin"
MACOS_PREFIX = "meerkat-connect-" + MACOS_TARGET
MACOS_INPUTS_KIND = "connect-macos-producer-inputs"
MACOS_STAGE_SCHEMA = "connect-macos-stage/1"
MACOS_ASSEMBLY_SCHEMA = "connect-macos-assembly/1"
MACOS_PINS_SCHEMA = "connect-library-pins/1"
WORKSPACE = ROOT.parent
CODESIGN = "/usr/bin/codesign"
# macOS allows 104 bytes for a Unix socket path; the installed checks keep theirs under the work root.
MACOS_WORK_ROOT_LIMIT = 24
MACOS_BUILD_SECONDS = 5 * 3600
MACOS_LANE = "connect-macos-arm64"
MACOS_STEPS = ("preflight", "library", "resources", "build", "stage", "check-gateway", "check-cli",
               "check-consumer", "check-conversation", "check-official-install", "stage-record")
# Each step needs the step records of these earlier steps (stage-record also takes check-consumer, or its
# stated absence).
MACOS_REQUIRES = {"preflight": (), "library": ("preflight",), "resources": ("library",),
                  "build": ("resources",), "stage": ("build",), "check-gateway": ("stage",),
                  "check-cli": ("stage",), "check-consumer": ("stage",), "check-conversation": ("stage",),
                  "check-official-install": ("stage",),
                  "stage-record": ("check-gateway", "check-cli", "check-conversation", "check-official-install")}
# The integration library's native tools: name, Cargo feature of toolkit-package ("" for none). The inspector
# is built with local-artifacts here: the library advertises MCPB archive inspection, which needs it.
MACOS_TOOLS = (("toolkit-local-recipes", "local-recipes"), ("toolkit-package-inspect", "local-artifacts"),
               ("toolkit-python-plan", ""), ("toolkit-node-plan", ""), ("toolkit-compose-plan", ""),
               ("toolkit-mcpb-install", "local-artifacts"))
# The three first-party components of the consumer bundle: family, the exporter's option kind.
MACOS_COMPONENTS = (("agno", "wheel", None), ("services", "wheel", None), ("activepieces", "archive", None))
CONSUMER_EXPORT_TARGET = "darwin-arm64-cpython311"
PACKAGE_LINE = re.compile(r"[A-Za-z0-9_.-]+ v[0-9][^\s]*(?: \([^()]*\))*\Z")
MACOS_SHIPPED = ("toolkit_gateway", "meerkat-connect")
MACOS_TESTS = ("packaged_setup", "cli_paired_host")
CLI_CONSUMER = "actual_cli_public_service_survives_setup_exit_and_fresh_rkat_in_exact_realm"
# What the setup conversation prints around the resource step (crates/meerkat-connect/src/main.rs and
# src/resources.rs; a test pins these three strings to the Rust source).
CONVERSATION_REALM = "Realm: "
CONVERSATION_PREPARING = "Preparing the supplied setup helper."
CONVERSATION_UNAVAILABLE = "Installed Connect resources are unavailable or changed"
MACOS_GATES = ("gateway_verification", "cli_composition", "shutdown_review", "consumer", "conversation_start",
               "official_playwright_install")
# The official Playwright route as the product's setup backend selects it (install-library-consumer.py's own
# fixed profile; a test pins these values to that script). The install hashes the browser, never starts it.
PLAYWRIGHT_CONFIGURATION = {"profile": "isolated-chrome", "config": {}, "operations": [
    "browser_navigate", "browser_snapshot", "browser_fill_form", "browser_click", "browser_close"]}
PLAYWRIGHT_SERVER = "playwright-isolated"
MACOS_CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
# What each check is expected to reach, read from its source and not measured: the producer captures no
# traffic. The stage record adds the hosts each step's own logs and evidence name.
MACOS_NETWORK = {
    "library": "registry.npmjs.org: npm ci of one first-party component's integrity-locked build dependencies "
               "(the build's one network use); every other input is a pinned file or the checkout",
    "check-gateway": "outbound HTTPS to public pages: one live public Hacker News read through the supplied "
                     "integration; controlled model, no provider key",
    "check-cli": "none expected: empty PATH, bundled runtimes",
    "check-consumer": "outbound HTTPS to public pages: live public Hacker News reads through the supplied "
                      "integration; controlled model, no provider key",
    "check-conversation": "none expected: the resource step prepares from the bundled runtimes",
    "check-official-install": "none expected: npm cache add and npm ci run with --offline from the library's "
                              "three pinned Playwright archives"}
# Mach-O constants (read only; nothing here executes or changes a file).
MACHO_MAGIC_64, MACHO_CPU_ARM64, MACHO_EXECUTE = 0xFEEDFACF, 0x0100000C, 2
LC_CODE_SIGNATURE, LC_VERSION_MIN_MACOSX, LC_BUILD_VERSION = 0x1D, 0x24, 0x32
LC_DYLIBS = (0x0C, 0x80000018, 0x8000001F, 0x20, 0x80000023)
CS_SUPERBLOB, CS_CODE_DIRECTORY, CS_ADHOC, CS_LINKER_SIGNED = 0xFADE0CC0, 0xFADE0C02, 0x2, 0x20000
MACOS_SYSTEM_LIBRARIES = ("/usr/lib/", "/System/Library/")


def macos_input_file():
    return input_path(MACOS_TARGET)


def checked_macos_inputs(value):
    """The committed macOS producer selection. A pending or unbound record refuses every step."""
    require(isinstance(value, dict) and value.get("schema_version") == 2
            and value.get("kind") == MACOS_INPUTS_KIND and value.get("target") == MACOS_TARGET,
            "not the macOS producer input selection")
    require(set(value) == {"schema_version", "kind", "target", "rust_toolchain", "features", "source",
                           "minimum_macos", "minimum_free_bytes", "sdk_build_tools", "node_prebuilt_policy",
                           "library"}, "exact macOS producer input fields required")
    library = value["library"]
    require(isinstance(library, dict) and set(library) == {"state", "pins", "reason"}
            and isinstance(library["pins"], dict) and set(library["pins"]) == {"path", "sha256"},
            "exact macOS library selection fields required")
    require(library["state"] == "selected" and not unbound({key: item for key, item in value.items()}),
            "unbound macOS producer inputs: the library selection is pending")
    source = value["source"]
    require(isinstance(source, dict) and set(source) == {"repository", "export_commit"}
            and source["repository"] == PUBLICATION_REPO and SHA.fullmatch(source["export_commit"]),
            "exact source repository and connect/ export commit required")
    require(isinstance(value["rust_toolchain"], str) and VERSION.fullmatch(value["rust_toolchain"])
            and isinstance(value["features"], str) and value["features"]
            and re.fullmatch(r"[0-9]+\.[0-9]+", value["minimum_macos"])
            and type(value["minimum_free_bytes"]) is int and value["minimum_free_bytes"] > 0,
            "invalid macOS build selection")
    tools = value["sdk_build_tools"]
    require(isinstance(tools, dict) and 0 < len(tools) <= 16
            and all(re.fullmatch(r"[a-z0-9_]+", name) and isinstance(version, str)
                    and re.fullmatch(r"[0-9][0-9A-Za-z.]*", version) for name, version in tools.items()),
            "exact SDK build tool selection required")
    for row in (value["node_prebuilt_policy"], library["pins"]):
        require(isinstance(row, dict) and set(row) == {"path", "sha256"} and HEX.fullmatch(row["sha256"]),
                "invalid committed file pin")
        require(checked_file(ROOT / relative(row["path"]), MAX_METADATA)["sha256"] == row["sha256"],
                "committed file differs from the macOS producer selection: " + row["path"])
    return value


def macos_work(work, *, create=False):
    require(isinstance(work, Path) and work.is_absolute() and str(work) == os.path.normpath(str(work)),
            "absolute normal work root required")
    if create and not work.exists():
        require(work.parent.is_dir(), "the work root's parent must exist")
        work.mkdir()
    require(work.is_dir() and not work.is_symlink() and work.resolve(strict=True) == work,
            "canonical existing work root required")
    require(not work.is_relative_to(WORKSPACE), "the work root must be outside the checkout")
    return work


def macos_git(*args):
    return subprocess.check_output(["git", "-C", str(WORKSPACE), *args], text=True).strip()


def macos_checkout():
    """This checkout's commit and tree; it must be clean (every step, before and after)."""
    require(macos_git("status", "--porcelain", "--untracked-files=normal") == "", "source checkout is dirty")
    return macos_git("rev-parse", "--verify", "HEAD^{commit}"), macos_git("rev-parse", "--verify", "HEAD^{tree}")


def step_path(work, step):
    return work / "records" / (step + ".step.json")


class StepRecorder(Recorder):
    """One macOS step's recorder: the steps share <work>/records, and every command name stays unique."""

    def __init__(self, work, step):
        self.work = work
        self.step = step
        self.records = work / "records"
        self.records.mkdir(exist_ok=True)
        self.commands = []

    def run(self, name, argv, **kwargs):
        self.commands.append(name)
        return super().run(name, argv, **kwargs)


def macos_environment(work, **extra):
    """The complete environment of a child that is not Cargo. No directory is named "home"."""
    for leaf in ("hm", "tmp"):
        (work / leaf).mkdir(exist_ok=True)
    return {"HOME": str(work / "hm"), "TMPDIR": str(work / "tmp"), "PATH": "/usr/bin:/bin",
            "LC_ALL": "en_US.UTF-8", "PIP_CONFIG_FILE": "/dev/null", "PIP_DISABLE_PIP_VERSION_CHECK": "1",
            "PIP_NO_CACHE_DIR": "1", "PYTHONDONTWRITEBYTECODE": "1", **extra}


def macos_cargo_environment(**extra):
    # Cargo inherits the job's toolchain environment; the lane name keeps one target directory per producer.
    return {"RUST_LANE_ID": MACOS_LANE, "CARGO_TERM_COLOR": "never", **extra}


def macos_begin(work, step):
    """Start one step: its earlier steps ran, it did not, and the checkout is still the recorded commit."""
    require(step in MACOS_STEPS and step != "preflight", "unknown macOS producer step")
    macos_work(work)
    require(not step_path(work, step).exists(), "this step already ran in this work root: " + step)
    for earlier in MACOS_REQUIRES[step]:
        require(step_path(work, earlier).is_file(), "an earlier step has no record: " + earlier)
    preflight = read_json(step_path(work, "preflight"))
    selection = work / "inputs-selection.json"
    require(binding(selection) == preflight["inputs"] and digest(macos_input_file()) == preflight["inputs"]["sha256"],
            "the macOS producer selection changed after preflight")
    inputs = checked_macos_inputs(read_json(selection))
    require(macos_checkout() == (preflight["revision"], preflight["tree"])
            and str(WORKSPACE) == preflight["checkout_root"] and str(work) == preflight["work_root"],
            "the checkout or the work root is not the one preflight recorded")
    require(binding(work / "source-manifest.json") == preflight["source_manifest"], "the source manifest changed")
    return {"work": work, "step": step, "inputs": inputs, "preflight": preflight,
            "record": StepRecorder(work, step), "source_manifest": work / "source-manifest.json"}


def macos_finish(context, outputs):
    work, step = context["work"], context["step"]
    require(macos_checkout() == (context["preflight"]["revision"], context["preflight"]["tree"]),
            "the checkout changed during the step")
    write_new(step_path(work, step), {"schema_version": 1, "kind": "connect-macos-step", "step": step,
        "revision": context["preflight"]["revision"], "commands": context["record"].commands,
        "outputs": outputs})
    return outputs


def macos_free_space(context):
    free = shutil.disk_usage(context["work"]).free
    require(free >= context["inputs"]["minimum_free_bytes"],
            f"free space {free} B is below the selected minimum {context['inputs']['minimum_free_bytes']} B")
    return free


def macho_facts(path):
    """Facts of a thin 64-bit Mach-O file, read from its header and load commands. Runs nothing."""
    with path.open("rb") as stream:
        header = stream.read(32)
        require(len(header) == 32, "not a Mach-O file: " + path.name)
        magic, cpu, _subtype, filetype, count, size, _flags, _reserved = struct.unpack("<8I", header)
        require(magic == MACHO_MAGIC_64 and 0 < count <= 4096 and 0 < size <= 4 * 1024**2,
                "thin 64-bit Mach-O file required: " + path.name)
        commands = stream.read(size)
        require(len(commands) == size, "truncated Mach-O load commands: " + path.name)
        facts = {"cpu": cpu, "filetype": filetype, "minimum_os": None, "sdk": None, "platform": None,
                 "libraries": [], "code_signature": None}
        offset = 0
        for _ in range(count):
            require(offset + 8 <= size, "invalid Mach-O load command: " + path.name)
            command, length = struct.unpack_from("<II", commands, offset)
            require(length >= 8 and offset + length <= size, "invalid Mach-O load command: " + path.name)
            if command == LC_BUILD_VERSION:
                platform_id, minimum, sdk = struct.unpack_from("<III", commands, offset + 8)
                facts.update(platform=platform_id, minimum_os=macho_version(minimum), sdk=macho_version(sdk))
            elif command == LC_VERSION_MIN_MACOSX:
                minimum, sdk = struct.unpack_from("<II", commands, offset + 8)
                facts.update(platform=1, minimum_os=macho_version(minimum), sdk=macho_version(sdk))
            elif command in LC_DYLIBS:
                name = struct.unpack_from("<I", commands, offset + 8)[0]
                require(24 <= name < length, "invalid Mach-O library name: " + path.name)
                raw = commands[offset + name:offset + length].split(b"\0", 1)[0]
                facts["libraries"].append(raw.decode("utf-8"))
            elif command == LC_CODE_SIGNATURE:
                where, extent = struct.unpack_from("<II", commands, offset + 8)
                require(8 <= extent <= 64 * 1024**2, "invalid Mach-O code signature: " + path.name)
                stream.seek(where)
                blob = stream.read(extent)
                require(len(blob) == extent, "truncated Mach-O code signature: " + path.name)
                facts["code_signature"] = macho_signature(blob, path.name)
            offset += length
    return facts


def macho_version(value):
    major, minor, patch = value >> 16, (value >> 8) & 0xFF, value & 0xFF
    return f"{major}.{minor}" + (f".{patch}" if patch else "")


def macho_signature(blob, name):
    magic, _length, count = struct.unpack_from(">III", blob, 0)
    require(magic == CS_SUPERBLOB and 0 < count <= 64 and 12 + 8 * count <= len(blob),
            "unsupported code signature: " + name)
    for index in range(count):
        _kind, where = struct.unpack_from(">II", blob, 12 + 8 * index)
        if where + 16 <= len(blob) and struct.unpack_from(">I", blob, where)[0] == CS_CODE_DIRECTORY:
            flags = struct.unpack_from(">I", blob, where + 12)[0]
            return {"flags": flags, "adhoc": bool(flags & CS_ADHOC), "linker_signed": bool(flags & CS_LINKER_SIGNED)}
    raise ValueError("code signature without a code directory: " + name)


def version_tuple(value):
    return tuple(int(part) for part in value.split("."))


def checked_macos_executable(path, minimum_macos):
    """A thin arm64 executable that links system libraries only and runs on the selected macOS."""
    facts = macho_facts(path)
    require(facts["cpu"] == MACHO_CPU_ARM64 and facts["filetype"] == MACHO_EXECUTE,
            "thin arm64 Mach-O executable required: " + path.name)
    outside = [name for name in facts["libraries"] if not name.startswith(MACOS_SYSTEM_LIBRARIES)]
    require(not outside, path.name + " links a library a user's Mac does not have: " + ", ".join(outside[:8]))
    require(facts["platform"] == 1 and facts["minimum_os"] is not None
            and version_tuple(facts["minimum_os"]) <= version_tuple(minimum_macos),
            f"{path.name} needs macOS {facts['minimum_os']}, above the selected {minimum_macos}")
    return facts


def signature_path(path):
    return path.with_name(path.name + ".signature.json")


def macos_sign(record, path, label):
    """Ad-hoc sign one executable in place, right after it was copied out of Cargo's output.

    Signing changes the file's bytes, so it happens before any record binds a digest. No identity is used.
    """
    require(path.is_file() and not path.is_symlink() and not signature_path(path).exists(),
            "one fresh unsigned executable required: " + path.name)
    unsigned = {"sha256": digest(path), "size": path.stat().st_size}
    record.run("sign-" + label, [CODESIGN, "--force", "--sign", "-", path], seconds=600)
    record.run("sign-verify-" + label, [CODESIGN, "--verify", "--strict", path], seconds=600)
    observed = macho_facts(path)["code_signature"]
    require(observed is not None and observed["adhoc"] and not observed["linker_signed"],
            "the executable does not carry codesign's ad-hoc signature: " + path.name)
    write_new(signature_path(path), {"schema_version": 1, "kind": "connect-macos-adhoc-signature",
        "identity": "-", "unsigned": unsigned, "signed": binding(path), "code_directory": observed,
        "sign": binding(record.records / ("sign-" + label + ".json")),
        "verify": binding(record.records / ("sign-verify-" + label + ".json"))})
    return signature_path(path)


def signed_binding(path):
    """The only way an executable's digest enters a macOS record: the bytes that were signed, unchanged."""
    signature = signature_path(path)
    require(signature.is_file() and not signature.is_symlink(),
            "no ad-hoc signature record: an executable is signed before a record binds its digest: " + path.name)
    current = binding(path)
    require(read_json(signature)["signed"] == current, "executable changed after signing: " + path.name)
    return current


def macos_emit(context, log, name, destination, *, test, label):
    """Copy the one executable Cargo emitted, make it executable, sign it. Returns its signed binding."""
    destination.parent.mkdir(exist_ok=True)
    require(not destination.exists(), "fresh executable destination required: " + destination.name)
    copy_exact(emitted_executable(log, name, test=test), destination)
    destination.chmod(0o755)
    macos_sign(context["record"], destination, label)
    return signed_binding(destination)


def pin_rows(pins_path, pinned_inputs):
    """The committed pin file and the directory the fetch step filled; both are read, never written."""
    require(pins_path is not None and pinned_inputs is not None, "--pins and --pinned-inputs required")
    require(pins_path.is_absolute() and pinned_inputs.is_absolute() and pinned_inputs.is_dir()
            and not pinned_inputs.is_symlink(), "absolute pin file and pinned input directory required")
    value = read_json(pins_path)
    require(isinstance(value, dict) and value.get("schema") == MACOS_PINS_SCHEMA
            and value.get("target") == MACOS_TARGET and isinstance(value.get("pins"), list),
            "not the macOS library pin file")
    return value["pins"]


def checked_pin(row, pinned_inputs):
    """One pinned public file, checked against its pin again before use."""
    require(isinstance(row.get("file"), str) and "/" not in row["file"] and isinstance(row.get("kind"), str),
            "invalid pin row")
    path = pinned_inputs / relative(row["kind"]) / relative(row["file"])
    require(path.is_file() and not path.is_symlink(), "pinned input is missing: " + row["file"])
    if row.get("sha256") is not None:
        require(HEX.fullmatch(row["sha256"]) and digest(path) == row["sha256"], "pinned input differs: " + row["file"])
    else:
        with path.open("rb") as stream:
            observed = hashlib.file_digest(stream, "sha512").hexdigest()
        require(isinstance(row.get("sha512"), str) and observed == row["sha512"], "pinned input differs: " + row["file"])
    return path


def pinned_file(rows, pinned_inputs, kind, name, version):
    """The one pin of an index name and version."""
    found = [row for row in rows if isinstance(row, dict) and row.get("kind") == kind
             and str(row.get("name", "")).lower().replace("-", "_") == name and row.get("version") == version]
    require(len(found) == 1, f"exactly one pin required for {kind} {name} {version}")
    return checked_pin(found[0], pinned_inputs)


def pinned_node(rows, pinned_inputs, destination):
    """The pinned upstream Node for this host, unpacked for the component build. Returns node and npm's CLI."""
    found = [row for row in rows if isinstance(row, dict) and row.get("kind") == "release-download"
             and re.fullmatch(r"node-v[0-9]+\.[0-9]+\.[0-9]+-darwin-arm64\.tar\.gz", str(row.get("file")))]
    require(len(found) == 1, "exactly one pinned Node archive for darwin arm64 required")
    archive = checked_pin(found[0], pinned_inputs)
    prefix = archive.name.removesuffix(".tar.gz")
    destination.mkdir()
    with tarfile.open(archive, "r:gz") as source:
        for member in source:
            require(member.name == prefix or member.name.startswith(prefix + "/"), "Node archive member outside its root")
        source.extractall(destination, filter="tar")
    node, npm = destination / prefix / "bin/node", destination / prefix / "lib/node_modules/npm/bin/npm-cli.js"
    require(node.is_file() and npm.is_file(), "the pinned Node archive lacks node or npm")
    return node, npm


def wheel_metadata(path):
    """Name, version and requirements of one wheel, from its own METADATA."""
    with zipfile.ZipFile(path) as wheel:
        names = [name for name in wheel.namelist() if re.fullmatch(r"[^/]+\.dist-info/METADATA", name)]
        require(len(names) == 1, "one wheel metadata file required: " + path.name)
        metadata = email.parser.BytesParser().parsebytes(wheel.read(names[0]))
    return {"name": metadata["Name"], "version": metadata["Version"],
            "requires": metadata.get_all("Requires-Dist") or [], **binding(path)}


def macos_versions():
    """The versions of this tree: one commit supplies Connect, Core and Meta."""
    connect = tomllib.loads((ROOT / "crates/meerkat-connect/Cargo.toml").read_text())["package"]["version"]
    core = tomllib.loads((WORKSPACE / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    toolkit = tomllib.loads((ROOT / "sdk/python/pyproject.toml").read_text())["project"]
    meta = tomllib.loads((WORKSPACE / "meta/sdk/python/pyproject.toml").read_text())["project"]
    require(all(isinstance(item, str) and VERSION.fullmatch(item)
                for item in (connect, core, toolkit["version"], meta["version"])),
            "explicit versions required in the tree")
    require(toolkit["name"] == "meerkat-toolkit" and toolkit["version"] == connect
            and meta["name"] == "meerkat-meta", "Connect's crate and SDK versions differ")
    require("meerkat-meta==" + meta["version"] in toolkit.get("optional-dependencies", {}).get("native-host", []),
            "Connect's SDK must require this tree's Meta SDK for its native host")
    return {"connect": connect, "core": core, "toolkit_sdk": toolkit["version"], "meta_sdk": meta["version"]}


def macos_preflight(work, source_sha):
    """Step 1: the host, the checkout, the selection and the toolchain, before anything is built."""
    require_native_host(MACOS_TARGET)
    require(isinstance(source_sha, str) and SHA.fullmatch(source_sha), "--source-sha: full commit required")
    macos_work(work, create=True)
    require(len(str(work)) <= MACOS_WORK_ROOT_LIMIT, "work root too long for macOS Unix socket paths")
    require(not step_path(work, "preflight").exists() and not (work / "inputs-selection.json").exists(),
            "preflight already ran in this work root")
    inputs = checked_macos_inputs(read_json(macos_input_file()))
    revision, tree = macos_checkout()
    require(revision == source_sha, "the checkout is not the selected commit")
    versions = macos_versions()
    copy_exact(macos_input_file(), work / "inputs-selection.json")
    record = StepRecorder(work, "preflight")
    channel = tomllib.loads((WORKSPACE / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    require(channel == inputs["rust_toolchain"], "rust-toolchain.toml differs from the selected toolchain")
    _, rustc = record.run("rust-version", ["rustc", "--version", "--verbose"], cwd=WORKSPACE)
    facts = dict(line.split(": ", 1) for line in rustc.read_text().splitlines() if ": " in line)
    require(facts.get("release") == channel and facts.get("host") == MACOS_TRIPLE,
            "the active Rust toolchain is not the selected one for this host")
    record.run("cargo-version", ["cargo", "--version", "--verbose"], cwd=WORKSPACE)
    record.run("cargo-paths", [ROOT / "scripts/repo-cargo", "--print-env"], cwd=WORKSPACE,
               env=macos_cargo_environment())
    record.run("python-version", [sys.executable, "--version"])
    require(sys.version_info >= (3, 11) and hasattr(tarfile, "tar_filter"), "Python 3.11.4 or newer required")
    record.run("system", ["/usr/bin/uname", "-a"])
    record.run("system-version", ["/usr/bin/sw_vers"])
    record.run("system-sdk", ["/usr/bin/xcrun", "--show-sdk-path"])
    zstd = shutil.which("zstd")
    require(zstd is not None, "zstd is not on PATH")
    record.run("zstd-version", [zstd, "--version"])
    source_manifest = work / "source-manifest.json"
    record.run("source-record", [sys.executable, "-I", "-B", ROOT / "scripts/connect-source-identity.py", "record",
        "--commit", revision, "--export-commit", inputs["source"]["export_commit"], "--output", source_manifest],
        cwd=work, env=macos_environment(work), inherit=False, seconds=900)
    epoch = int(macos_git("log", "-1", "--format=%ct", "HEAD"))
    edition = time.strftime("%Y%m%d", time.gmtime(epoch)) + "-connect-" + revision[:8]
    value = {"schema_version": 1, "kind": "connect-macos-step", "step": "preflight", "target": MACOS_TARGET,
        "repository": PUBLICATION_REPO, "revision": revision, "tree": tree, "checkout_root": str(WORKSPACE),
        "work_root": str(work), "versions": versions, "rust_toolchain": channel, "source_date_epoch": epoch,
        "library_edition": edition, "inputs": binding(work / "inputs-selection.json"),
        "source_manifest": binding(source_manifest), "zstd": zstd, "commands": record.commands,
        "free_bytes": shutil.disk_usage(work).free}
    write_new(step_path(work, "preflight"), value)
    return value


def macos_tool_argv(name, feature):
    argv = [ROOT / "scripts/repo-cargo", "build", "--locked", "--release", "--manifest-path",
            WORKSPACE / "Cargo.toml", "-p", "toolkit-package"]
    if feature:
        argv += ["--features", feature]
    return [*argv, "--bin", name, "--message-format=json-render-diagnostics"]


def macos_owner_argv(context, pins, pinned_inputs, step, **values):
    """The other owners' library scripts, by command line only (their interface files own these lines)."""
    work, python = context["work"], [sys.executable, "-I", "-B"]
    pinned = ["--pins", pins, "--pinned-inputs", pinned_inputs]
    closure = work / "consumers-closure/dependency-closure-macos-arm64.json"
    if step == "library-components":
        return [*python, ROOT / "scripts/build-library-components.py", *pinned, "--python", sys.executable,
                "--node", values["node"], "--npm-cli", values["npm"],
                "--source-date-epoch", str(context["preflight"]["source_date_epoch"]),
                "--output", work / "components", "--verify-reproducible"]
    if step == "library-consumers":
        components = [item for label, kind, _file in MACOS_COMPONENTS for item in (
            f"--{label}-{kind}", values["components"][label]["path"],
            f"--{label}-sha256", values["components"][label]["sha256"])]
        return [*python, ROOT / "scripts/export-library-consumers.py", "--target", CONSUMER_EXPORT_TARGET,
                *pinned, *components, "--closure-record", closure, "--output", work / "consumers"]
    if step == "library-kit":
        return [*python, ROOT / "scripts/package-agent-install-kit.py", "--target", MACOS_TARGET, *pinned,
                "--tools", work / "tools", "--tool-dependencies", work / "tools/dependencies.txt",
                "--rust-license", values["rust_license"], "--output", work / "kit"]
    require(step == "library-package", "unknown library owner step")
    kit = work / "kit" / ("agent-install-kit-" + MACOS_TARGET + ".tar.gz")
    return [*python, ROOT / "scripts/package-library-release.py", "--target", MACOS_TARGET,
            "--edition", context["preflight"]["library_edition"], *pinned, "--consumers", work / "consumers",
            "--consumers-contents-sha256", digest(work / "consumers/CONTENTS.json"),
            "--consumer-closure", closure, "--kit", kit, "--kit-sha256", digest(kit),
            "--output", work / "library"]


def macos_library_archive(context):
    name = "toolkit-library-" + context["preflight"]["library_edition"] + "-" + MACOS_TARGET + ".tar.gz"
    directory = context["work"] / "library"
    found = sorted(path.name for path in directory.glob("toolkit-library-*.tar.gz"))
    require(found == [name], "exactly this commit's one macOS library archive required")
    return directory / name


def macos_library(work, pins, pinned_inputs):
    """Step 3: the six library tools from this checkout (signed, recorded), then the library itself."""
    context = macos_begin(work, "library")
    pin_rows(pins, pinned_inputs)
    record, preflight = context["record"], context["preflight"]
    free = macos_free_space(context)
    tools = work / "tools"
    tools.mkdir()
    outputs = {"free_bytes": free, "tools": {}}
    for name, feature in MACOS_TOOLS:
        cargo, log = record.run("tool-" + name, macos_tool_argv(name, feature), cwd=WORKSPACE,
                                env=macos_cargo_environment(), seconds=MACOS_BUILD_SECONDS)
        bound = macos_emit(context, log, name, tools / name, test=False, label="tool-" + name)
        checked_macos_executable(tools / name, context["inputs"]["minimum_macos"])
        build = tools / (name + ".build.json")
        write_new(build, {"schema_version": 1, "binary": "tools/" + name, "sha256": bound["sha256"],
            "bytes": bound["size"], "mode": "0755", "target": MACOS_TRIPLE, "command": cargo["argv"],
            "features": [feature] if feature else [],
            "source": {"repository": PUBLICATION_REPO, "revision": preflight["revision"], "tree": preflight["tree"],
                       "source_manifest_sha256": preflight["source_manifest"]["sha256"],
                       "lock_sha256": digest(WORKSPACE / "Cargo.lock")},
            "signature": binding(signature_path(tools / name)),
            "build": {"record": binding(record.records / ("tool-" + name + ".json")), "messages": binding(log)},
            "scope": "one tool of the integration library built from this commit; not native or business execution"})
        outputs["tools"][name] = binding(build)
    # The crates the tools were built from, for the kit's archives and licence texts. It builds nothing.
    _, listing = record.run("tool-dependencies", [ROOT / "scripts/repo-cargo", "tree", "--locked",
        "--manifest-path", WORKSPACE / "Cargo.toml", "-p", "toolkit-package",
        "--features", "local-recipes,local-artifacts", "--target", MACOS_TRIPLE, "-e", "normal,build",
        "--prefix", "none", "--format", "{p}"], cwd=WORKSPACE, env=macos_cargo_environment(), seconds=1800)
    lines = [line for line in listing.read_text().splitlines() if PACKAGE_LINE.fullmatch(line)]
    require(lines, "the tool dependency listing is empty")
    (tools / "dependencies.txt").write_text("\n".join(lines) + "\n")
    _, sysroot = record.run("rust-sysroot", ["rustc", "--print", "sysroot"], cwd=WORKSPACE)
    rust_license = Path(sysroot.read_text().strip()) / "share/doc/rust/COPYRIGHT-library.html"
    environment = macos_environment(work)

    def owner(step, **values):
        record.run(step, macos_owner_argv(context, pins, pinned_inputs, step, **values), cwd=work,
                   env=environment, inherit=False, seconds=3600)

    node, npm = pinned_node(pin_rows(pins, pinned_inputs), pinned_inputs, work / "node")
    owner("library-components", node=node, npm=npm)
    built = read_json(work / "components/components.json")
    require(built.get("schema") == "connect-library-components/1" and isinstance(built.get("components"), dict),
            "not the component build's record")
    components = {}
    for label, _kind, _file in MACOS_COMPONENTS:
        row = built["components"][label]
        path = work / "components" / relative(row["file"])
        require("/" not in row["file"] and path.is_file() and not path.is_symlink()
                and digest(path) == row["sha256"], "a built component differs from its record: " + label)
        components[label] = binding(path)
    (work / "consumers-closure").mkdir()
    owner("library-consumers", components=components)
    owner("library-kit", rust_license=rust_license)
    owner("library-package")
    archive = macos_library_archive(context)
    read_json(work / "library/build.json")
    for name, _feature in MACOS_TOOLS:
        signed_binding(tools / name)
    outputs.update(library=binding(archive), library_build=binding(work / "library/build.json"),
                   components=components, components_reproducible=built.get("reproducible"),
                   tool_dependencies=binding(tools / "dependencies.txt"))
    return macos_finish(context, outputs)


def macos_resources(work, pins, pinned_inputs):
    """Step 4: the starter for exactly this library, then the resource closure the executables bind."""
    context = macos_begin(work, "resources")
    pin_rows(pins, pinned_inputs)
    record, environment = context["record"], macos_environment(work)
    archive = macos_library_archive(context)
    require(binding(archive) == read_json(step_path(work, "library"))["outputs"]["library"],
            "the library changed after the library step")
    selection = work / "library-selection.json"
    write_new(selection, {"schema_version": 1, "status": "candidate_input_not_published",
        "assets": [{"upload_name": archive.name, "bytes": archive.stat().st_size, "sha256": digest(archive)}]})
    starter = work / "starter"
    record.run("starter-export", [sys.executable, "-I", "-B", ROOT / "scripts/export-setup-starter.py",
        "--destination", starter, "--manifest", selection, "--manifest-sha256", digest(selection)],
        cwd=work, env=environment, inherit=False, seconds=600)
    policy = ROOT / relative(context["inputs"]["node_prebuilt_policy"]["path"])
    record.run("resources", [sys.executable, "-I", "-B", ROOT / "scripts/package-connect-resources.py",
        "--destination", work / "resources", "--zstd", context["preflight"]["zstd"], "--target", MACOS_TARGET,
        "--pins", pins, "--pinned-inputs", pinned_inputs,
        "--library", archive, "--library-sha256", digest(archive), "--library-bytes", str(archive.stat().st_size),
        "--starter", starter, "--release-manifest-sha256", digest(starter / "release-manifest.json"),
        "--node-prebuilt-policy", policy,
        "--node-prebuilt-policy-sha256", context["inputs"]["node_prebuilt_policy"]["sha256"]],
        cwd=work, env=environment, inherit=False, seconds=3600)
    built = macos_resource_record(work)
    print("resources_sha256 " + built["resources_sha256"])
    return macos_finish(context, {"resources_sha256": built["resources_sha256"],
        "resource_build_record": binding(work / "resources/build.json"),
        "resource_archive": binding(macos_resource_archive(work)), "library_selection": binding(selection)})


def macos_resource_archive(work):
    return work / "resources" / ("meerkat-connect-resources-" + MACOS_TARGET + ".tar.gz")


def macos_resource_record(work):
    value = read_json(work / "resources/build.json")
    archive = macos_resource_archive(work)
    require(value.get("target") == MACOS_TARGET and isinstance(value.get("resources_sha256"), str)
            and HEX.fullmatch(value["resources_sha256"]) and archive.is_file()
            and value.get("archive_sha256") == digest(archive), "not this target's resource build record")
    return value


def macos_resource_digest(context):
    """The digest every native build, test build and assembly of this work root is bound to."""
    built = macos_resource_record(context["work"])
    recorded = read_json(step_path(context["work"], "resources"))["outputs"]
    require(recorded["resource_build_record"] == binding(context["work"] / "resources/build.json")
            and recorded["resources_sha256"] == built["resources_sha256"],
            "the resource closure changed after the resources step")
    return built["resources_sha256"]


def macos_native_argv(context, *selection):
    return [ROOT / "scripts/repo-cargo", selection[0], "--locked", "--release", "--manifest-path",
            WORKSPACE / "Cargo.toml", "-p", "meerkat-connect", "--features", context["inputs"]["features"],
            *selection[1:], "--message-format=json-render-diagnostics"]


def macos_source_check(context, label):
    manifest = context["source_manifest"]
    context["record"].run(label, [sys.executable, "-I", "-B", ROOT / "scripts/connect-source-identity.py", "check",
        "--manifest", manifest, "--manifest-sha256", digest(manifest)], cwd=context["work"],
        env=macos_environment(context["work"]), inherit=False, seconds=900)


def macos_build(work):
    """Step 5: both executables in one Cargo build, signed before any record binds them."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "build")
    record, inputs = context["record"], context["inputs"]
    resources_sha = macos_resource_digest(context)
    macos_source_check(context, "identity-check-before-build")
    free = macos_free_space(context)
    cargo, log = record.run("native-build", macos_native_argv(context, "build",
        "--bin", "toolkit_gateway", "--bin", "meerkat-connect"), cwd=WORKSPACE,
        env=macos_cargo_environment(MEERKAT_CONNECT_RESOURCES_SHA256=resources_sha), seconds=MACOS_BUILD_SECONDS)
    outputs = {"free_bytes": free, "resources_sha256": resources_sha}
    version = context["preflight"]["versions"]["connect"]
    for name in MACOS_SHIPPED:
        # Order: copy, sign, verify, facts, first run. Only then may a record name the digest.
        binary = work / "binary" / name
        bound = macos_emit(context, log, name, binary, test=False, label=name)
        facts = checked_macos_executable(binary, inputs["minimum_macos"])
        write_new(binary.with_name(name + ".facts.json"), {"schema_version": 1, "binary": bound, **facts})
        _, said = record.run("version-" + name, [binary, "--version"], cwd=work,
                             env=macos_environment(work), inherit=False, seconds=120)
        require(said.read_text().startswith(name + " " + version), name + " does not report the tree's version")
        outputs[name] = signed_binding(binary)
    macos_source_check(context, "identity-check-after-build")
    gateway = work / "records/gateway-build-record.json"
    write_new(gateway, macos_build_record(context, "toolkit_gateway", cargo, resources_sha))
    outputs["gateway_build_record"] = binding(gateway)
    return macos_finish(context, outputs)


def macos_build_record(context, kind, cargo, resources_sha, **extra):
    """The assembler's schema 1 native build record. The binary is bound through its signature record."""
    work = context["work"]
    binary = work / "binary" / kind
    return {"schema_version": 1, "target": MACOS_TARGET, "binary_kind": kind, "resources_sha256": resources_sha,
            "binary": signed_binding(binary), "signature": binding(signature_path(binary)), "cargo": cargo,
            "source_manifest": binding(context["source_manifest"]),
            "resource_build_record_sha256": digest(work / "resources/build.json"), **extra}


def macos_assembly_argv(context, kind, record_path):
    work = context["work"]
    binary, resources = work / "binary" / kind, work / "resources/build.json"
    return [sys.executable, "-I", "-B", ROOT / "scripts/assemble-connect-product.py", "--binary", binary,
            "--binary-sha256", signed_binding(binary)["sha256"], "--build-record", record_path,
            "--resource-build-record", resources, "--resource-build-record-sha256", digest(resources),
            "--binary-kind", kind, "--target", MACOS_TARGET,
            "--release-version", context["preflight"]["versions"]["connect"]]


def macos_sdk_wheels(context, pins, pinned_inputs, meta_wheel, meta_wheel_sha256):
    """Connect's SDK wheel from this checkout, and Meta's: from this checkout, or the supplied published one."""
    work, record, versions = context["work"], context["record"], context["preflight"]["versions"]
    rows = pin_rows(pins, pinned_inputs)
    environment = macos_environment(work, SOURCE_DATE_EPOCH=str(context["preflight"]["source_date_epoch"]))
    scratch = work / "sdk-build"
    wheelhouse = scratch / "tools"
    wheelhouse.mkdir(parents=True)
    lines = []
    for name, version in sorted(context["inputs"]["sdk_build_tools"].items()):
        source = pinned_file(rows, pinned_inputs, "pypi-wheel", name, version)
        copy_exact(source, wheelhouse / source.name)
        lines.append(f"{name}=={version} --hash=sha256:{digest(source)}")
    requirements = scratch / "requirements.txt"
    requirements.write_text("\n".join(lines) + "\n")
    venv = scratch / "venv"
    record.run("sdk-build-venv", [sys.executable, "-I", "-m", "venv", venv], cwd=work, env=environment, inherit=False)
    python = venv / "bin/python"
    record.run("sdk-build-tools", [python, "-I", "-m", "pip", "install", "--no-index", "--no-deps",
        "--require-hashes", "--find-links", wheelhouse, "-r", requirements], cwd=work, env=environment, inherit=False)
    record.run("sdk-build-inventory", [python, "-I", "-m", "pip", "list", "--format=json"], cwd=work,
               env=environment, inherit=False)
    (work / "sdk").mkdir()
    sources = {"toolkit": ("meerkat-toolkit", versions["toolkit_sdk"], ROOT / "sdk/python"),
               "meta": ("meerkat-meta", versions["meta_sdk"], WORKSPACE / "meta/sdk/python")}
    described = {}
    for label, (distribution, version, source) in sources.items():
        if label == "meta" and meta_wheel is not None:
            require(HEX.fullmatch(meta_wheel_sha256 or "") and checked_file(meta_wheel)["sha256"] == meta_wheel_sha256,
                    "the supplied Meta wheel differs from its stated digest")
            built, origin = meta_wheel, "supplied"
        else:
            # A scratch copy: the build backend writes build/ and egg-info, never into the checkout.
            shutil.copytree(source, scratch / label / "source", symlinks=True)
            (scratch / label / "dist").mkdir()
            record.run("sdk-wheel-" + label, [python, "-I", "-m", "build", "--wheel", "--no-isolation",
                "--outdir", scratch / label / "dist", scratch / label / "source"], cwd=scratch / label,
                env=environment, inherit=False, seconds=900)
            wheels = list((scratch / label / "dist").iterdir())
            require(len(wheels) == 1 and wheels[0].suffix == ".whl", "one built SDK wheel required: " + label)
            built, origin = wheels[0], "built_from_checkout"
        metadata = wheel_metadata(built)
        require(metadata["name"] == distribution and metadata["version"] == version,
                "SDK wheel is not this tree's " + distribution)
        copy_exact(built, work / "sdk" / built.name)
        described[label] = {"source": origin, "distribution": distribution, "version": version,
                            **binding(work / "sdk" / built.name)}
    extra = [item for item in wheel_metadata(Path(described["toolkit"]["path"]))["requires"]
             if re.match(r"meerkat-meta\s*==\s*" + re.escape(versions["meta_sdk"]) + r"\s*;", item)
             and "native-host" in item]
    require(len(extra) == 1, "Connect's SDK wheel must require this tree's Meta SDK for its native host")
    write_new(work / "sdk/SDK-WHEELS.json", {"schema_version": 1, **described})
    return described


def macos_stage(work, pins, pinned_inputs, meta_wheel=None, meta_wheel_sha256=None):
    """Step 6: both test executables in one Cargo build, the SDK wheels, and the two unqualified prefixes."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "stage")
    record = context["record"]
    resources_sha = macos_resource_digest(context)
    for name in MACOS_SHIPPED:
        # The production bytes were copied and signed before this test build can replace Cargo's output.
        signed_binding(work / "binary" / name)
    free = macos_free_space(context)
    tests = [item for name in MACOS_TESTS for item in ("--test", name)]
    cargo, log = record.run("test-build", macos_native_argv(context, "test", *tests, "--no-run"), cwd=WORKSPACE,
        env=macos_cargo_environment(MEERKAT_CONNECT_RESOURCES_SHA256=resources_sha), seconds=MACOS_BUILD_SECONDS)
    outputs = {"free_bytes": free}
    for name in MACOS_TESTS:
        outputs[name] = macos_emit(context, log, name, work / "binary" / name, test=True, label=name)
    native = read_json(record.records / "native-build.json")
    cli_record = work / "records/cli-build-record.json"
    write_new(cli_record, macos_build_record(context, "meerkat-connect", native, resources_sha,
        test_executable=signed_binding(work / "binary/packaged_setup"), test_build=cargo))
    outputs["sdk"] = macos_sdk_wheels(context, pins, pinned_inputs, meta_wheel, meta_wheel_sha256)
    environment = macos_environment(work)
    for kind, label, record_path in (("toolkit_gateway", "gateway", work / "records/gateway-build-record.json"),
                                     ("meerkat-connect", "cli", cli_record)):
        record.run(label + "-stage", [*macos_assembly_argv(context, kind, record_path), "--stage-only",
            "--destination", work / (label + "-stage")], cwd=work, env=environment, inherit=False, seconds=1800)
        outputs[label + "_staging"] = binding(work / (label + "-stage") / "staging.json")
    installed = work / "cli-stage" / MACOS_PREFIX / "bin/packaged_setup"
    copy_exact(work / "binary/packaged_setup", installed)
    installed.chmod(0o755)
    outputs.update(cli_build_record=binding(cli_record), installed_test=binding(installed))
    return macos_finish(context, outputs)


def macos_check_gateway(work):
    """Step 7: the installed gateway verifier against the staged gateway and exactly the two SDK wheels."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "check-gateway")
    record, environment = context["record"], macos_environment(work)
    wheels = read_json(work / "sdk/SDK-WHEELS.json")
    lines = []
    for label in ("meta", "toolkit"):
        row = wheels[label]
        require(binding(Path(row["path"])) == {key: row[key] for key in ("path", "size", "sha256")},
                "an SDK wheel changed after staging")
        lines.append(f"{row['distribution']}=={row['version']} --hash=sha256:{row['sha256']}")
    requirements = work / "verifier-requirements.txt"
    requirements.write_text("\n".join(lines) + "\n")
    venv = work / "verifier-venv"
    record.run("verifier-venv", [sys.executable, "-I", "-m", "venv", venv], cwd=work, env=environment, inherit=False)
    python = venv / "bin/python"
    record.run("verifier-install", [python, "-I", "-m", "pip", "install", "--no-index", "--no-deps",
        "--require-hashes", "--find-links", work / "sdk", "-r", requirements], cwd=work, env=environment, inherit=False)
    record.run("verifier-inventory", [python, "-I", "-m", "pip", "list", "--format=json"], cwd=work,
               env=environment, inherit=False)
    _, site_log = record.run("verifier-site", [python, "-I", "-c",
        "import sysconfig; print(sysconfig.get_path('purelib'))"], cwd=work, env=environment, inherit=False)
    site = Path(site_log.read_text().strip())
    staged = work / "gateway-stage/staging.json"
    record.run("gateway-verify", [python, "-I", "-B", ROOT / "scripts/verify_native_gateway_subprocess.py",
        "--toolkit-gateway", work / "gateway-stage" / MACOS_PREFIX / "bin/toolkit_gateway",
        "--toolkit-python", site, "--meta-python", site, "--case", "installed-product",
        "--installed-staging", staged, "--installed-staging-sha256", digest(staged),
        "--output", work / "gateway-verification"], cwd=work, env=environment, inherit=False, seconds=3600)
    manifest = work / "gateway-verification/manifest.json"
    require(read_json(manifest)["status"] == "passed", "the installed gateway verification did not pass")
    return macos_finish(context, {"status": "passed", "manifest": binding(manifest)})


def macos_installed_test(context, executable, name, directory, *, prefix=None, extra=None, path="", seconds=900):
    """Run one ignored installed test exactly once, outside the checkout, with a complete environment."""
    for leaf in ("hm", "tmp", "empty-path"):
        (directory / leaf).mkdir(parents=True)
    environment = {"HOME": str(directory / "hm"), "TMPDIR": str(directory / "tmp"),
                   "PATH": path or str(directory / "empty-path"), "LC_ALL": "en_US.UTF-8",
                   "RUST_TEST_THREADS": "1", "RUST_MIN_STACK": "8388608", **(extra or {})}
    record = context["record"]
    execution, log = record.run(name, [executable, "--exact", "--ignored", name, "--nocapture"],
                                cwd=directory, env=environment, inherit=False, seconds=seconds)
    # libtest exits successfully when a filter selects zero tests, too.
    require(re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", log.read_text(errors="replace")),
            "the selected installed test did not actually run once: " + name)
    evidence = [binding(record.records / (name + ".json")), binding(log)]
    if prefix is not None:
        roots = list((directory / "tmp").glob(prefix + "*"))
        require(len(roots) == 1 and (roots[0] / "native-dispatch.jsonl").is_file(),
                "owned installed test evidence is missing: " + name)
        evidence += [binding(item) for item in sorted(roots[0].iterdir()) if item.is_file() and not item.is_symlink()]
    return execution, evidence


def macos_check_cli(work):
    """Step 8: the two installed CLI tests. A passing shutdown test leaves its review pending."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "check-cli")
    staged = work / "cli-stage/staging.json"
    candidate = read_json(staged)["candidate"]
    installed = work / "cli-stage" / MACOS_PREFIX / "bin/packaged_setup"
    require(binding(installed) == read_json(step_path(work, "stage"))["outputs"]["installed_test"],
            "the installed test executable changed after staging")
    directory = work / "cli-qualification"
    directory.mkdir(exist_ok=True)
    execution, evidence = macos_installed_test(context, installed, CLI_COMPOSITION,
        directory / "composition-run", prefix="connect-product-acceptance-")
    composition = directory / "installed-composition.json"
    write_new(composition, {"schema_version": 1, "kind": "meerkat_connect_installed_composition",
        "scope": CLI_COMPOSITION, "status": "passed", "candidate": candidate,
        "test_executable": binding(installed), "execution": execution, "evidence": evidence})
    execution, evidence = macos_installed_test(context, installed, CLI_SHUTDOWN,
        directory / "shutdown-run", prefix="connect-active-worker-acceptance-")
    evidence += [binding(item) for item in (staged, work / "records/cli-build-record.json",
        work / "resources/build.json", context["source_manifest"], work / "records/native-build.json",
        work / "records/test-build.json", composition)]
    # Deliberately not accepted: an independent reader assesses these observations in stage two.
    pending = directory / "native-shutdown-review.pending.json"
    write_new(pending, {"schema_version": 1, "kind": "meerkat_connect_native_shutdown_review",
        "scope": "active_setup_worker_shutdown", "disposition": "pending", "candidate": candidate,
        "test_executable": binding(installed), "execution": execution, "evidence": evidence})
    return macos_finish(context, {"composition": binding(composition), "pending_review": binding(pending)})


def macos_rkat(context, archive, archive_sha256, from_checkout):
    """The ordinary Core executable of the consumer check: the published asset, or this commit's build."""
    work, record = context["work"], context["record"]
    require((archive is not None) != bool(from_checkout),
            "exactly one of --rkat-archive (with --rkat-archive-sha256) and --rkat-from-checkout required")
    destination = work / "rkat/rkat"
    destination.parent.mkdir()
    core = context["preflight"]["versions"]["core"]
    if from_checkout:
        _, log = record.run("rkat-build", [ROOT / "scripts/repo-cargo", "build", "--locked", "--release",
            "--manifest-path", WORKSPACE / "Cargo.toml", "-p", "rkat", "--bin", "rkat",
            "--message-format=json-render-diagnostics"], cwd=WORKSPACE, env=macos_cargo_environment(),
            seconds=MACOS_BUILD_SECONDS)
        copy_exact(emitted_executable(log, "rkat", test=False), destination)
        described = {"source": "built_from_checkout", "version": core,
                     "build": binding(record.records / "rkat-build.json")}
    else:
        expected = "rkat-" + core + "-" + MACOS_TRIPLE + ".tar.gz"
        require(archive.name == expected, "the Core archive of this tree's version is " + expected)
        require(HEX.fullmatch(archive_sha256 or "") and checked_file(archive)["sha256"] == archive_sha256,
                "the Core archive differs from its stated digest")
        with tarfile.open(archive, "r:gz") as source:
            members = source.getmembers()
            require(len(members) == 1 and members[0].name in ("rkat", "./rkat") and members[0].isfile()
                    and 0 < members[0].size <= MAX_FILE, "the Core archive must hold exactly the rkat executable")
            with source.extractfile(members[0]) as reader, destination.open("xb") as writer:
                shutil.copyfileobj(reader, writer)
        described = {"source": "published_archive", "version": core,
                     "archive": {"name": archive.name, "size": archive.stat().st_size, "sha256": archive_sha256}}
    destination.chmod(0o755)
    # Not a shipped file: only that this host can run it is required of it.
    facts = macho_facts(destination)
    require(facts["cpu"] == MACHO_CPU_ARM64 and facts["filetype"] == MACHO_EXECUTE,
            "thin arm64 Mach-O rkat executable required")
    return destination, {**described, "executable": binding(destination)}


def macos_check_consumer(work, archive=None, archive_sha256=None, from_checkout=False):
    """Step 9: setup, exit, then a fresh ordinary rkat in the same realm, against the staged product."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "check-consumer")
    rkat, described = macos_rkat(context, archive, archive_sha256, from_checkout)
    cli = signed_binding(work / "binary/meerkat-connect")
    prefix = work / "cli-stage" / MACOS_PREFIX
    require(digest(prefix / "bin/meerkat-connect") == cli["sha256"], "the staged CLI is not the signed executable")
    directory = work / "cli-qualification"
    directory.mkdir(exist_ok=True)
    execution, evidence = macos_installed_test(context, work / "binary/cli_paired_host", CLI_CONSUMER,
        directory / "consumer-run", path="/usr/bin:/bin", seconds=3600,
        extra={"CONNECT_REAL_LIBRARY_PREFIX": str(prefix), "CONNECT_SUPPLIED_CLI_SHA256": cli["sha256"],
               "CONNECT_FINAL_RKAT": str(rkat), "CONNECT_FINAL_RKAT_SHA256": digest(rkat)})
    consumer = directory / "consumer.json"
    write_new(consumer, {"schema_version": 1, "kind": "meerkat_connect_consumer_check", "scope": CLI_CONSUMER,
        "status": "passed", "rkat": described, "cli": cli, "test_executable": signed_binding(work / "binary/cli_paired_host"),
        "execution": execution, "evidence": evidence})
    return macos_finish(context, {"consumer": binding(consumer), "rkat_source": described["source"]})


def after_resource_step(transcript):
    """What the CLI printed after the line that announces its resource step ("" when nothing yet)."""
    if CONVERSATION_PREPARING not in transcript:
        return ""
    rest = transcript.split(CONVERSATION_PREPARING, 1)[1]
    return rest.split("\n", 1)[1].strip() if "\n" in rest else ""


def conversation_verdict(transcript, *, alive, quiet):
    """What one observed start of the setup conversation shows. Text only; nothing is inferred beyond it."""
    require(CONVERSATION_UNAVAILABLE not in transcript,
            "the staged CLI refused its installed resources")
    require(CONVERSATION_REALM in transcript and CONVERSATION_PREPARING in transcript,
            "the staged CLI did not reach its resource step")
    after = after_resource_step(transcript)
    require(alive and quiet and after, "the staged CLI did not print and then wait after its resource step")
    return after


def macos_conversation(argv, cwd, environment, *, seconds=900, quiet_seconds=20, close_seconds=60):
    """Start argv on a pseudo-terminal, read until it waits at a prompt, then close it. POSIX only."""
    import pty
    import select
    pid, master = pty.fork()
    if pid == 0:  # the child: a controlling terminal of its own, then the product
        try:
            os.chdir(cwd)
            os.execve(argv[0], argv, environment)
        finally:
            os._exit(127)
    raw, deadline, last, status = b"", time.monotonic() + seconds, time.monotonic(), None
    observed = {"timed_out": False, "closed_by": None}

    def reap():
        nonlocal status
        if status is None:
            done, code = os.waitpid(pid, os.WNOHANG)
            if done:
                status = code
        return status is not None

    def read_some(wait):
        nonlocal raw, last
        if select.select([master], [], [], wait)[0]:
            try:
                block = os.read(master, 65536)
            except OSError:
                block = b""
            if block and len(raw) < 1024**2:
                raw += block
                last = time.monotonic()
            return bool(block)
        return False

    try:
        while not reap():
            read_some(0.5)
            if (time.monotonic() - last >= quiet_seconds
                    and after_resource_step(raw.decode("utf-8", errors="replace"))):
                break
            if time.monotonic() >= deadline:
                observed["timed_out"] = True
                break
        alive_at_prompt = not reap()
        while not alive_at_prompt and read_some(0.2):
            pass  # a program that already ended: take what it still had to say
        quiet = time.monotonic() - last >= quiet_seconds
        at_prompt = len(raw)  # what follows is the program's answer to being closed, not its start
        # Close: Ctrl-C at the terminal first, then signals to the session. Each wait is bounded.
        for name, action in (("terminal_interrupt", lambda: os.write(master, b"\x03")),
                             ("sigint", lambda: os.killpg(pid, signal.SIGINT)),
                             ("sigterm", lambda: os.killpg(pid, signal.SIGTERM)),
                             ("sigkill", lambda: os.killpg(pid, signal.SIGKILL))):
            if reap():
                break
            try:
                action()
            except OSError:
                pass
            observed["closed_by"] = name
            limit = time.monotonic() + close_seconds
            while not reap() and time.monotonic() < limit:
                read_some(0.2)
        require(reap(), "the staged CLI could not be closed")
    finally:
        os.close(master)
    observed.update(alive_at_prompt=alive_at_prompt, quiet_at_prompt=quiet,
        exit_code=os.waitstatus_to_exitcode(status), transcript=raw[:at_prompt].decode("utf-8", errors="replace"),
        closing_output=raw[at_prompt:].decode("utf-8", errors="replace")[:65536])
    return observed


def macos_check_conversation(work):
    """Step 10: the staged CLI itself, on a terminal, gets past its resource step to a prompt."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "check-conversation")
    cli = work / "cli-stage" / MACOS_PREFIX / "bin/meerkat-connect"
    require(digest(cli) == signed_binding(work / "binary/meerkat-connect")["sha256"],
            "the staged CLI is not the signed executable")
    directory = work / "cli-qualification/conversation-run"
    for leaf in ("hm", "tmp", "empty-path", "state", "config", "context"):
        (directory / leaf).mkdir(parents=True)
    environment = {"HOME": str(directory / "hm"), "TMPDIR": str(directory / "tmp"),
                   "PATH": str(directory / "empty-path"), "LC_ALL": "en_US.UTF-8", "TERM": "xterm-256color"}
    argv = [str(cli), "--realm", "conversation-check", "--state-root", str(directory / "state"),
            "--context-root", str(directory / "context"), "--user-config-root", str(directory / "config")]
    observed = macos_conversation(argv, directory / "context", environment)
    transcript = directory / "transcript.txt"
    transcript.write_text(observed.pop("transcript"), encoding="utf-8")
    conversation = work / "cli-qualification/conversation-start.json"
    try:
        prompt = conversation_verdict(transcript.read_text(encoding="utf-8"),
                                      alive=observed["alive_at_prompt"], quiet=observed["quiet_at_prompt"])
        require(observed["closed_by"] in ("terminal_interrupt", "sigint", "sigterm"),
                "the staged CLI did not close on an interrupt or a termination request")
    except ValueError as refusal:
        write_new(conversation.with_name("conversation-start.refused.json"), {"schema_version": 1,
            "status": "refused", "reason": str(refusal), "argv": argv, "environment": environment,
            **observed, "transcript": binding(transcript)})
        raise
    write_new(conversation, {"schema_version": 1, "kind": "meerkat_connect_conversation_start",
        "scope": "staged_cli_on_a_terminal_passes_its_resource_step_then_prints_and_waits", "status": "passed",
        "cli": signed_binding(work / "binary/meerkat-connect"), "argv": argv, "environment": environment,
        **observed, "after_resource_step_bytes": len(prompt.encode()), "transcript": binding(transcript),
        "evidence": [binding(transcript)],
        "not_claimed": ["model choice", "sign-in", "the conversation itself", "terminal interaction beyond the first prompt"]})
    return macos_finish(context, {"conversation": binding(conversation)})


def macos_check_official_install(work, chrome=None):
    """The official Playwright install, through the library's own installer, with the closure's Node and npm.

    This is the command the product's setup backend runs for that route (its Python, its installer, its
    official bundle, `--node` and `--npm-cli` from the assembled closure). A failure fails the step. When the
    host has no Chrome executable to pin, the check is recorded as not run, never as passed.
    """
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "check-official-install")
    closure = work / "cli-stage" / MACOS_PREFIX / "libexec/meerkat-connect"
    manifest = read_json(closure / "resources.json")
    python, node, npm, archive = (closure / relative(manifest[key]) for key in ("python", "node", "npm_cli", "archive"))
    directory = work / "cli-qualification/official-run"
    for leaf in ("hm", "tmp", "process", "library"):
        (directory / leaf).mkdir(parents=True)
    result = work / "cli-qualification/official-playwright.json"
    chrome = Path(MACOS_CHROME) if chrome is None else chrome
    common = {"schema_version": 1, "kind": "meerkat_connect_official_install_check",
              "scope": "official_playwright_install_through_the_library_installer"}
    if not (chrome.is_absolute() and chrome.is_file() and not chrome.is_symlink() and os.access(chrome, os.X_OK)):
        reason = "no Chrome executable at " + str(chrome) + ": the installer pins the host's Chrome by digest"
        write_new(result, {**common, "status": "not_run", "reason": reason, "evidence": []})
        return macos_finish(context, {"status": "not_run", "reason": reason, "record": binding(result)})
    stem = archive.name.removesuffix(".tar.gz")
    with tarfile.open(archive, "r:gz") as source:
        for member in source:
            require(member.name == stem or member.name.startswith(stem + "/"), "library member outside its root")
        source.extractall(directory / "library", filter="tar")
    library = directory / "library" / stem
    installer, official = library / "setup/install-library-consumer.py", library / "official"
    configuration, profile = directory / "selected.json", directory / "playwright-host.json"
    write_new(configuration, PLAYWRIGHT_CONFIGURATION)
    write_new(profile, {"schema_version": 1, "chrome_executable": str(chrome), "chrome_sha256": digest(chrome),
                        "process_parent": str(directory / "process")})
    destination = directory / "install"
    argv = [python, "-I", "-B", installer, "--official-bundle", official,
            "--contents-sha256", digest(official / "CONTENTS.json"), "--component", "playwright",
            "--playwright-host-profile", profile, "--playwright-host-profile-sha256", digest(profile),
            "--node", node, "--npm-cli", npm, "--destination", destination,
            "--config", configuration, "--config-sha256", digest(configuration), "--server-name", PLAYWRIGHT_SERVER]
    environment = {"HOME": str(directory / "hm"), "TMPDIR": str(directory / "tmp"), "PATH": "/usr/bin:/bin",
                   "LC_ALL": "en_US.UTF-8"}
    record = context["record"]
    execution, log = record.run("official-playwright-install", argv, cwd=directory, env=environment,
                                inherit=False, seconds=1800)
    receipt = read_json(destination / "ready/installed.json")
    require(receipt.get("scope") == "observed_software_installation_only"
            and receipt.get("family") == "official-playwright" and (destination / "ready/mcp.json").is_file(),
            "the official Playwright install left no installation receipt")
    evidence = [binding(record.records / "official-playwright-install.json"), binding(log),
                binding(destination / "ready/installed.json"), binding(destination / "ready/mcp.json"),
                binding(configuration), binding(profile)]
    evidence += [binding(path) for path in sorted(destination.glob("*.stdout")) + sorted(destination.glob("*.stderr"))]
    write_new(result, {**common, "status": "passed", "execution": execution,
        "installer": binding(installer), "official_contents_sha256": digest(official / "CONTENTS.json"),
        "runtime": {"python": binding(python), "node": binding(node), "npm_cli": binding(npm)},
        "evidence": evidence,
        "not_claimed": ["a browser start", "Chrome compatibility", "the MCP endpoint serving", "any tool call"]})
    return macos_finish(context, {"status": "passed", "record": binding(result)})


def tar_tree(directory, destination):
    """An uncompressed tar of one tree: sorted, numeric owner 0, links kept as links, modes kept.

    Returns the names it left out: sockets and other special files a check may leave behind hold no bytes.
    """
    skipped = []
    with tarfile.open(destination, "x", format=tarfile.PAX_FORMAT) as archive:
        for path in [directory, *sorted(directory.rglob("*"))]:
            name = path.relative_to(directory.parent).as_posix()
            mode = path.lstat().st_mode
            if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode) or stat.S_ISLNK(mode)):
                skipped.append(name)
                continue
            info = archive.gettarinfo(str(path), arcname=name)
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            if info.isfile():
                with path.open("rb") as payload:
                    archive.addfile(info, payload)
            else:
                archive.addfile(info)
    return skipped


def record_hosts(paths):
    """Host names that a step's own logs and evidence name in URLs. Not a capture of traffic."""
    hosts = set()
    for path in paths:
        if path.is_file() and not path.is_symlink() and path.stat().st_size <= MAX_METADATA:
            text = path.read_bytes().decode("utf-8", errors="replace")
            hosts.update(match.lower() for match in re.findall(r"https?://([A-Za-z0-9.-]+\.[A-Za-z]{2,})", text))
    return sorted(hosts)


def evidence_files(work, record_path):
    """A record and every file under the work root that its evidence rows bind."""
    value = read_json(record_path)
    rows = list(value.get("evidence", []))
    rows += [value[key] for key in ("test_executable", "transcript") if isinstance(value.get(key), dict)]
    paths = [record_path]
    for row in rows:
        path = Path(row["path"])
        require(binding(path) == {key: row[key] for key in ("path", "size", "sha256")},
                "bound evidence changed: " + path.name)
        if path.is_relative_to(work):
            paths.append(path)
    return paths


def macos_product(directory, kind, version, binary_sha256):
    """One assembled product as the assembler left it: the archive, its checksum file and assembly.json."""
    assembly = read_json(directory / "assembly.json")
    expected = ("meerkat-connect-gateway-" if kind == "toolkit_gateway" else "meerkat-connect-") + version \
        + "-" + MACOS_TARGET + ".tar.gz"
    require(assembly.get("archive") == expected and assembly.get("release_version") == version
            and assembly.get("binary_sha256") == binary_sha256, "assembly differs from the selection: " + kind)
    archive = directory / expected
    checksum = archive.with_name(expected + ".sha256")
    require(checksum.read_text() == f"{digest(archive)}  {expected}\n", "product checksum file differs: " + kind)
    return {"archive": checked_file(archive), "checksum": checked_file(checksum, MAX_METADATA),
            "assembly": checked_file(directory / "assembly.json", MAX_METADATA)}


def macos_stage_record(work, output, consumer_not_run=None):
    """Step 11: the stage record. The gateway product is complete here; the CLI product waits for the
    independent shutdown review, which stays pending."""
    require_native_host(MACOS_TARGET)
    context = macos_begin(work, "stage-record")
    preflight = context["preflight"]
    require(output is not None and output.is_absolute() and not output.exists() and output.parent.is_dir(),
            "fresh absolute --output with an existing parent required")
    steps = {step: read_json(step_path(work, step)) for step in MACOS_STEPS[:-1] if step_path(work, step).is_file()}
    consumer_ran = "check-consumer" in steps
    require(consumer_ran != (consumer_not_run is not None),
            "the consumer check either ran, or --consumer-not-run states why it did not")
    require(consumer_ran or consumer_not_run.strip(), "a reason is required with --consumer-not-run")
    qualification = work / "cli-qualification"
    official = read_json(qualification / "official-playwright.json")
    gates = {"gateway_verification": {"status": steps["check-gateway"]["outputs"]["status"]},
             "official_playwright_install": {key: official[key] for key in ("status", "reason") if key in official},
             "cli_composition": {"status": read_json(qualification / "installed-composition.json")["status"]},
             "shutdown_review": {"status": read_json(qualification / "native-shutdown-review.pending.json")["disposition"]},
             "conversation_start": {"status": read_json(qualification / "conversation-start.json")["status"]},
             "consumer": ({"status": read_json(qualification / "consumer.json")["status"],
                           "rkat": read_json(qualification / "consumer.json")["rkat"]["source"]} if consumer_ran
                          else {"status": "not_run", "reason": consumer_not_run.strip()})}
    require(gates["shutdown_review"]["status"] == "pending", "stage one never accepts the shutdown review")
    require(gates["gateway_verification"]["status"] == "passed", "the gateway verification did not pass")
    # The gateway product needs nothing from the review: its final archive is assembled now, so the exact
    # bytes can be scanned from this stage record. Nothing here publishes it.
    verification = work / "gateway-verification/manifest.json"
    (work / "products").mkdir()
    context["record"].run("gateway-final", [*macos_assembly_argv(context, "toolkit_gateway",
        work / "records/gateway-build-record.json"), "--gateway-verification", verification,
        "--gateway-verification-sha256", digest(verification), "--destination", work / "products/gateway"],
        cwd=work, env=macos_environment(work), inherit=False, seconds=1800)
    executables = {name: signed_binding(work / "binary" / name) for name in (*MACOS_SHIPPED, *MACOS_TESTS)}
    gateway = macos_product(work / "products/gateway", "toolkit_gateway", preflight["versions"]["connect"],
                            executables["toolkit_gateway"]["sha256"])
    selected = [work / name for name in ("inputs-selection.json", "source-manifest.json", "library-selection.json",
                "library/build.json", "resources/build.json", "sdk/SDK-WHEELS.json", "components/components.json",
                "tools/dependencies.txt", "gateway-verification/manifest.json")]
    selected += [macos_library_archive(context), macos_resource_archive(work)]
    selected += sorted(path for path in (work / "products/gateway").iterdir() if path.is_file() and not path.is_symlink())
    selected += sorted(path for path in (work / "records").rglob("*") if path.is_file() and not path.is_symlink())
    selected += sorted(path for path in (work / "binary").iterdir() if path.is_file())
    selected += sorted(path for path in (work / "tools").iterdir() if path.name.endswith((".build.json", ".signature.json")))
    selected += sorted(path for path in (work / "sdk").iterdir() if path.suffix == ".whl")
    checks = {"check-cli": ["installed-composition.json", "native-shutdown-review.pending.json"],
              "check-consumer": ["consumer.json"] if consumer_ran else [],
              "check-conversation": ["conversation-start.json"],
              "check-official-install": ["official-playwright.json"]}
    by_step = {}
    for step, names in checks.items():
        by_step[step] = [path for name in names for path in evidence_files(work, qualification / name)]
        selected += by_step[step]
    by_step["check-gateway"] = [path for path in (work / "gateway-verification").rglob("*") if path.is_file()]
    by_step["library"] = [work / "components/components.json",
                          *sorted(path for path in (work / "components").glob("logs/*") if path.is_file())]
    for step in [step for step in by_step if step not in steps]:
        del by_step[step]
    for step in by_step:
        # Cargo's own logs name registry URLs of every crate; they are not what a step reached.
        by_step[step] = by_step[step] + [work / "records" / (name + ".log") for name in steps[step]["commands"]
                                         if step != "library" or name == "library-components"]
    output.mkdir()
    rows, seen = [], set()
    for path in selected:
        name = path.relative_to(work).as_posix()
        if name in seen:
            continue
        seen.add(name)
        require(path.is_file() and not path.is_symlink(), "a stage file is missing: " + name)
        copy_exact(path, output / name)
        rows.append({"path": name, "bytes": path.stat().st_size, "sha256": digest(path)})
    restore, skipped = {}, []
    for name in ("gateway-stage", "cli-stage", "gateway-verification"):
        skipped += tar_tree(work / name, output / (name + ".tar"))
        restore[name + ".tar"] = name
        rows.append({"path": name + ".tar", "bytes": (output / (name + ".tar")).stat().st_size,
                     "sha256": digest(output / (name + ".tar"))})
    stage = {"schema": MACOS_STAGE_SCHEMA, "target": MACOS_TARGET, "repository": PUBLICATION_REPO,
        "revision": preflight["revision"], "tree": preflight["tree"], "checkout_root": preflight["checkout_root"],
        "work_root": preflight["work_root"], "versions": preflight["versions"],
        "library_edition": preflight["library_edition"], "inputs_sha256": preflight["inputs"]["sha256"],
        "pins_sha256": context["inputs"]["library"]["pins"]["sha256"],
        "resources_sha256": steps["resources"]["outputs"]["resources_sha256"],
        "executables": executables, "gates": gates,
        "products": {"gateway": {"state": "assembled_not_scanned_not_published", "directory": "products/gateway",
                                 **gateway},
                     "cli": {"state": "awaits_shutdown_review"}},
        "network": {step: {"expected": MACOS_NETWORK[step], "hosts_named_in_records": record_hosts(paths),
                           "captured": False} for step, paths in sorted(by_step.items())},
        "steps": {step: {"commands": value["commands"]} for step, value in steps.items()},
        "restore": restore, "special_files_left_out": skipped,
        "files": sorted(rows, key=lambda row: row["path"])}
    write_new(output / "STAGE.json", stage)
    macos_finish(context, {"stage": binding(output / "STAGE.json"), "files": len(rows)})
    return stage


def checked_stage(directory):
    """A stage record directory: STAGE.json and exactly its files, each with its recorded bytes."""
    require(directory is not None and directory.is_absolute() and directory.is_dir(), "absolute --stage directory required")
    stage = read_json(directory / "STAGE.json")
    require(isinstance(stage, dict) and stage.get("schema") == MACOS_STAGE_SCHEMA
            and stage.get("target") == MACOS_TARGET and isinstance(stage.get("products"), dict)
            and set(stage["products"]) == {"gateway", "cli"}
            and stage["products"]["cli"] == {"state": "awaits_shutdown_review"}
            and isinstance(stage.get("files"), list) and 0 < len(stage["files"]) <= 32768
            and set(stage.get("gates", {})) == set(MACOS_GATES), "not a macOS stage record")
    for row in stage["files"]:
        path = directory / relative(row["path"])
        require(set(row) == {"path", "bytes", "sha256"} and path.is_file() and not path.is_symlink()
                and path.stat().st_size == row["bytes"] and digest(path) == row["sha256"],
                "a stage file is missing or changed: " + str(row.get("path")))
    return stage


def restore_stage(stage, directory, work):
    """Put every stage file at its place under the same absolute work root; existing bytes must be equal."""
    # The staged trees first: a file below gives its own bytes a second check, never a partial tree.
    for archive, name in sorted(stage["restore"].items()):
        require(archive == name + ".tar" and name in ("gateway-stage", "cli-stage", "gateway-verification"),
                "unexpected stage tree")
        if (work / name).exists():
            continue
        with tarfile.open(directory / archive, "r:") as source:
            for member in source:
                require(member.name == name or member.name.startswith(name + "/"), "stage tree member outside its root")
            source.extractall(work, filter="tar")
    for row in stage["files"]:
        if row["path"] in stage["restore"]:
            continue
        target = work / row["path"]
        if target.exists():
            require(target.is_file() and not target.is_symlink() and digest(target) == row["sha256"],
                    "the work root holds other bytes: " + row["path"])
        else:
            copy_exact(directory / row["path"], target)
    for name in (*MACOS_SHIPPED, *MACOS_TESTS):
        (work / "binary" / name).chmod(0o755)
    (work / "cli-stage" / MACOS_PREFIX / "bin/packaged_setup").chmod(0o755)


def macos_assemble(work, stage_directory, review, review_sha256, output):
    """Stage two: the CLI product, after the independent review accepted the shutdown record.

    No product is run and nothing is built or signed. It runs on macOS: the product tree's symbolic links
    carry modes there that the installed executables compare with their inventory.
    """
    require_native_host(MACOS_TARGET)
    stage = checked_stage(stage_directory)
    require(str(work) == stage["work_root"] and str(WORKSPACE) == stage["checkout_root"],
            "stage two needs the stage-one paths: work root " + stage["work_root"]
            + " and checkout " + stage["checkout_root"])
    macos_work(work, create=True)
    require(macos_checkout() == (stage["revision"], stage["tree"]), "the checkout is not the stage's commit")
    require(output is not None and output.is_absolute() and not output.exists() and output.parent.is_dir(),
            "fresh absolute --output with an existing parent required")
    gates = stage["gates"]
    refused = [name for name in MACOS_GATES if name != "shutdown_review" and gates[name].get("status") != "passed"]
    require(not refused, "stage gates did not pass: " + ", ".join(refused))
    require(gates["shutdown_review"].get("status") == "pending", "the stage's shutdown review must be pending")
    require(gates["consumer"].get("rkat") == "published_archive",
            "a release needs the consumer check against Core's published rkat")
    restore_stage(stage, stage_directory, work)
    require(binding(work / "inputs-selection.json")["sha256"] == stage["inputs_sha256"]
            == digest(macos_input_file()), "the macOS producer selection differs from the stage's")
    checked_macos_inputs(read_json(work / "inputs-selection.json"))
    executables = {name: signed_binding(work / "binary" / name) for name in MACOS_SHIPPED}
    require(all(executables[name] == stage["executables"][name] for name in MACOS_SHIPPED),
            "an executable differs from the stage's signed bytes")
    version = stage["versions"]["connect"]
    gateway = macos_product(work / "products/gateway", "toolkit_gateway", version,
                            executables["toolkit_gateway"]["sha256"])
    recorded = stage["products"]["gateway"]
    require(all(gateway[key] == recorded[key] for key in ("archive", "checksum", "assembly")),
            "the gateway product differs from the stage's")
    pending_path = work / "cli-qualification/native-shutdown-review.pending.json"
    require(HEX.fullmatch(review_sha256 or "") and review is not None
            and checked_file(review, MAX_METADATA)["sha256"] == review_sha256,
            "exact independent shutdown review required")
    pending, accepted = read_json(pending_path), read_json(review)
    require(accepted.get("disposition") == "accepted" and accepted.get("candidate") == pending["candidate"]
            and all(row in accepted.get("evidence", []) for row in pending["evidence"]),
            "the accepted review does not cover this producer's exact observations")
    output.mkdir()
    record = Recorder(output)
    preflight = read_json(step_path(work, "preflight"))
    context = {"work": work, "preflight": preflight, "source_manifest": work / "source-manifest.json"}
    require(binding(context["source_manifest"]) == preflight["source_manifest"], "the source manifest changed")
    qualified = output / "qualified-build-record.json"
    final = read_json(work / "records/cli-build-record.json")
    final.update(installed_composition=binding(work / "cli-qualification/installed-composition.json"),
                 native_shutdown_review=binding(review))
    write_new(qualified, final)
    record.run("cli-final", [*macos_assembly_argv(context, "meerkat-connect", qualified),
        "--destination", output / "cli"], cwd=output, env=macos_environment(output), inherit=False, seconds=1800)
    products = {"cli": macos_product(output / "cli", "meerkat-connect", version, executables["meerkat-connect"]["sha256"])}
    # The gateway product of the stage, beside the CLI product, so one directory holds what would be published.
    (output / "gateway").mkdir()
    for key in ("archive", "checksum", "assembly"):
        copy_exact(work / "products/gateway" / gateway[key]["name"], output / "gateway" / gateway[key]["name"])
    products["gateway"] = macos_product(output / "gateway", "toolkit_gateway", version,
                                        executables["toolkit_gateway"]["sha256"])
    require(products["gateway"] == gateway, "the gateway product changed while it was copied")
    value = {"schema": MACOS_ASSEMBLY_SCHEMA, "target": MACOS_TARGET, "revision": stage["revision"],
        "version": version, "stage_sha256": digest(stage_directory / "STAGE.json"),
        "shutdown_review_sha256": review_sha256, "products": products, "native_execution": False,
        "private_text_scan": "required_before_publication", "publication": "not_performed"}
    write_new(output / "ASSEMBLY.json", value)
    return value


SCAN_REPORT_TOKENS = {"status": ("clean", "refused"), "result": ("clean", "hits", "errors", "hits-and-errors")}
SCAN_REPORT_KEY = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\Z")


def checked_scan_report(report, assets):
    """A private-text scan report as it may leave the place where the private rules are.

    It may hold counts by class, file digests, the digest of the scanner's list, a schema name, a status
    and a tool version. It never holds matched text, token text or the list: any other text refuses.
    """
    require(isinstance(report, dict) and isinstance(report.get("payload"), list), "not a private-text scan report")
    expected = {(row["name"], row["size"], row["sha256"]) for row in assets}
    scanned = set()
    for row in report["payload"]:
        require(isinstance(row, dict) and set(row) == {"name", "bytes", "sha256"}
                and (row["name"], row["bytes"], row["sha256"]) in expected,
                "the scan report names a file that is not one of the product files")
        scanned.add(row["name"])
    missing = sorted(row["name"] for row in assets if row["name"] not in scanned)
    require(not missing, "the private-text scan did not cover: " + ", ".join(missing))

    def walk(value, key):
        if value is None or isinstance(value, (bool, int)):
            return
        if isinstance(value, dict):
            for name, item in value.items():
                require(SCAN_REPORT_KEY.match(name), "the scan report carries free text")
                walk(item, name)
        elif isinstance(value, list):
            for item in value:
                walk(item, key)
        else:
            require(isinstance(value, str) and (HEX.fullmatch(value)
                    or value in SCAN_REPORT_TOKENS.get(key, ())
                    or (key == "schema" and re.fullmatch(r"[a-z0-9-]+(?:/[a-z0-9-]+){0,3}", value))
                    or (key in ("version", "tool_version") and re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,3}", value))),
                    "the scan report carries free text")

    walk({key: value for key, value in report.items() if key != "payload"}, "")
    require(report.get("status") == "clean", "the private-text scan is not clean")
    return report


def macos_accept_review(stage_directory, output):
    """Record the independent reader's acceptance of the shutdown record of one exact stage.

    Running it is the act of acceptance, after the evidence was read. It checks that every observation the
    pending record binds is in the stage record with exactly its bytes, and writes the accepted record.
    """
    stage = checked_stage(stage_directory)
    require(output is not None and output.is_absolute() and not output.exists() and output.parent.is_dir(),
            "fresh absolute --output with an existing parent required")
    pending_path = stage_directory / "cli-qualification/native-shutdown-review.pending.json"
    pending = read_json(pending_path)
    require(pending.get("disposition") == "pending" and stage["gates"]["shutdown_review"] == {"status": "pending"}
            and isinstance(pending.get("evidence"), list) and pending["evidence"],
            "the stage holds no pending shutdown record")
    root = Path(stage["work_root"])
    for row in pending["evidence"]:
        path = stage_directory / relative(Path(row["path"]).relative_to(root).as_posix())
        require(path.is_file() and not path.is_symlink() and path.stat().st_size == row["size"]
                and digest(path) == row["sha256"],
                "an observation the record binds is missing or changed: " + path.name)
    write_new(output, {**pending, "disposition": "accepted", "pending_record_sha256": digest(pending_path),
                       "stage_sha256": digest(stage_directory / "STAGE.json")})
    print("accepted shutdown review sha256 " + digest(output))
    return digest(output)


def macos_release_check(assembly_directory, privacy_report, privacy_report_sha256):
    """The last check before publication: a clean private-text scan of exactly the two product archives.

    The scan runs where the private rules are. Its report holds counts and file digests only, so it can be
    handed to a hosted job; the rules never are.
    """
    require(assembly_directory is not None and assembly_directory.is_absolute() and assembly_directory.is_dir(),
            "absolute --assembly directory required")
    assembly = read_json(assembly_directory / "ASSEMBLY.json")
    require(isinstance(assembly, dict) and assembly.get("schema") == MACOS_ASSEMBLY_SCHEMA
            and assembly.get("target") == MACOS_TARGET and set(assembly.get("products", {})) == {"gateway", "cli"}
            and assembly.get("publication") == "not_performed", "not a macOS assembly record")
    publishable = assembly_directory / "PUBLISHABLE.json"
    require(not publishable.exists(), "this assembly was already checked")
    assets = []
    for label in ("cli", "gateway"):
        for key in ("archive", "checksum"):
            row = assembly["products"][label][key]
            require(checked_file(assembly_directory / label / relative(row["name"])) == row,
                    "a product file differs from the assembly record: " + row["name"])
            assets.append(row)
    require(HEX.fullmatch(privacy_report_sha256 or "") and privacy_report is not None
            and checked_file(privacy_report, MAX_METADATA)["sha256"] == privacy_report_sha256,
            "exact private-text scan report required")
    report = checked_scan_report(read_json(privacy_report), assets)
    value = {"schema": "connect-macos-publishable/1", "target": MACOS_TARGET, "revision": assembly["revision"],
        "version": assembly["version"], "assembly_sha256": digest(assembly_directory / "ASSEMBLY.json"),
        "privacy_report_sha256": privacy_report_sha256,
        "scanner_list_sha256": report.get("rules_sha256"), "assets": assets, "publication": "not_performed"}
    write_new(publishable, value)
    return value


def macos_main(arguments):
    parser = argparse.ArgumentParser(prog="connect-candidate.py macos",
        description="The complete macOS arm64 installation: the stage one steps, the stage two assembly and "
                    "the check before publication.")
    parser.add_argument("step", choices=(*MACOS_STEPS, "accept-review", "assemble", "release-check"))
    parser.add_argument("--work", type=Path)
    for name in ("pins", "pinned-inputs", "output", "stage", "review", "assembly", "privacy-report",
                 "rkat-archive", "meta-wheel", "chrome"):
        parser.add_argument("--" + name, type=Path)
    for name in ("source-sha", "review-sha256", "privacy-report-sha256", "rkat-archive-sha256",
                 "meta-wheel-sha256", "consumer-not-run"):
        parser.add_argument("--" + name)
    parser.add_argument("--rkat-from-checkout", action="store_true")
    args = parser.parse_args(arguments)
    os.umask(0o022)
    try:
        require(args.work is not None or args.step in ("accept-review", "release-check"), "--work required")
        if args.step == "accept-review":
            macos_accept_review(args.stage, args.output)
        elif args.step == "preflight":
            macos_preflight(args.work, args.source_sha)
        elif args.step == "library":
            macos_library(args.work, args.pins, args.pinned_inputs)
        elif args.step == "resources":
            macos_resources(args.work, args.pins, args.pinned_inputs)
        elif args.step == "build":
            macos_build(args.work)
        elif args.step == "stage":
            macos_stage(args.work, args.pins, args.pinned_inputs, args.meta_wheel, args.meta_wheel_sha256)
        elif args.step == "check-gateway":
            macos_check_gateway(args.work)
        elif args.step == "check-cli":
            macos_check_cli(args.work)
        elif args.step == "check-consumer":
            macos_check_consumer(args.work, args.rkat_archive, args.rkat_archive_sha256, args.rkat_from_checkout)
        elif args.step == "check-conversation":
            macos_check_conversation(args.work)
        elif args.step == "check-official-install":
            macos_check_official_install(args.work, args.chrome)
        elif args.step == "stage-record":
            macos_stage_record(args.work, args.output, args.consumer_not_run)
        elif args.step == "assemble":
            macos_assemble(args.work, args.stage, args.review, args.review_sha256, args.output)
        else:
            macos_release_check(args.assembly, args.privacy_report, args.privacy_report_sha256)
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError, tarfile.TarError,
            zipfile.BadZipFile) as refusal:
        print(f"connect-candidate macos {args.step} refused: {type(refusal).__name__}: {refusal}", file=sys.stderr)
        return 1
    return 0


def main():
    if sys.argv[1:2] == ["macos"]:
        return macos_main(sys.argv[2:])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("preflight", "build", "stage-cli", "assemble-cli", "fetch-built",
                                            "accept-publication", "publish-accepted", "privacy-scan"))
    for name in ("request", "candidate", "accepted", "output", "candidate-work", "shutdown-review",
                 "rules", "exemptions", "accepted-findings"):
        parser.add_argument("--" + name, type=Path)
    reader = parser.add_mutually_exclusive_group()
    reader.add_argument("--forge-reader-source", type=Path, help="exact selected Forge reader checkout")
    reader.add_argument("--meta-source", type=Path, help="legacy alias for --forge-reader-source; reader selection is still checked")
    parser.add_argument("--acceptance-sha256")
    parser.add_argument("--shutdown-review-sha256")
    parser.add_argument("payload", nargs="*", type=Path, help="privacy-scan only: the exact release payload files")
    args = parser.parse_intermixed_args()
    command = args.command
    require(command == "privacy-scan" or not args.payload, "payload files are taken by privacy-scan only")
    if command == "preflight":
        preflight(native=True)
    elif command == "build":
        build()
    elif command == "stage-cli":
        require(args.candidate_work, "retained candidate work directory required")
        stage_cli(args.candidate_work)
    elif command == "assemble-cli":
        require(all((args.candidate_work, args.shutdown_review, args.shutdown_review_sha256, args.output)),
                "retained producer, exact independent review and output required")
        assemble_cli(args.candidate_work, args.shutdown_review, args.shutdown_review_sha256, args.output,
                     rules=args.rules, exemptions=args.exemptions, accepted_findings=args.accepted_findings)
    elif command == "privacy-scan":
        privacy = privacy_inputs(args.rules, args.exemptions, args.accepted_findings)
        require(args.output and args.payload, "privacy scan output and payload files required")
        privacy_scan(args.payload, privacy, args.output)
    elif command == "fetch-built":
        fetch_built()
    elif command == "accept-publication":
        reader_source = args.forge_reader_source or args.meta_source
        require(all((args.request, args.candidate, reader_source, args.output)), "acceptance paths required")
        accept_publication(args.request, args.candidate, reader_source, args.output)
    else:
        require(all((args.accepted, args.acceptance_sha256, args.output)), "selected acceptance and output required")
        publish_accepted(args.accepted, args.acceptance_sha256, args.output)


if __name__ == "__main__":
    sys.exit(main())
