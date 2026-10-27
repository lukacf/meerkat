"""Acquire an exact public npm choice for the existing selected Node installer.

npm owns resolution; the pinned installer owns archive/closure validation.
The host trusts the whole pinned npm distribution and the public registry.
This scope is not a process, filesystem or network sandbox.
"""
from __future__ import annotations

import base64
import copy
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import ssl
import stat
import sys
import tarfile
import tempfile
import time
import unicodedata
import urllib.parse
import urllib.request

SCOPE = "trusted_npm_registry_acquisition"
RESULT_SCOPE = "retained_node_package_inputs"
NPM_VERSION = "12.2.0"
TARGETS = {"macos-arm64": ("darwin", "arm64", "arm64"),
           "linux-x86_64": ("linux", "x86_64", "x64")}
TIMEOUT = 120
FILE_LIMIT = 64 * 1024 * 1024
TOTAL_LIMIT = 512 * 1024 * 1024
TOOL_LIMIT = 128 * 1024 * 1024
DOCUMENT_LIMIT = 4 * 1024 * 1024
SEARCH_LIMIT = 20
SEARCH_BYTES = 256 * 1024
SEARCH_TIMEOUT = 30
SEARCH_QUERY_BYTES = 512
ARCHIVE_COUNT = 255
HEX = re.compile(r"[a-f0-9]{64}\Z")
REQUEST_ID = re.compile(r"[a-f0-9]{32}\Z")
PACKAGE = re.compile(r"(?:@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*\Z")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\Z")


