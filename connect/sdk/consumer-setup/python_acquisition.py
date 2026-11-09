"""Retain a PyPI wheel selection for the existing selected Python installer.

Pip owns resolution. The pinned installer owns wheel validation. This module
binds their observations to retained bytes; it does not install or activate.
The trusted PyPI/pip scope is not an OS or network sandbox.
"""
from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import re
import ssl
import stat
import sys
import time
import urllib.parse
import urllib.request


SCOPE = "trusted_pypi_wheel_acquisition"
RESULT_SCOPE = "retained_python_wheel_inputs"
REPORT_LIMIT = 1024 * 1024
WHEEL_LIMIT = 64 * 1024 * 1024
TOTAL_LIMIT = 512 * 1024 * 1024
PIP_RSS_LIMIT = 2 * 1024 * 1024 * 1024
TOOL_LIMIT = 128 * 1024 * 1024
WHEEL_COUNT = 256
TIMEOUT = 120
DEFAULT_TARGET = "macos-arm64"
TARGETS = {"macos-arm64": ("darwin", "arm64"), "linux-x86_64": ("linux", "x86_64")}
HEX = re.compile(r"[a-f0-9]{64}\Z")
IDENTIFIER = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*\Z")
REQUEST_ID = re.compile(r"[a-f0-9]{32}\Z")
VERSION = re.compile(r"[0-9][A-Za-z0-9.!+_-]{0,127}\Z")
CONSTRAINT_NAME = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?\Z")
CONSTRAINT_LIMIT = 32
CONSTRAINT_BYTES = 16384
FILENAME = re.compile(r"[A-Za-z0-9_.+!-]{1,188}\.whl\Z")


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def wire(value):
    return (json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"),
                       allow_nan=False) + "\n").encode("ascii")


def _target(value):
    if not isinstance(value, str) or value not in TARGETS:
        raise ValueError()
    return value


def _runtime(io, raw, target):
    value = io.decode(raw)
    io.keys(value, ("target", "stdlib", *(("pointer_bits",) if target == "linux-x86_64" else ())))
    expected = ["cpython", 3, 11, *TARGETS[_target(target)]]
    if (wire(value["target"]) != wire(expected) or not isinstance(value["stdlib"], list)
            or any(not isinstance(name, str) for name in value["stdlib"])
            or target == "linux-x86_64" and (type(value["pointer_bits"]) is not int or value["pointer_bits"] != 64)):
        raise ValueError()
    return value


def _rss_bytes(maximum, system):
    # getrusage reports bytes on Darwin and KiB on Linux.
    return maximum * 1024 if system == "linux" else maximum


def _inventory(path, limit):
    """Inspect one bounded directory without following its entries or alias."""
    result = {}
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        with os.scandir(fd) as entries:
            for entry in entries:
                if len(result) >= limit:
                    raise ValueError()
                mode = entry.stat(follow_symlinks=False).st_mode
                if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
                    raise ValueError()
                result[entry.name] = "directory" if stat.S_ISDIR(mode) else "file"
    finally:
        os.close(fd)
    return result


