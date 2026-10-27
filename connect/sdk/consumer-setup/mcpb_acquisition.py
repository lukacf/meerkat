"""Retain host-approved public MCPB bytes; the canonical reader owns ZIP meaning.

Origin restrictions are not resolved-address egress enforcement or a sandbox.
The host owns that policy. Redirect query strings exist only in worker memory.
"""
from __future__ import annotations

import copy
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import ssl
import stat
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


SCOPE = "trusted_public_mcpb_acquisition"
RESULT_SCOPE = "retained_mcpb_artifact_bytes"
ARTIFACT_LIMIT = 64 * 1024 * 1024
TOOL_LIMIT = 128 * 1024 * 1024
REPORT_LIMIT = 64 * 1024
TIMEOUT = 120
MAX_REDIRECTS = 5
HEX = re.compile(r"[a-f0-9]{64}\Z")
REQUEST_ID = re.compile(r"[a-f0-9]{32}\Z")


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def wire(value):
    return (json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"),
                       allow_nan=False) + "\n").encode("ascii")


def _keys(value, names):
    if type(value) is not dict or set(value) != set(names):
        raise ValueError()


def _hash(value):
    return type(value) is str and HEX.fullmatch(value) is not None


def origin(value):
    """Accept one unambiguous canonical HTTPS origin, including an explicit port."""
    if type(value) is not str or not 1 <= len(value) <= 320 or any(ord(c) < 33 or ord(c) > 126 for c in value):
        raise ValueError()
    parts = urllib.parse.urlsplit(value)
    if (parts.scheme != "https" or parts.path or "?" in value or "#" in value
            or parts.username is not None or parts.password is not None or "\\" in value
            or not parts.hostname):
        raise ValueError()
    host = parts.hostname
    if ":" in host:
        host = "[" + str(ipaddress.IPv6Address(host)) + "]"
    elif (len(host) > 253 or any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label)
                               for label in host.split("."))):
        raise ValueError()
    port = parts.port
    if port is not None and not 1 <= port <= 65535:
        raise ValueError()
    canonical = "https://" + host + (":" + str(port) if port not in (None, 443) else "")
    if value != canonical:
        raise ValueError()
    return canonical


def artifact_url(value, *, redirect=False):
    if (type(value) is not str or not 1 <= len(value) <= (8192 if redirect else 4096)
            or any(ord(c) < 33 or ord(c) > 126 for c in value)
            or "\\" in value or "#" in value or not redirect and "?" in value
            or re.search(r"%(?![0-9a-fA-F]{2})", value)):
        raise ValueError()
    parts = urllib.parse.urlsplit(value)
    source = origin(parts.scheme + "://" + parts.netloc)
    if (parts.path and not parts.path.startswith("/")
            or re.search(r"%(?:0[0-9a-f]|1[0-9a-f]|7f)", parts.path, re.I)):
        raise ValueError()
    return source


def _profile(value):
    _keys(value, ("schema_version", "scope", "python", "allowed_origins", "allowed_redirect_origins"))
    _keys(value["python"], ("path", "sha256"))
    path = value["python"]["path"]
    if (type(value["schema_version"]) is not int or value["schema_version"] != 1
            or value["scope"] != SCOPE or not _hash(value["python"]["sha256"])
            or type(path) is not str or not path.startswith("/") or str(Path(path)) != path
            or ".." in Path(path).parts or any(ord(c) < 32 or ord(c) == 127 for c in path)):
        raise ValueError()
    for name, minimum in (("allowed_origins", 1), ("allowed_redirect_origins", 0)):
        values = value[name]
        if type(values) is not list or not minimum <= len(values) <= 32:
            raise ValueError()
        if [origin(item) for item in values] != values or len(set(values)) != len(values):
            raise ValueError()
    return copy.deepcopy(value)


def _arguments(request, profile):
    _keys(request, ("request_id", "url", "artifact_sha256"))
    if (type(request["request_id"]) is not str or not REQUEST_ID.fullmatch(request["request_id"])
            or not _hash(request["artifact_sha256"])
            or artifact_url(request["url"]) not in profile["allowed_origins"]):
        raise ValueError()
    return copy.deepcopy(request)