class _UnsupportedNpmClosure(ValueError):
    """An intentional package-graph exclusion, not an uncertain failure."""


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def wire(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def _target(profile):
    target = profile.get("target", "macos-arm64")
    if not isinstance(target, str) or target not in TARGETS:
        raise ValueError()
    return target


def _conditional_lock(lock):
    return any(isinstance(row, dict) and (row.get("optional") or row.get("optionalDependencies")
               or any(key in row for key in ("os", "cpu", "libc")))
               for row in lock["packages"].values())


def _npm_target(runtime, node_sha):
    # This records the existing owned Node probe, not a caller platform choice.
    return {"node_sha256": node_sha, "node_version": runtime["version"], "node_abi": runtime["abi"],
            "platform": runtime["platform"], "arch": runtime["arch"], "libc": runtime["libc"]}


def _runtime_matches(runtime, target):
    platform, _, arch = TARGETS[target]
    return (isinstance(runtime, dict) and runtime.get("platform") == platform
            and runtime.get("arch") == arch and isinstance(runtime.get("version"), str)
            and re.fullmatch(r"v24\.(?:1[5-9]|[2-9][0-9]|[1-9][0-9]{2,})\.[0-9]+", runtime["version"]) is not None)


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
    """Observe retained package inputs without npm, imports, or network access.

    The caller verifies the durable request's digest and owns exclusive custody.
    This does not verify an installed runtime or renew the original authority.
    """
    try:
        request = copy.deepcopy(request)
        args = _arguments(io, request["arguments"])
        workspace, stage, job = Path(request["workspace"]), Path(request["staging_root"]), Path(job_directory)
        if (request["family"] != "selected-node" or not REQUEST_ID.fullmatch(request["job_id"])
                or job != workspace / "acquisitions" / request["job_id"]):
            raise ValueError()
        io.canonical_directory(job)
        assets = workspace / "node-acquisition"
        io.canonical_directory(assets)
        pins = request["host"]["assets"]
        io.keys(pins, ("profile.json", "npm.tgz", "installer.py", "worker.py"))
        for name, expected in pins.items():
            if not isinstance(expected, str) or not HEX.fullmatch(expected) or sha(io.read_file(assets, name, TOOL_LIMIT)) != expected:
                raise ValueError()
        profile = io.decode(io.read_file(assets, "profile.json", 65536))
        target = _target(profile)
        if (pins["profile.json"] != request["host"]["profile_sha256"] or profile["scope"] != SCOPE
                or profile["npm"]["sha256"] != pins["npm.tgz"]):
            raise ValueError()
        destination = stage / job.name
        io.canonical_directory(destination)
        raw = io.read_file(destination, "acquisition.json", DOCUMENT_LIMIT)
        receipt = io.decode(raw)
        io.keys(receipt, ("schema_version", "scope", "request", "host_profile_sha256", "npm_sha256", "npm_version",
                          "installer_sha256", "staged_id", "primary", "artifacts", "files", "runtime",
                          *(["target"] if target != "macos-arm64" else [])))
        if (type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1
                or receipt["scope"] != RESULT_SCOPE or receipt["request"] != args or receipt["staged_id"] != job.name
                or receipt["host_profile_sha256"] != pins["profile.json"] or receipt["npm_sha256"] != pins["npm.tgz"]
                or receipt["npm_version"] != NPM_VERSION or receipt["installer_sha256"] != pins["installer.py"]):
            raise ValueError()
        if target != "macos-arm64" and receipt["target"] != target:
            raise ValueError()
        artifacts = receipt["artifacts"]
        if not isinstance(artifacts, list) or not 1 <= len(artifacts) <= ARCHIVE_COUNT or artifacts[0] != receipt["primary"]:
            raise ValueError()
        for row in artifacts:
            io.keys(row, ("name", "version", "url", "integrity", "sha256"))
            registry_url(row["url"])
            checked_integrity(row["integrity"])
            if (not isinstance(row["sha256"], str) or not HEX.fullmatch(row["sha256"])
                    or not isinstance(row["name"], str) or len(row["name"]) > 214 or not PACKAGE.fullmatch(row["name"])
                    or not isinstance(row["version"], str) or len(row["version"]) > 128 or not VERSION.fullmatch(row["version"])):
                raise ValueError()
        primary = artifacts[0]
        if (primary["name"], primary["version"]) != (args["package"], args["version"]):
            raise ValueError()
        metadata = io.decode(io.read_file(job, "payloads/metadata", DOCUMENT_LIMIT))
        if (metadata["name"] != primary["name"] or metadata["version"] != primary["version"]
                or metadata["dist"]["tarball"] != primary["url"] or metadata["dist"]["integrity"] != primary["integrity"]):
            raise ValueError()
        project = wire({"name": "toolkit-acquisition", "version": "1.0.0", "private": True,
                        "dependencies": {args["package"]: "file:./artifact.tgz"}})
        lock = io.read_file(job, "project/package-lock.json", DOCUMENT_LIMIT)
        rows = _lock_rows(io, lock, args, primary)
        if rows != [{"url": row["url"], "integrity": row["integrity"]} for row in artifacts[1:]]:
            raise ValueError()
        locked = io.decode(lock)["packages"]
        for row in artifacts:
            matches = {(path.rsplit("node_modules/", 1)[-1], value.get("version"))
                       for path, value in locked.items() if path and value.get("integrity") == row["integrity"]}
            if matches != {(row["name"], row["version"])}:
                raise ValueError()
        proof_files = {"runtime.json": io.read_file(job, "runtime.json", 65536)}
        runtime = io.decode(proof_files["runtime.json"])
        if runtime != receipt["runtime"] or not _runtime_matches(runtime, target):
            raise ValueError()
        # Old macOS acquisitions predate the pre-network probe. New jobs bind
        # both observations and the two worker settings to the frozen profile.
        if target != "macos-arm64" or any((job / name).exists() or (job / name).is_symlink()
                                         for name in ("probe.json", "preflight.json")):
            proof_files.update({name: io.read_file(job, name, 65536)
                                for name in ("probe.json", "preflight.json", "resolve.json")})
            probe = io.decode(proof_files["probe.json"])
            io.keys(probe, ("node", "node_sha256", "npm", "directory", "target"))
            if (probe["target"] != target or probe["node_sha256"] != profile["node_sha256"]
                    or not isinstance(probe["node"], str) or not Path(probe["node"]).is_absolute()
                    or probe["npm"] != str(assets / "npm") or probe["directory"] != str(job)
                    or io.decode(proof_files["resolve.json"]) != probe
                    or io.decode(proof_files["preflight.json"]) != runtime):
                raise ValueError()
        expected = {"artifact.tgz": primary["sha256"], "closure/package-json": sha(project), "closure/package-lock": sha(lock)}
        expected.update({"closure/archive-" + row["sha256"]: row["sha256"] for row in artifacts[1:]})
        if _conditional_lock(io.decode(lock)):
            expected["closure/npm-target"] = sha(wire(_npm_target(runtime, profile["node_sha256"])))
        if receipt["files"] != expected:
            raise ValueError()
        root = _inventory(destination, 5)
        optional = {name: "file" for name in ("package.json", "activation.json") if name in root}
        if root != {"artifact.tgz": "file", "acquisition.json": "file", "closure": "directory", **optional}:
            raise ValueError()
        if _inventory(destination / "closure", ARCHIVE_COUNT + 2) != {name.split("/", 1)[1]: "file" for name in expected if name.startswith("closure/")}:
            raise ValueError()
        for name in optional:
            io.read_file(destination, name, 1024 * 1024)
        total = 0
        for name, digest in expected.items():
            payload = io.read_file(destination, name, FILE_LIMIT)
            total += len(payload)
            if total > TOTAL_LIMIT or sha(payload) != digest:
                raise ValueError()
            if name == "closure/package-json" and (payload != project or payload != io.read_file(job, "project/package.json", DOCUMENT_LIMIT)):
                raise ValueError()
            if name == "closure/package-lock" and payload != lock:
                raise ValueError()
        for index, row in enumerate(artifacts):
            name = "artifact.tgz" if index == 0 else "closure/archive-" + row["sha256"]
            source = "project/artifact.tgz" if index == 0 else "payloads/archive-" + str(index - 1)
            payload = io.read_file(destination, name, FILE_LIMIT)
            if integrity(payload) != row["integrity"] or payload != io.read_file(job, source, FILE_LIMIT):
                raise ValueError()
        if (io.read_file(destination, "acquisition.json", DOCUMENT_LIMIT) != raw
                or any(io.read_file(job, name, 65536) != value for name, value in proof_files.items())):
            raise ValueError()
        return {**receipt, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(raw)}
    except Exception:
        raise io.Error("node_acquisition_readback_unconfirmed", 409) from None


def integrity(raw):
    return "sha512-" + base64.b64encode(hashlib.sha512(raw).digest()).decode()


def checked_integrity(value):
    if not isinstance(value, str) or not value.startswith("sha512-"):
        raise ValueError("npm_integrity_required")
    raw = base64.b64decode(value[7:], validate=True)
    if len(raw) != 64 or base64.b64encode(raw).decode() != value[7:]:
        raise ValueError("npm_integrity_required")
    return value


def registry_url(value):
    if not isinstance(value, str) or len(value) > 4096 or any(ord(c) < 33 or ord(c) > 126 for c in value):
        raise ValueError("registry_url")
    url = urllib.parse.urlsplit(value)
    path = urllib.parse.unquote(url.path)
    if (url.scheme != "https" or url.netloc != "registry.npmjs.org" or url.query or url.fragment
            or not path.startswith("/") or "\\" in path
            or any(p in ("", ".", "..") for p in path.split("/")[1:])):
        raise ValueError("registry_url")
    return value


def tool_payloads(raw):
    """Inspect the host-trusted npm distribution without general tar extraction."""
    with gzip.GzipFile(fileobj=io.BytesIO(raw)) as stream:
        expanded = stream.read(TOOL_LIMIT + 1)
    if len(expanded) > TOOL_LIMIT:
        raise ValueError("npm_tool_size")
    files, names, total = {}, set(), 0
    with tarfile.open(fileobj=io.BytesIO(expanded), mode="r:") as archive:
        for index, member in enumerate(archive):
            if index >= 30000:
                raise ValueError("npm_tool_members")
            name = member.name.rstrip("/")
            parts = name.split("/")
            if (parts[0] != "package" or any(p in ("", ".", "..") for p in parts)
                    or "\\" in name or (not member.isdir() and not member.isfile())
                    or member.issparse() or name.casefold() in names):
                raise ValueError("npm_tool_path")
            names.add(name.casefold())
            if member.isdir():
                continue
            if len(parts) < 2 or member.size > FILE_LIMIT:
                raise ValueError("npm_tool_size")
            total += member.size
            if total > TOOL_LIMIT:
                raise ValueError("npm_tool_size")
            with archive.extractfile(member) as stream:
                value = stream.read(FILE_LIMIT + 1)
            if len(value) != member.size:
                raise ValueError("npm_tool_truncated")
            files["/".join(parts[1:])] = value
    metadata = json.loads(files["package.json"])
    if metadata.get("name") != "npm" or metadata.get("version") != NPM_VERSION or "bin/npm-cli.js" not in files:
        raise ValueError("npm_tool_version")
    for name in files:
        if any(str(p) in files for p in PurePosixPath(name).parents):
            raise ValueError("npm_tool_collision")
    return files


def _arguments(io, request):
    try:
        io.keys(request, ("request_id", "package", "version"))
        if (not isinstance(request["request_id"], str) or not REQUEST_ID.fullmatch(request["request_id"])
                or not isinstance(request["package"], str) or len(request["package"]) > 214
                or not PACKAGE.fullmatch(request["package"])
                or not isinstance(request["version"], str) or len(request["version"]) > 128
                or not VERSION.fullmatch(request["version"])):
            raise ValueError()
        prerelease = request["version"].split("+", 1)[0].partition("-")[2]
        if any(part.isdigit() and len(part) > 1 and part.startswith("0") for part in prerelease.split(".")):
            raise ValueError()
    except (ValueError, TypeError, KeyError, io.Error):
        raise io.Error("invalid_node_acquisition_arguments") from None
    return copy.deepcopy(request)


def search_query(request):
    if type(request) is not dict or set(request) != {"query"}:
        raise ValueError()
    query = request["query"]
    if (type(query) is not str or not query.strip() or query != query.strip()
            or len(query.encode("utf-8")) > SEARCH_QUERY_BYTES
            or any(unicodedata.category(char).startswith("C") for char in query)
            or any(term.startswith(("/", "-")) for term in query.split())):
        raise ValueError()
    return query


def _lock_rows(io, raw, request, primary):
    lock = io.decode(raw)
    if lock.get("lockfileVersion") != 3 or not isinstance(lock.get("packages"), dict):
        raise ValueError()
    rows, seen = [], {}
    primary_path = "node_modules/" + request["package"]
    for location, row in lock["packages"].items():
        if not isinstance(row, dict):
            raise ValueError()
        if any(row.get(k) for k in
            ("dev", "devOptional", "inBundle", "link", "inert")):
            raise _UnsupportedNpmClosure()
        if not location:
            continue
        expected = checked_integrity(row.get("integrity"))
        if location == primary_path:
            if (row.get("resolved") != "file:artifact.tgz" or expected != primary["integrity"]
                    or row.get("version") != request["version"]):
                raise ValueError()
            continue
        url = registry_url(row.get("resolved"))
        if expected == primary["integrity"]:
            raise ValueError()
        if expected in seen:
            if seen[expected] != url:
                raise ValueError()
            continue
        seen[expected] = url
        rows.append({"url": url, "integrity": expected})
    # The existing Node plan permits 256 flat closure inputs. Conditional
    # graphs reserve one of those slots for the exact target.
    if primary_path not in lock["packages"] or len(rows) + 1 > ARCHIVE_COUNT - int(_conditional_lock(lock)):
        raise ValueError()
    return rows


class NodeAcquisition:
    def __init__(self, io, profile_path, profile_sha, python_path, staging_root,
                 installer_path, installer_sha, node_path):
        self.io, self.workspace = io, None
        self.profile_sha, self.installer_sha = profile_sha, installer_sha
        try:
            self.profile_path, self.python = Path(profile_path), Path(python_path)
            self.node = Path(node_path)
            if not isinstance(profile_sha, str) or not HEX.fullmatch(profile_sha):
                raise ValueError()
            self.profile_raw = self.read_path(self.profile_path, 65536)
            if sha(self.profile_raw) != profile_sha:
                raise ValueError()
            self.profile = io.decode(self.profile_raw)
            io.keys(self.profile, ("schema_version", "scope", "python_sha256", "node_sha256", "npm",
                                   *(["allow_acquisition"] if "allow_acquisition" in self.profile else []),
                                   *(["target"] if "target" in self.profile else [])))
            self.allow_acquisition = self.profile.get("allow_acquisition", True)
            if type(self.allow_acquisition) is not bool:
                raise ValueError()
            self.staging = Path(staging_root) if self.allow_acquisition else None
            self.installer = Path(installer_path) if self.allow_acquisition else None
            self.target = _target(self.profile)
            io.keys(self.profile["npm"], ("path", "sha256"))
            if type(self.profile["schema_version"]) is not int or self.profile["schema_version"] != 1 or self.profile["scope"] != SCOPE:
                raise ValueError()
            self.npm = Path(self.profile["npm"]["path"])
            self.inputs = {}
            for path, expected in ((self.python, self.profile["python_sha256"]),
                                   (self.node, self.profile["node_sha256"]),
                                   (self.npm, self.profile["npm"]["sha256"]),
                                   *(([(self.installer, installer_sha)]) if self.allow_acquisition else [])):
                if not isinstance(expected, str) or not HEX.fullmatch(expected):
                    raise ValueError()
                raw = self.read_path(path, TOOL_LIMIT)
                if sha(raw) != expected:
                    raise ValueError()
                self.inputs[path] = raw
            if not os.access(self.node, os.X_OK) or not os.access(self.python, os.X_OK):
                raise ValueError()
            self.tool_files = tool_payloads(self.inputs[self.npm])
            self.worker_raw = self.read_path(Path(__file__).resolve(), DOCUMENT_LIMIT)
            if self.allow_acquisition:
                io.canonical_directory(self.staging)
        except (OSError, ValueError, TypeError, KeyError, AttributeError, tarfile.TarError, io.Error):
            raise io.Error("invalid_node_acquisition_profile") from None

    def read_path(self, path, limit):
        if not path.is_absolute() or str(path) != str(path.resolve(strict=True)):
            raise ValueError()
        return self.io.read_file(self.io.canonical_directory(path.parent), path.name, limit)

    def arguments(self, request):
        if not self.allow_acquisition:
            raise self.io.Error("node_acquisition_unavailable", 409)
        return _arguments(self.io, request)

    def search_summary(self):
        return {"available": True, "scope": "untrusted_public_npm_search_evidence",
                "registry": "https://registry.npmjs.org", "query_sent_publicly": True,
                "max_candidates": SEARCH_LIMIT, "max_query_bytes": SEARCH_QUERY_BYTES,
                "max_response_bytes": SEARCH_BYTES, "timeout_seconds": SEARCH_TIMEOUT}

    def search(self, request, runner):
        try:
            query = search_query(request)
        except (ValueError, UnicodeError):
            raise self.io.Error("invalid_node_search_arguments") from None
        self.verify()
        try:
            # Ephemeral npm config/cache cannot alter selected inputs or jobs.
            with tempfile.TemporaryDirectory(prefix="node-search-", dir=self.workspace / "tmp") as scratch:
                directory = Path(scratch)
                for name in ("home", "tmp", "cache", "project"):
                    (directory / name).mkdir(mode=0o700)
                raw = wire({"node": str(self.node), "node_sha256": sha(self.inputs[self.node]),
                            "npm": str(self.assets / "npm"), "directory": str(directory),
                            "target": self.target, "query": query})
                config = directory / "search.json"
                self.io.write_new(config, raw)
                code, response = runner.run_compose(
                    [str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                     "--search", str(config), sha(raw)],
                    output_limit=SEARCH_BYTES, timeout=SEARCH_TIMEOUT)
                if code or type(response) is not bytes or len(response) > SEARCH_BYTES:
                    raise ValueError()
                if not _runtime_matches(self.io.decode(self.io.read_file(directory, "runtime.json", 1024)), self.target):
                    raise ValueError()
                rows = self.io.decode(response)
                if type(rows) is not list or len(rows) > SEARCH_LIMIT:
                    raise ValueError()
                candidates = []
                for row in rows:
                    if type(row) is not dict:
                        raise ValueError()
                    selected = _arguments(self.io, {"request_id": "0" * 32,
                        "package": row.get("name"), "version": row.get("version")})
                    description = row.get("description", "")
                    if type(description) is not str:
                        raise ValueError()
                    description = "".join(char if not unicodedata.category(char).startswith("C") else " "
                                          for char in description)
                    description = description.encode("utf-8")[:2048].decode("utf-8", errors="ignore")
                    name, version = selected["package"], selected["version"]
                    package_url = urllib.parse.quote(name, safe="@/")
                    candidates.append({"name": name, "version": version, "description": description,
                        "sources": {"npm": "https://www.npmjs.com/package/" + package_url + "/v/" + version,
                                    "registry": "https://registry.npmjs.org/" + urllib.parse.quote(name, safe="@") + "/" + version}})
            self.verify()
            return {"scope": "untrusted_public_npm_search_evidence", "query": query,
                    "candidates": candidates, "selection_required": True,
                    "notice": "Registry order and descriptions are untrusted research evidence. The agent must choose an exact package/version and separately acquire, inspect, check and install it."}
        except (OSError, ValueError, TypeError, KeyError, RecursionError, self.io.Error):
            raise self.io.Error("node_search_failed", 409) from None

    def summary(self):
        return {"available": True, "scope": SCOPE, "staging_root": str(self.staging),
                "host_profile_sha256": self.profile_sha, "registry": "https://registry.npmjs.org",
                "npm_version": NPM_VERSION, "max_archives": ARCHIVE_COUNT,
                "max_archive_bytes": FILE_LIMIT, "max_total_bytes": TOTAL_LIMIT,
                "timeout_seconds": TIMEOUT, "target": "Node 24 " + ("macOS arm64" if self.target == "macos-arm64" else "Linux x86_64")}

    def bind(self, workspace):
        self.io.canonical_directory(workspace)
        if self.staging is not None and (workspace == self.staging or workspace in self.staging.parents or self.staging in workspace.parents):
            raise self.io.Error("invalid_node_acquisition_workspace")
        self.workspace, self.assets = workspace, workspace / "node-acquisition"
        self.assets.mkdir(mode=0o700)
        self.frozen = {"profile.json": self.profile_raw, "npm.tgz": self.inputs[self.npm], "worker.py": self.worker_raw}
        if self.allow_acquisition:
            self.frozen["installer.py"] = self.inputs[self.installer]
        self.frozen.update({"npm/" + name: raw for name, raw in self.tool_files.items()})
        for name, raw in self.frozen.items():
            path = self.assets / name
            path.parent.mkdir(parents=True, exist_ok=True)
            self.io.write_new(path, raw)
            path.chmod(0o400)

    def verify(self):
        try:
            if self.workspace is None or self.read_path(self.profile_path, 65536) != self.profile_raw:
                raise ValueError()
            for path, raw in self.inputs.items():
                if self.read_path(path, TOOL_LIMIT) != raw:
                    raise ValueError()
            entries = list(self.assets.rglob("*"))
            if any(p.is_symlink() for p in entries):
                raise ValueError()
            actual = {str(p.relative_to(self.assets)) for p in entries if not p.is_dir()}
            expected_dirs = {str(p) for name in self.frozen for p in PurePosixPath(name).parents if str(p) != "."}
            actual_dirs = {str(p.relative_to(self.assets)) for p in entries if p.is_dir()}
            if actual != set(self.frozen) or actual_dirs != expected_dirs:
                raise ValueError()
            for name, raw in self.frozen.items():
                if self.io.read_file(self.assets, name, TOOL_LIMIT) != raw:
                    raise ValueError()
            if self.allow_acquisition:
                self.io.canonical_directory(self.staging)
        except (OSError, ValueError, self.io.Error):
            raise self.io.Error("node_acquisition_host_changed", 409) from None

    def fetch(self, url, limit, directory, tag, checkpoint, run_callable, expected=None):
        registry_url(url)
        settings = wire({"url": url, "limit": limit, "destination": str(directory / "payloads" / tag),
                         "integrity": expected})
        config = directory / (tag + ".json")
        self.io.write_new(config, settings)
        code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                                "--fetch", str(config), sha(settings)],
                               capture=True, inspection_timeout=checkpoint())
        checkpoint()
        if code:
            raise self.io.Error("node_acquisition_download_failed", 409)
        raw = self.io.read_file(directory / "payloads", tag, limit)
        if expected is not None and integrity(raw) != checked_integrity(expected):
            raise ValueError()
        return raw

    def archive(self, owner, raw):
        result = owner["npm_archive"](raw)
        metadata = owner["unique_json"](result["files"]["package.json"])
        # Keep archive bytes intact. The pinned canonical owner selects the
        # target graph only after all lock rows and payloads have been retained.
        if ("npm-shrinkwrap.json" in result["files"]
                or any(metadata.get(k) for k in ("workspaces",))):
            raise _UnsupportedNpmClosure("unsupported_npm_closure")
        return result

    def lock_rows(self, raw, request, primary):
        return _lock_rows(self.io, raw, request, primary)

    def run(self, request, job_directory, run_callable, check_stop):
        request = self.arguments(request)
        self.verify()
        deadline = time.monotonic() + TIMEOUT
        def checkpoint():
            check_stop()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise self.io.Error("node_acquisition_timeout", 409)
            return remaining
        try:
            if job_directory.parent != self.workspace / "acquisitions" or not REQUEST_ID.fullmatch(job_directory.name):
                raise ValueError()
            self.io.canonical_directory(job_directory)
            destination = self.staging / job_directory.name
            if destination.exists() or destination.is_symlink():
                raise ValueError()
            for name in ("home", "tmp", "payloads", "project", "cache"):
                (job_directory / name).mkdir(mode=0o700)
            settings = wire({"node": str(self.node), "node_sha256": sha(self.inputs[self.node]),
                             "npm": str(self.assets / "npm"), "directory": str(job_directory), "target": self.target})
            self.io.write_new(job_directory / "probe.json", settings)
            code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                                    "--probe", str(job_directory / "probe.json"), sha(settings)],
                                   capture=True, inspection_timeout=checkpoint())
            checkpoint()
            if code:
                raise self.io.Error("node_acquisition_runtime_unsupported", 409)
            preflight_raw = self.io.read_file(job_directory, "preflight.json", 65536)
            preflight = self.io.decode(preflight_raw)
            if not _runtime_matches(preflight, self.target):
                raise self.io.Error("node_acquisition_runtime_unsupported", 409)
            owner = {"__name__": "toolkit_pinned_npm_inspector"}
            exec(compile(self.inputs[self.installer], str(self.assets / "installer.py"), "exec"), owner)
            metadata_url = "https://registry.npmjs.org/" + urllib.parse.quote(request["package"], safe="@") + "/" + urllib.parse.quote(request["version"], safe="")
            metadata_raw = self.fetch(metadata_url, DOCUMENT_LIMIT, job_directory, "metadata", checkpoint, run_callable)
            metadata = self.io.decode(metadata_raw)
            if metadata.get("name") != request["package"] or metadata.get("version") != request["version"]:
                raise ValueError()
            primary = {"name": request["package"], "version": request["version"],
                       "url": registry_url(metadata["dist"]["tarball"]),
                       "integrity": checked_integrity(metadata["dist"]["integrity"])}
            artifact = self.fetch(primary["url"], FILE_LIMIT, job_directory, "primary", checkpoint, run_callable, primary["integrity"])
            inspected = self.archive(owner, artifact)
            if (inspected["name"], inspected["version"]) != (request["package"], request["version"]):
                raise ValueError()
            primary["sha256"] = sha(artifact)
            project = wire({"name": "toolkit-acquisition", "version": "1.0.0", "private": True,
                            "dependencies": {request["package"]: "file:./artifact.tgz"}})
            self.io.write_new(job_directory / "project" / "package.json", project)
            self.io.write_new(job_directory / "project" / "artifact.tgz", artifact)
            self.io.write_new(job_directory / "resolve.json", settings)
            code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "worker.py"),
                                    "--resolve", str(job_directory / "resolve.json"), sha(settings)],
                                   capture=True, inspection_timeout=checkpoint())
            checkpoint()
            if code:
                raise self.io.Error("node_acquisition_resolution_failed", 409)
            lock_raw = self.io.read_file(job_directory / "project", "package-lock.json", DOCUMENT_LIMIT)
            runtime = self.io.decode(self.io.read_file(job_directory, "runtime.json", DOCUMENT_LIMIT))
            if runtime != preflight or not _runtime_matches(runtime, self.target):
                raise ValueError()
            rows = self.lock_rows(lock_raw, request, primary)
            files = {"artifact.tgz": artifact, "closure/package-json": project, "closure/package-lock": lock_raw}
            total = sum(map(len, files.values()))
            artifacts = [primary]
            for index, row in enumerate(rows):
                raw = self.fetch(row["url"], FILE_LIMIT, job_directory, "archive-" + str(index), checkpoint, run_callable, row["integrity"])
                total += len(raw)
                if total > TOTAL_LIMIT:
                    raise ValueError()
                parsed = self.archive(owner, raw)
                row = {**row, "name": parsed["name"], "version": parsed["version"], "sha256": sha(raw)}
                files["closure/archive-" + row["sha256"]] = raw
                artifacts.append(row)
            if _conditional_lock(self.io.decode(lock_raw)):
                files["closure/npm-target"] = wire(_npm_target(runtime, sha(self.inputs[self.node])))
                if total + len(files["closure/npm-target"]) > TOTAL_LIMIT:
                    raise ValueError()
            owner["npm_archive_closure"](files)
            if self.io.read_file(job_directory / "project", "package.json", DOCUMENT_LIMIT) != project:
                raise ValueError()
            self.verify()
            checkpoint()
            destination.mkdir(mode=0o700)
            (destination / "closure").mkdir(mode=0o700)
            for name, raw in files.items():
                checkpoint()
                self.io.write_new(destination / name, raw)
            for name, raw in files.items():
                if self.io.read_file(destination, name, FILE_LIMIT) != raw:
                    raise ValueError()
            self.verify()
            checkpoint()
            if (self.io.read_file(job_directory, "preflight.json", 65536) != preflight_raw
                    or self.io.decode(self.io.read_file(job_directory, "runtime.json", 65536)) != runtime
                    or any(self.io.read_file(job_directory, name, 65536) != settings
                           for name in ("probe.json", "resolve.json"))):
                raise ValueError()
            receipt = {"schema_version": 1, "scope": RESULT_SCOPE, "request": request,
                       "host_profile_sha256": self.profile_sha, "npm_sha256": sha(self.inputs[self.npm]),
                       "npm_version": NPM_VERSION, "installer_sha256": self.installer_sha,
                       "staged_id": job_directory.name, "primary": primary, "artifacts": artifacts,
                       "files": {name: sha(raw) for name, raw in files.items()}, "runtime": runtime}
            if self.target != "macos-arm64":
                receipt["target"] = self.target
            receipt_raw = wire(receipt)
            self.io.write_new(destination / "acquisition.json", receipt_raw)
            return {**receipt, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(receipt_raw)}
        except self.io.Error:
            raise
        except _UnsupportedNpmClosure:
            raise self.io.Error("node_acquisition_unsupported_closure", 409) from None
        except Exception:
            raise self.io.Error("node_acquisition_unconfirmed", 409) from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("registry_redirect")