def readback(io, request, job_directory):
    """Observe retained acquisition bytes, never replay work or attest usability.

    The caller verifies the durable request's digest and owns exclusive custody.
    The receipt is historical evidence, not proof of current runtime or access.
    """
    try:
        request = copy.deepcopy(request)
        args = _arguments(io, request["arguments"])
        workspace, stage, job = Path(request["workspace"]), Path(request["staging_root"]), Path(job_directory)
        if (request["family"] != "selected-python" or not REQUEST_ID.fullmatch(request["job_id"])
                or job != workspace / "acquisitions" / request["job_id"]):
            raise ValueError()
        io.canonical_directory(job)
        assets = workspace / "python-acquisition"
        io.canonical_directory(assets)
        pins = request["host"]["assets"]
        io.keys(pins, ("profile.json", "pip.pyz", "installer.py", "worker.py"))
        for name, expected in pins.items():
            if not isinstance(expected, str) or not HEX.fullmatch(expected) or sha(io.read_file(assets, name, TOOL_LIMIT)) != expected:
                raise ValueError()
        profile = io.decode(io.read_file(assets, "profile.json", 65536))
        target = _target(profile.get("target", DEFAULT_TARGET))
        if (pins["profile.json"] != request["host"]["profile_sha256"] or profile["scope"] != SCOPE
                or profile["pip"]["sha256"] != pins["pip.pyz"]):
            raise ValueError()
        worker_raw = io.read_file(job, "worker.json", 65536)
        worker = io.decode(worker_raw)
        expected_worker = {"request": args, "pip": str(assets / "pip.pyz"),
                           "pip_sha256": pins["pip.pyz"], "directory": str(job)}
        if "target" in worker:
            expected_worker["target"] = target
        elif target != DEFAULT_TARGET or "target" in profile:
            raise ValueError()
        if wire(worker) != wire(expected_worker):
            raise ValueError()
        _check_constraint_file(io, args, job)
        runtime_raw = io.read_file(job, "runtime.json", REPORT_LIMIT)
        _runtime(io, runtime_raw, target)
        destination = stage / job.name
        io.canonical_directory(destination)
        raw = io.read_file(destination, "acquisition.json", REPORT_LIMIT)
        receipt = io.decode(raw)
        io.keys(receipt, ("schema_version", "scope", "request", "host_profile_sha256", "report_sha256",
                          "pip_sha256", "installer_sha256", "staged_id", "primary", "artifacts", "files",
                          *(("target",) if "target" in receipt else ())))
        if (type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1
                or receipt["scope"] != RESULT_SCOPE or receipt["request"] != args
                or receipt["staged_id"] != job.name or receipt["host_profile_sha256"] != pins["profile.json"]
                or receipt["pip_sha256"] != pins["pip.pyz"] or receipt["installer_sha256"] != pins["installer.py"]
                or receipt.get("target", DEFAULT_TARGET) != target
                or "target" in profile and "target" not in receipt):
            raise ValueError()
        report = io.read_file(job, "report.json", REPORT_LIMIT)
        rows = _parse_report(io, report, args)
        if (receipt["report_sha256"] != sha(report) or wire(receipt["artifacts"]) != wire(rows)
                or wire(receipt["primary"]) != wire(rows[0])):
            raise ValueError()
        requirements = "".join(row["name"] + "==" + row["version"] + " --hash=sha256:" + row["sha256"] + "\n"
                               for row in sorted(rows, key=lambda row: row["name"])).encode("ascii")
        expected = {"artifact.whl" if row["primary"] else "closure/" + row["filename"]: row["sha256"] for row in rows}
        expected["closure/requirements"] = sha(requirements)
        if receipt["files"] != expected:
            raise ValueError()
        root = _inventory(destination, 5)
        optional = {name: "file" for name in ("package.json", "activation.json") if name in root}
        if root != {"artifact.whl": "file", "acquisition.json": "file", "closure": "directory", **optional}:
            raise ValueError()
        if _inventory(destination / "closure", WHEEL_COUNT) != {name.split("/", 1)[1]: "file" for name in expected if name.startswith("closure/")}:
            raise ValueError()
        # Later authored metadata is bounded but outside this byte proof.
        for name in optional:
            io.read_file(destination, name, REPORT_LIMIT)
        total = 0
        for name, digest in expected.items():
            payload = io.read_file(destination, name, WHEEL_LIMIT)
            total += len(payload)
            if total > TOTAL_LIMIT or sha(payload) != digest:
                raise ValueError()
            original = requirements if name == "closure/requirements" else io.read_file(job, "payloads/" + name.rsplit("/", 1)[-1], WHEEL_LIMIT)
            if payload != original:
                raise ValueError()
        if (io.read_file(destination, "acquisition.json", REPORT_LIMIT) != raw
                or io.read_file(job, "worker.json", 65536) != worker_raw
                or io.read_file(job, "runtime.json", REPORT_LIMIT) != runtime_raw):
            raise ValueError()
        _check_constraint_file(io, args, job)
        return {**receipt, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(raw)}
    except Exception:
        raise io.Error("python_acquisition_readback_unconfirmed", 409) from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("artifact_redirect")