def _report(value, request, profile):
    _keys(value, ("primary",))
    row = value["primary"]
    _keys(row, ("url", "sha256", "byte_length", "final_origin", "redirect_count"))
    if (row["url"] != request["url"] or row["sha256"] != request["artifact_sha256"]
            or type(row["byte_length"]) is not int or not 0 <= row["byte_length"] <= ARTIFACT_LIMIT
            or type(row["redirect_count"]) is not int or not 0 <= row["redirect_count"] <= MAX_REDIRECTS
            or origin(row["final_origin"]) not in (profile["allowed_redirect_origins"]
                if row["redirect_count"] else [artifact_url(request["url"])])):
        raise ValueError()
    return row


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def fetch(request, profile, checkpoint):
    """Network-only worker operation; never return or persist redirect URLs."""
    request = _arguments(request, profile)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
                                        urllib.request.HTTPSHandler(context=ssl.create_default_context()))
    current, seen, count = request["url"], set(), 0
    while True:
        remaining = checkpoint()
        if current in seen:
            raise ValueError()
        seen.add(current)
        current_origin = artifact_url(current, redirect=count > 0)
        if current_origin not in (profile["allowed_redirect_origins"] if count else profile["allowed_origins"]):
            raise ValueError()
        req = urllib.request.Request(current, headers={"Accept-Encoding": "identity"})
        try:
            response = opener.open(req, timeout=min(10, remaining))
        except urllib.error.HTTPError as error:
            # Do not propagate the exception: its text and headers can carry a
            # signed URL. Every redirect is validated before the next request.
            try:
                locations = error.headers.get_all("Location", [])
                if error.code not in (301, 302, 303, 307, 308) or count >= MAX_REDIRECTS or len(locations) != 1:
                    raise ValueError() from None
                location = locations[0]
                if (not location or len(location) > 8192
                        or any(ord(c) < 33 or ord(c) > 126 for c in location)):
                    raise ValueError() from None
                current = urllib.parse.urljoin(current, location)
                artifact_url(current, redirect=True)
                count += 1
            finally:
                error.close()
            continue
        with response:
            if response.status != 200 or response.geturl() != current:
                raise ValueError()
            lengths = response.headers.get_all("Content-Length", [])
            encodings = response.headers.get_all("Content-Encoding", [])
            if (len(lengths) > 1 or lengths and (not lengths[0].isdecimal() or int(lengths[0]) > ARTIFACT_LIMIT)
                    or len(encodings) > 1 or encodings and encodings[0].lower() != "identity"):
                raise ValueError()
            data = bytearray()
            while True:
                checkpoint()
                chunk = response.read1(min(65536, ARTIFACT_LIMIT + 1 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > ARTIFACT_LIMIT:
                    raise ValueError()
            if lengths and len(data) != int(lengths[0]):
                raise ValueError()
        checkpoint()
        raw = bytes(data)
        if sha(raw) != request["artifact_sha256"]:
            raise ValueError()
        return raw, {"primary": {"url": request["url"], "sha256": sha(raw), "byte_length": len(raw),
                                 "final_origin": current_origin, "redirect_count": count}}


class McpbAcquisition:
    def __init__(self, profile_path, profile_sha, staging_root, *, io):
        self.io, self.workspace, self.profile_sha = io, None, profile_sha
        try:
            self.profile_path, self.staging = Path(profile_path), Path(staging_root)
            self.profile_raw = self.read_path(self.profile_path, REPORT_LIMIT)
            if not _hash(profile_sha) or sha(self.profile_raw) != profile_sha:
                raise ValueError()
            self.profile = _profile(io.decode(self.profile_raw))
            self.python = Path(self.profile["python"]["path"])
            self.python_raw = self.read_path(self.python, TOOL_LIMIT)
            self.worker_path = Path(__file__).resolve()
            self.worker_raw = self.read_path(self.worker_path, REPORT_LIMIT)
            if sha(self.python_raw) != self.profile["python"]["sha256"] or not os.access(self.python, os.X_OK):
                raise ValueError()
            io.canonical_directory(self.staging)
            self.frozen = {"profile.json": self.profile_raw, "worker.py": self.worker_raw}
        except (OSError, ValueError, TypeError, KeyError, AttributeError, io.Error):
            raise io.Error("invalid_mcpb_acquisition_profile") from None

    def read_path(self, path, limit):
        if not path.is_absolute() or str(path) != str(path.resolve(strict=True)):
            raise ValueError()
        return self.io.read_file(self.io.canonical_directory(path.parent), path.name, limit)

    def arguments(self, request):
        try:
            return _arguments(request, self.profile)
        except (ValueError, TypeError, KeyError):
            raise self.io.Error("invalid_mcpb_acquisition_arguments") from None

    def summary(self):
        return {"available": True, "scope": SCOPE, "staging_root": str(self.staging),
                "host_profile_sha256": self.profile_sha,
                "allowed_origins": list(self.profile["allowed_origins"]),
                "allowed_redirect_origins": list(self.profile["allowed_redirect_origins"]),
                "max_artifact_bytes": ARTIFACT_LIMIT, "max_redirects": MAX_REDIRECTS,
                "timeout_seconds": TIMEOUT, "max_socket_wait_seconds": 10}

    def bind(self, workspace):
        self.io.canonical_directory(workspace)
        if workspace == self.staging or workspace in self.staging.parents or self.staging in workspace.parents:
            raise self.io.Error("invalid_mcpb_acquisition_workspace")
        self.workspace, self.assets = workspace, workspace / "mcpb-acquisition"
        self.assets.mkdir(mode=0o700)
        for name, raw in self.frozen.items():
            self.io.write_new(self.assets / name, raw)
            (self.assets / name).chmod(0o400)

    def verify(self):
        try:
            if self.workspace is None:
                raise ValueError()
            for path, raw in ((self.profile_path, self.profile_raw), (self.python, self.python_raw),
                              (self.worker_path, self.worker_raw)):
                if self.read_path(path, TOOL_LIMIT) != raw:
                    raise ValueError()
            for name, raw in self.frozen.items():
                if self.io.read_file(self.assets, name, TOOL_LIMIT) != raw:
                    raise ValueError()
            self.io.canonical_directory(self.staging)
        except (OSError, ValueError, self.io.Error):
            raise self.io.Error("mcpb_acquisition_host_changed", 409) from None

    def run(self, request, job_directory, run_callable, check_stop):
        request = self.arguments(request)
        self.verify()
        deadline = time.monotonic() + TIMEOUT
        def checkpoint():
            check_stop()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise self.io.Error("mcpb_acquisition_timeout", 409)
            return remaining
        try:
            checkpoint()
            if (job_directory.parent != self.workspace / "acquisitions"
                    or not REQUEST_ID.fullmatch(job_directory.name)):
                raise ValueError()
            self.io.canonical_directory(job_directory)
            if self.io.read_file(job_directory, "request.json", REPORT_LIMIT) != wire(request):
                raise ValueError()
            destination = self.staging / job_directory.name
            if destination.exists() or destination.is_symlink():
                raise ValueError()
            settings = wire({"request": request, "profile": self.profile, "directory": str(job_directory)})
            self.io.write_new(job_directory / "worker.json", settings)
            code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                                    "--fetch", str(job_directory / "worker.json"), sha(settings)],
                                   capture=True, inspection_timeout=checkpoint())
            checkpoint()
            if code:
                raise self.io.Error("mcpb_acquisition_download_failed", 409)
            report_raw = self.io.read_file(job_directory, "report.json", REPORT_LIMIT)
            primary = _report(self.io.decode(report_raw), request, self.profile)
            payload = self.io.read_file(job_directory, "artifact.mcpb", ARTIFACT_LIMIT)
            if len(payload) != primary["byte_length"] or sha(payload) != request["artifact_sha256"]:
                raise ValueError()
            self.verify()
            checkpoint()
            destination.mkdir(mode=0o700)
            self.io.write_new(destination / "artifact.mcpb", payload)
            if (self.io.read_file(destination, "artifact.mcpb", ARTIFACT_LIMIT) != payload
                    or self.io.read_file(job_directory, "artifact.mcpb", ARTIFACT_LIMIT) != payload
                    or self.io.read_file(job_directory, "report.json", REPORT_LIMIT) != report_raw
                    or self.io.read_file(job_directory, "worker.json", REPORT_LIMIT) != settings
                    or self.io.read_file(job_directory, "request.json", REPORT_LIMIT) != wire(request)):
                raise ValueError()
            self.verify()
            checkpoint()
            receipt = {"schema_version": 1, "scope": RESULT_SCOPE, "request": request,
                       "host_profile_sha256": self.profile_sha, "staged_id": job_directory.name,
                       "primary": primary, "files": {"artifact.mcpb": request["artifact_sha256"]},
                       "report_sha256": sha(report_raw)}
            raw = wire(receipt)
            self.io.write_new(destination / "acquisition.json", raw)
            return {**receipt, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(raw)}
        except self.io.Error:
            raise
        except Exception:
            raise self.io.Error("mcpb_acquisition_unconfirmed", 409) from None