def settings(path, expected):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError()
        raw = stream.read(65537)
    if len(raw) > 65536 or sha(raw) != expected:
        raise ValueError()
    return json.loads(raw)


def fetch_worker(value):
    url, limit = registry_url(value["url"]), value["limit"]
    if type(limit) is not int or not 0 < limit <= FILE_LIMIT:
        raise ValueError()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()))
    request = urllib.request.Request(url, headers={"Accept-Encoding": "identity"})
    with opener.open(request, timeout=10) as response:
        if response.status != 200 or response.geturl() != url or response.headers.get("Content-Encoding", "identity") != "identity":
            raise ValueError()
        length = response.headers.get("Content-Length")
        if length is not None and (not length.isdecimal() or int(length) > limit):
            raise ValueError()
        raw = bytearray()
        while True:
            chunk = response.read1(min(65536, limit + 1 - len(raw)))
            if not chunk:
                break
            raw.extend(chunk)
            if len(raw) > limit:
                raise ValueError()
        if length is not None and len(raw) != int(length):
            raise ValueError()
    if value["integrity"] is not None and integrity(raw) != checked_integrity(value["integrity"]):
        raise ValueError()
    with Path(value["destination"]).open("xb") as stream:
        stream.write(raw)


def runtime_worker(value, *, probe=False, search=False):
    import platform
    import resource
    import struct
    target = _target(value)
    system, machine, arch = TARGETS[target]
    if (sys.platform != system or platform.machine() != machine
            or target == "linux-x86_64" and struct.calcsize("P") != 8):
        raise ValueError()
    node, npm, directory = Path(value["node"]), Path(value["npm"]), Path(value["directory"])
    if sha(node.read_bytes()) != value["node_sha256"] or json.loads((npm / "package.json").read_bytes())["version"] != NPM_VERSION:
        raise ValueError()
    for kind, limit in ((resource.RLIMIT_FSIZE, FILE_LIMIT), (resource.RLIMIT_CPU, TIMEOUT), (resource.RLIMIT_CORE, 0)):
        _, hard = resource.getrlimit(kind)
        bound = limit if hard == resource.RLIM_INFINITY else min(limit, hard)
        resource.setrlimit(kind, (bound, bound))
    environment = {"PATH": "/usr/bin:/bin", "HOME": str(directory / "home"), "LANG": "C.UTF-8",
                   "TMPDIR": str(directory / "tmp"), "TMP": str(directory / "tmp"), "TEMP": str(directory / "tmp"),
                   "npm_config_userconfig": str(directory / "user.npmrc"),
                   "npm_config_globalconfig": str(directory / "global.npmrc"),
                   "npm_config_cache": str(directory / "cache")}
    output = "preflight.json" if probe else "runtime.json"
    bootstrap = ("const fs=require('node:fs');if(process.platform!==" + json.dumps(system)
        + "||process.arch!==" + json.dumps(arch)
        + "||Number(process.versions.node.split('.')[0])!==24||Number(process.versions.node.split('.')[1])<15)process.exit(73);"
        + "fs.writeFileSync('../" + output + "',JSON.stringify({platform:process.platform,arch:process.arch,version:process.version,abi:process.versions.modules,libc:process.platform==='linux'?{family:'glibc',version:process.report.getReport().header.glibcVersionRuntime}:null}),{flag:'wx'});")
    args = [str(node), "--max-old-space-size=1024", "-e", bootstrap]
    if not probe:
        for name in ("user.npmrc", "global.npmrc"):
            with (directory / name).open("xb") as stream:
                stream.write(b"# Host-owned empty configuration\n")
        args[-1] += "process.argv=[process.execPath,process.argv[1],...process.argv.slice(2)];require(process.argv[1]);"
        if search:
            query = search_query({"query": value["query"]})
            args += [str(npm / "bin/npm-cli.js"), "search", "--json", "--color=false",
                "--searchlimit=" + str(SEARCH_LIMIT), "--ignore-scripts", "--ignore-extension",
                "--no-audit", "--no-fund", "--registry=https://registry.npmjs.org",
                "--fetch-retries=0", "--fetch-timeout=10000", "--", query]
        else:
            args += [str(npm / "bin/npm-cli.js"),
            "install", "--package-lock-only", "--lockfile-version=3", "--ignore-scripts", "--ignore-extension", "--no-audit", "--no-fund",
            "--bin-links=false", "--allow-git=none", "--allow-remote=none", "--allow-directory=none", "--allow-file=root",
            "--registry=https://registry.npmjs.org", "--fetch-retries=0", "--fetch-timeout=10000"]
    os.chdir(directory / "project")
    os.execve(node, args, environment)


def resolve_worker(value):
    runtime_worker(value)


def probe_worker(value):
    runtime_worker(value, probe=True)


def search_worker(value):
    runtime_worker(value, search=True)


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4 or sys.argv[1] not in ("--fetch", "--resolve", "--probe", "--search"):
            raise ValueError()
        value = settings(sys.argv[2], sys.argv[3])
        {"--fetch": fetch_worker, "--resolve": resolve_worker, "--probe": probe_worker, "--search": search_worker}[sys.argv[1]](value)
    except Exception:
        print("node_acquisition_worker_failed", file=sys.stderr)
        raise SystemExit(75) from None