def artifact_url(value):
    if not isinstance(value, str) or len(value) > 4096 or any(ord(c) < 33 or ord(c) > 126 for c in value):
        raise ValueError("artifact_url")
    parts = urllib.parse.urlsplit(value)
    filename = parts.path.rsplit("/", 1)[-1]
    if (parts.scheme != "https" or parts.netloc != "files.pythonhosted.org"
            or parts.query or parts.fragment or not parts.path.startswith("/packages/")
            or any(p in ("", ".", "..") for p in parts.path.split("/")[1:])
            or "%" in parts.path or not FILENAME.fullmatch(filename)):
        raise ValueError("artifact_url")
    return filename


def _constraint_pins(request):
    """Exact limits, not additional install roots; preserve the original request."""
    if "constraints" not in request:
        return {}
    values = request["constraints"]
    if not isinstance(values, list) or not 1 <= len(values) <= CONSTRAINT_LIMIT:
        raise ValueError()
    pins = {}
    for item in values:
        if not isinstance(item, dict) or set(item) != {"distribution", "version"}:
            raise ValueError()
        name, version = item["distribution"], item["version"]
        if (not isinstance(name, str) or not CONSTRAINT_NAME.fullmatch(name)
                or not isinstance(version, str) or not VERSION.fullmatch(version)):
            raise ValueError()
        name = re.sub(r"[-_.]+", "-", name).lower()
        if (name in pins or name == request["distribution"] and version != request["version"]):
            raise ValueError()
        pins[name] = version
    return pins


def _constraint_bytes(request):
    pins = _constraint_pins(request)
    return "".join(name + "==" + pins[name] + "\n" for name in sorted(pins)).encode("ascii")


def _check_constraint_file(io, request, directory):
    if "constraints" in request:
        if io.read_file(directory, "constraints.txt", CONSTRAINT_BYTES) != _constraint_bytes(request):
            raise ValueError()


def _arguments(io, request):
    try:
        io.keys(request, ("request_id", "distribution", "version",
                          *(("constraints",) if "constraints" in request else ())))
        if (not isinstance(request["request_id"], str) or not REQUEST_ID.fullmatch(request["request_id"])
                or not isinstance(request["distribution"], str) or len(request["distribution"]) > 128
                or not IDENTIFIER.fullmatch(request["distribution"])
                or not isinstance(request["version"], str) or not VERSION.fullmatch(request["version"])):
            raise ValueError()
        _constraint_pins(request)
    except (ValueError, TypeError, KeyError, io.Error):
        raise io.Error("invalid_python_acquisition_arguments") from None
    return copy.deepcopy(request)


def _parse_report(io, raw, request):
    pins = _constraint_pins(request)
    report = io.decode(raw)
    if (not isinstance(report, dict) or report.get("version") != "1"
            or not isinstance(report.get("install"), list) or not 1 <= len(report["install"]) <= WHEEL_COUNT
            or not isinstance(report.get("pip_version"), str)):
        raise ValueError()
    rows, identities, filenames, roots = [], set(), set(), 0
    for item in report["install"]:
        if (not isinstance(item, dict) or type(item.get("requested")) is not bool
                or item.get("is_direct") is not False or item.get("is_yanked") is not False):
            raise ValueError()
        info, metadata = item["download_info"], item["metadata"]
        io.keys(info, ("url", "archive_info"))
        filename = artifact_url(info["url"])
        hashes = info["archive_info"]["hashes"]
        digest = hashes["sha256"]
        if not isinstance(digest, str) or not HEX.fullmatch(digest):
            raise ValueError()
        identity = re.sub(r"[-_.]+", "-", metadata["name"]).lower()
        version = metadata["version"]
        if (not IDENTIFIER.fullmatch(identity) or len(identity) > 128
                or not isinstance(version, str) or not VERSION.fullmatch(version)
                or identity in identities or filename.casefold() in filenames
                or identity in pins and version != pins[identity]):
            raise ValueError()
        if item["requested"]:
            roots += 1
            if (identity, version) != (request["distribution"], request["version"]):
                raise ValueError()
        identities.add(identity)
        filenames.add(filename.casefold())
        rows.append({"name": identity, "version": version, "filename": filename,
                     "url": info["url"], "sha256": digest, "primary": item["requested"]})
    if roots != 1:
        raise ValueError()
    return sorted(rows, key=lambda row: (not row["primary"], row["name"]))