def _inventory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        with os.scandir(fd) as entries:
            result = set()
            for entry in entries:
                if len(result) >= 5 or not stat.S_ISREG(entry.stat(follow_symlinks=False).st_mode):
                    raise ValueError()
                result.add(entry.name)
            return result
    finally:
        os.close(fd)


def readback(io, request, job_directory):
    """Current retained-byte observation only; never import or run old assets."""
    try:
        request = copy.deepcopy(request)
        workspace, stage, job = Path(request["workspace"]), Path(request["staging_root"]), Path(job_directory)
        if (request["family"] != "selected-mcpb" or not REQUEST_ID.fullmatch(request["job_id"])
                or job != workspace / "acquisitions" / request["job_id"]):
            raise ValueError()
        io.canonical_directory(job)
        assets = workspace / "mcpb-acquisition"
        io.canonical_directory(assets)
        pins = request["host"]["assets"]
        _keys(pins, ("profile.json", "worker.py"))
        frozen = {name: io.read_file(assets, name, REPORT_LIMIT) for name in pins}
        if any(not _hash(expected) or sha(frozen[name]) != expected for name, expected in pins.items()):
            raise ValueError()
        profile = _profile(io.decode(frozen["profile.json"]))
        if pins["profile.json"] != request["host"]["profile_sha256"]:
            raise ValueError()
        args = _arguments(request["arguments"], profile)
        settings = wire({"request": args, "profile": profile, "directory": str(job)})
        if (io.read_file(job, "worker.json", REPORT_LIMIT) != settings
                or io.read_file(job, "request.json", REPORT_LIMIT) != wire(args)):
            raise ValueError()
        report_raw = io.read_file(job, "report.json", REPORT_LIMIT)
        primary = _report(io.decode(report_raw), args, profile)
        destination = stage / job.name
        io.canonical_directory(destination)
        raw = io.read_file(destination, "acquisition.json", REPORT_LIMIT)
        expected = {"schema_version": 1, "scope": RESULT_SCOPE, "request": args,
                    "host_profile_sha256": pins["profile.json"], "staged_id": job.name,
                    "primary": primary, "files": {"artifact.mcpb": args["artifact_sha256"]},
                    "report_sha256": sha(report_raw)}
        if raw != wire(expected):
            raise ValueError()
        names = _inventory(destination)
        optional = names & {"package.json", "activation.json", "configuration.json"}
        if names != {"artifact.mcpb", "acquisition.json"} | optional:
            raise ValueError()
        for name in optional:
            io.read_file(destination, name, 1024 * 1024)
        payload = io.read_file(destination, "artifact.mcpb", ARTIFACT_LIMIT)
        if (len(payload) != primary["byte_length"] or sha(payload) != args["artifact_sha256"]
                or payload != io.read_file(job, "artifact.mcpb", ARTIFACT_LIMIT)):
            raise ValueError()
        if (io.read_file(destination, "acquisition.json", REPORT_LIMIT) != raw
                or io.read_file(job, "worker.json", REPORT_LIMIT) != settings
                or io.read_file(job, "report.json", REPORT_LIMIT) != report_raw
                or io.read_file(job, "request.json", REPORT_LIMIT) != wire(args)
                or any(io.read_file(assets, name, REPORT_LIMIT) != data for name, data in frozen.items())):
            raise ValueError()
        return {**expected, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(raw)}
    except Exception:
        raise io.Error("mcpb_acquisition_readback_unconfirmed", 409) from None


def _decode(raw):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError()
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))


def _fetch_worker(path, expected):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError()
        raw = stream.read(REPORT_LIMIT + 1)
    if len(raw) > REPORT_LIMIT or not _hash(expected) or sha(raw) != expected:
        raise ValueError()
    settings = _decode(raw)
    _keys(settings, ("request", "profile", "directory"))
    profile = _profile(settings["profile"])
    directory = Path(settings["directory"])
    if not directory.is_absolute() or str(directory.resolve(strict=True)) != str(directory):
        raise ValueError()
    deadline = time.monotonic() + TIMEOUT
    def checkpoint():
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError()
        return remaining
    payload, report = fetch(settings["request"], profile, checkpoint)
    with (directory / "artifact.mcpb").open("xb") as stream:
        stream.write(payload)
    with (directory / "report.json").open("xb") as stream:
        stream.write(wire(report))


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4 or sys.argv[1] != "--fetch":
            raise ValueError()
        _fetch_worker(sys.argv[2], sys.argv[3])
    except Exception:
        print("mcpb_acquisition_worker_failed", file=sys.stderr)
        raise SystemExit(75) from None