class PythonAcquisition:
    def __init__(self, io, profile_path, profile_sha, python_path, staging_root,
                 installer_path, installer_sha):
        self.io = io
        self.workspace = None
        self.profile_sha = profile_sha
        self.installer_sha = installer_sha
        try:
            self.profile_path = Path(profile_path)
            self.python = Path(python_path)
            self.staging = Path(staging_root)
            self.installer = Path(installer_path)
            if not isinstance(profile_sha, str) or not HEX.fullmatch(profile_sha):
                raise ValueError()
            self.profile_raw = self.read_path(self.profile_path, 64 * 1024)
            if sha(self.profile_raw) != profile_sha:
                raise ValueError()
            self.profile = io.decode(self.profile_raw)
            io.keys(self.profile, ("schema_version", "scope", "python_sha256", "pip",
                                   *(("target",) if "target" in self.profile else ())))
            io.keys(self.profile["pip"], ("path", "sha256"))
            self.target = _target(self.profile.get("target", DEFAULT_TARGET))
            if (type(self.profile["schema_version"]) is not int or self.profile["schema_version"] != 1
                    or self.profile["scope"] != SCOPE):
                raise ValueError()
            for value in (self.profile["python_sha256"], self.profile["pip"]["sha256"], installer_sha):
                if not isinstance(value, str) or not HEX.fullmatch(value):
                    raise ValueError()
            self.pip = Path(self.profile["pip"]["path"])
            self.python_raw = self.read_path(self.python, TOOL_LIMIT)
            self.pip_raw = self.read_path(self.pip, TOOL_LIMIT)
            self.installer_raw = self.read_path(self.installer, TOOL_LIMIT)
            self.worker_raw = self.read_path(Path(__file__).resolve(), REPORT_LIMIT)
            if (sha(self.python_raw) != self.profile["python_sha256"]
                    or sha(self.pip_raw) != self.profile["pip"]["sha256"]
                    or sha(self.installer_raw) != installer_sha or not os.access(self.python, os.X_OK)):
                raise ValueError()
            io.canonical_directory(self.staging)
        except (OSError, ValueError, TypeError, KeyError, AttributeError, io.Error):
            raise io.Error("invalid_python_acquisition_profile") from None

    def read_path(self, path, limit):
        if not path.is_absolute() or str(path) != str(path.resolve(strict=True)):
            raise ValueError()
        return self.io.read_file(self.io.canonical_directory(path.parent), path.name, limit)

    def arguments(self, request):
        return _arguments(self.io, request)

    def summary(self):
        return {"available": True, "scope": SCOPE, "staging_root": str(self.staging),
                "host_profile_sha256": self.profile_sha, "index_url": "https://pypi.org/simple",
                "target": "CPython 3.11 " + ("macOS arm64" if self.target == DEFAULT_TARGET else "Linux x86_64"),
                "max_wheels": WHEEL_COUNT,
                "max_wheel_bytes": WHEEL_LIMIT, "max_total_bytes": TOTAL_LIMIT,
                "timeout_seconds": TIMEOUT, "max_socket_wait_seconds": 10,
                "pip_rss_monitor_bytes": PIP_RSS_LIMIT, "resource_monitor": "best_effort"}

    def bind(self, workspace):
        self.io.canonical_directory(workspace)
        if workspace == self.staging or workspace in self.staging.parents or self.staging in workspace.parents:
            raise self.io.Error("invalid_python_acquisition_workspace")
        self.workspace = workspace
        self.assets = workspace / "python-acquisition"
        self.assets.mkdir(mode=0o700)
        self.frozen = {"profile.json": self.profile_raw, "pip.pyz": self.pip_raw,
                       "installer.py": self.installer_raw, "worker.py": self.worker_raw}
        for name, raw in self.frozen.items():
            self.io.write_new(self.assets / name, raw)
            (self.assets / name).chmod(0o400)

    def verify(self):
        try:
            if self.workspace is None:
                raise ValueError()
            for path, raw in ((self.profile_path, self.profile_raw), (self.python, self.python_raw),
                              (self.pip, self.pip_raw), (self.installer, self.installer_raw)):
                if self.read_path(path, TOOL_LIMIT) != raw:
                    raise ValueError()
            for name, raw in self.frozen.items():
                if self.io.read_file(self.assets, name, TOOL_LIMIT) != raw:
                    raise ValueError()
            self.io.canonical_directory(self.staging)
        except (OSError, ValueError, self.io.Error):
            raise self.io.Error("python_acquisition_host_changed", 409) from None

    def parse_report(self, raw, request):
        return _parse_report(self.io, raw, request)

    def fetch(self, row, checkpoint):
        checkpoint()
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
                                            urllib.request.HTTPSHandler(context=ssl.create_default_context()))
        request = urllib.request.Request(row["url"], headers={"Accept-Encoding": "identity"})
        with opener.open(request, timeout=min(10, checkpoint())) as response:
            if response.status != 200 or response.geturl() != row["url"]:
                raise ValueError()
            length = response.headers.get("Content-Length")
            if length is not None and (not length.isdecimal() or int(length) > WHEEL_LIMIT):
                raise ValueError()
            if response.headers.get("Content-Encoding", "identity") != "identity":
                raise ValueError()
            data = bytearray()
            while True:
                checkpoint()
                # One buffered read yields between network receives. read(n)
                # may keep filling n bytes while a slow peer resets each socket
                # timeout, indefinitely postponing our stop/deadline check.
                chunk = response.read1(min(65536, WHEEL_LIMIT + 1 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > WHEEL_LIMIT:
                    raise ValueError()
            if length is not None and len(data) != int(length):
                raise ValueError()
        checkpoint()
        raw = bytes(data)
        if sha(raw) != row["sha256"]:
            raise ValueError()
        return raw

    def fetch_retained(self, row, checkpoint, run_callable, job_directory):
        # All network parsing belongs in a Runner-owned child. Socket timeouts
        # only limit inactivity: a peer can trickle headers/chunk framing and
        # otherwise prevent the parent job's deadline or stop check from running.
        stem = "download-" + row["sha256"]
        settings = wire({"row": row, "destination": str(job_directory / "payloads" / stem)})
        path = job_directory / (stem + ".json")
        self.io.write_new(path, settings)
        code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                                "--fetch", str(path), sha(settings)],
                               capture=True, inspection_timeout=checkpoint())
        checkpoint()
        if code:
            raise self.io.Error("python_acquisition_download_failed", 409)
        raw = self.io.read_file(job_directory / "payloads", stem, WHEEL_LIMIT)
        if sha(raw) != row["sha256"]:
            raise ValueError()
        return raw

    def run(self, request, job_directory, run_callable, check_stop):
        request = self.arguments(request)
        self.verify()
        deadline = time.monotonic() + TIMEOUT
        def checkpoint():
            check_stop()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise self.io.Error("python_acquisition_timeout", 409)
            return remaining
        try:
            if (job_directory.parent != self.workspace / "acquisitions"
                    or not REQUEST_ID.fullmatch(job_directory.name)):
                raise ValueError()
            self.io.canonical_directory(job_directory)
            destination = self.staging / job_directory.name
            if destination.exists() or destination.is_symlink():
                raise ValueError()
            for name in ("home", "tmp", "payloads"):
                (job_directory / name).mkdir(mode=0o700)
            worker = {"request": request, "pip": str(self.assets / "pip.pyz"),
                      "pip_sha256": sha(self.pip_raw), "directory": str(job_directory), "target": self.target}
            worker_raw = wire(worker)
            self.io.write_new(job_directory / "worker.json", worker_raw)
            code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                                    "--worker", str(job_directory / "worker.json"), sha(worker_raw)],
                                   capture=True, inspection_timeout=checkpoint())
            checkpoint()
            if code != 0:
                reason = {1: "python_acquisition_resolution_failed", 72: "python_acquisition_resource_limit",
                          73: "python_acquisition_runtime_unsupported", 74: "python_acquisition_version_invalid",
                          75: "python_acquisition_worker_failed"}.get(code, "python_acquisition_unconfirmed")
                raise self.io.Error(reason, 409)
            _check_constraint_file(self.io, request, job_directory)
            report_raw = self.io.read_file(job_directory, "report.json", REPORT_LIMIT)
            rows = self.parse_report(report_raw, request)
            runtime_raw = self.io.read_file(job_directory, "runtime.json", REPORT_LIMIT)
            runtime = _runtime(self.io, runtime_raw, self.target)
            owner = {"__name__": "toolkit_pinned_wheel_inspector"}
            exec(compile(self.installer_raw, str(self.assets / "installer.py"), "exec"), owner)
            total, expanded, members, paths, scripts, files = 0, 0, 0, [], [], {}
            for row in rows:
                raw = self.fetch_retained(row, checkpoint, run_callable, job_directory)
                total += len(raw)
                if total > TOTAL_LIMIT:
                    raise ValueError()
                wheel = owner["selected_wheel"](row["filename"], raw, set(runtime["stdlib"]))
                if (wheel["name"], wheel["version"], wheel["sha256"]) != (row["name"], row["version"], row["sha256"]):
                    raise ValueError()
                expanded += wheel["expanded_bytes"]
                members += len(wheel["files"])
                if expanded > TOTAL_LIMIT or members > owner["MAX_NPM_MEMBERS"]:
                    raise ValueError()
                paths.extend(wheel["files"])
                scripts.extend(wheel["scripts"])
                stored = "artifact.whl" if row["primary"] else row["filename"]
                self.io.write_new(job_directory / "payloads" / stored, raw)
                files["artifact.whl" if row["primary"] else "closure/" + stored] = sha(raw)
                del wheel, raw
            owner["checked_paths"](paths)
            owner["checked_paths"](scripts)
            requirements = "".join(row["name"] + "==" + row["version"] + " --hash=sha256:" + row["sha256"] + "\n"
                                   for row in sorted(rows, key=lambda row: row["name"])).encode("ascii")
            if total + len(requirements) > TOTAL_LIMIT:
                raise ValueError()
            files["closure/requirements"] = sha(requirements)
            self.verify()
            checkpoint()
            destination.mkdir(mode=0o700)
            (destination / "closure").mkdir(mode=0o700)
            for name, expected in files.items():
                checkpoint()
                raw = requirements if name == "closure/requirements" else self.io.read_file(
                    job_directory / "payloads", name.rsplit("/", 1)[-1], WHEEL_LIMIT)
                if sha(raw) != expected:
                    raise ValueError()
                self.io.write_new(destination / name, raw)
            if (self.io.read_file(job_directory, "report.json", REPORT_LIMIT) != report_raw
                    or self.io.read_file(job_directory, "runtime.json", REPORT_LIMIT) != runtime_raw
                    or self.io.read_file(job_directory, "worker.json", 64 * 1024) != worker_raw):
                raise ValueError()
            for name, expected in files.items():
                checkpoint()
                if sha(self.io.read_file(destination, name, WHEEL_LIMIT)) != expected:
                    raise ValueError()
            self.verify()
            checkpoint()
            _check_constraint_file(self.io, request, job_directory)
            receipt = {"schema_version": 1, "scope": RESULT_SCOPE, "request": request,
                       "host_profile_sha256": self.profile_sha, "report_sha256": sha(report_raw),
                       "pip_sha256": sha(self.pip_raw), "installer_sha256": self.installer_sha,
                       "staged_id": job_directory.name, "primary": rows[0], "artifacts": rows,
                       "files": files, "target": self.target}
            receipt_raw = wire(receipt)
            self.io.write_new(destination / "acquisition.json", receipt_raw)
            return {**receipt, "receipt_path": str(destination / "acquisition.json"),
                    "receipt_sha256": sha(receipt_raw)}
        except self.io.Error:
            raise
        except Exception:
            raise self.io.Error("python_acquisition_unconfirmed", 409) from None


def _worker_settings(path, expected):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError()
        raw = stream.read(64 * 1024 + 1)
    if len(raw) > 64 * 1024 or sha(raw) != expected:
        raise ValueError()
    return json.loads(raw)


def _fetch_worker(path, expected):
    settings = _worker_settings(path, expected)
    # This child receives only parent-validated artifact metadata. Repeat the
    # URL check before I/O and retain the hash check in both child and parent.
    row = settings["row"]
    artifact_url(row["url"])
    deadline = time.monotonic() + TIMEOUT
    def checkpoint():
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError()
        return remaining
    raw = PythonAcquisition.fetch(None, row, checkpoint)
    with Path(settings["destination"]).open("xb") as stream:
        stream.write(raw)


def _worker(path, expected):
    """Run trusted pip in the Runner-owned child, without a second child owner."""
    import platform
    import resource
    import runpy
    import struct
    import threading
    settings = _worker_settings(path, expected)
    target = [sys.implementation.name, *sys.version_info[:2], sys.platform, platform.machine()]
    requested_target = _target(settings.get("target", DEFAULT_TARGET))
    if (target != ["cpython", 3, 11, *TARGETS[requested_target]]
            or requested_target == "linux-x86_64" and struct.calcsize("P") * 8 != 64):
        raise SystemExit(73)
    directory, pip = Path(settings["directory"]), Path(settings["pip"])
    if sha(pip.read_bytes()) != settings["pip_sha256"]:
        raise ValueError()
    for kind, limit in ((resource.RLIMIT_FSIZE, WHEEL_LIMIT), (resource.RLIMIT_CPU, TIMEOUT),
                        (resource.RLIMIT_CORE, 0)):
        _, hard = resource.getrlimit(kind)
        bound = limit if hard == resource.RLIM_INFINITY else min(limit, hard)
        resource.setrlimit(kind, (bound, bound))
    os.environ.clear()
    os.environ.update({"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(directory / "home"),
                       "TMPDIR": str(directory / "tmp"), "TMP": str(directory / "tmp"),
                       "TEMP": str(directory / "tmp"), "PIP_CONFIG_FILE": os.devnull,
                       "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"})
    os.chdir(directory)
    sys.path.insert(0, str(pip))
    from pip._vendor.packaging.version import Version
    version = settings["request"]["version"]
    try:
        for value in [version, *_constraint_pins(settings["request"]).values()]:
            if str(Version(value)) != value:
                raise ValueError()
    except ValueError:
        raise SystemExit(74) from None
    constraint_args = []
    if "constraints" in settings["request"]:
        constraint_path = directory / "constraints.txt"
        fd = os.open(constraint_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(_constraint_bytes(settings["request"]))
        constraint_args = ["--constraint", str(constraint_path)]
    with (directory / "runtime.json").open("xb") as stream:
        stream.write(wire({"target": target, "stdlib": sorted(sys.stdlib_module_names),
                          **({"pointer_bits": 64} if requested_target == "linux-x86_64" else {})}))
    finished = threading.Event()
    def watch_temp():
        # These sampled guards can overshoot; they are not an allocation limit
        # or filesystem confinement. Linux ru_maxrss is reported in KiB.
        while not finished.wait(0.1):
            total, count = 0, 0
            try:
                if _rss_bytes(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss, sys.platform) > PIP_RSS_LIMIT:
                    os._exit(72)
                for root, _, names in os.walk(directory / "tmp", followlinks=False):
                    for name in names:
                        try:
                            total += (Path(root) / name).lstat().st_size
                            count += 1
                        except FileNotFoundError:
                            pass
                        if total > TOTAL_LIMIT or count > 4096:
                            os._exit(72)
            except OSError:
                os._exit(72)
    threading.Thread(target=watch_temp, daemon=True).start()
    sys.argv = [str(pip), "--disable-pip-version-check", "--no-input", "--keyring-provider", "disabled",
                "--retries", "0", "--timeout", "10", "--no-cache-dir", "install", "--dry-run",
                "--ignore-installed", "--only-binary=:all:", "--progress-bar", "off",
                "--report", str(directory / "report.json"), "--index-url", "https://pypi.org/simple",
                *constraint_args, settings["request"]["distribution"] + "==" + version]
    try:
        runpy.run_path(str(pip), run_name="__main__")
    finally:
        finished.set()


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4 or sys.argv[1] not in ("--worker", "--fetch"):
            raise ValueError()
        (_worker if sys.argv[1] == "--worker" else _fetch_worker)(sys.argv[2], sys.argv[3])
    except Exception:
        print("python_acquisition_worker_failed", file=sys.stderr)
        raise SystemExit(75) from None
