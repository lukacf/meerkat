"""Retain explicit public native payloads over an existing exact npm acquisition.

The source owner supplies current same-workspace authority. This owner never
runs package code, changes the source receipt, or enables npm lifecycle scripts.
Host origin allowlists are not resolved-address egress enforcement or a sandbox.
"""
from __future__ import annotations

import copy
import gzip
import hashlib
import io as binary_io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
import time


SCOPE = "trusted_public_node_prebuilt_acquisition"
RESULT_SCOPE = "retained_node_prebuilt_inputs"
TIMEOUT = 120
FILE_LIMIT = 64 * 1024 * 1024
TOTAL_LIMIT = 512 * 1024 * 1024
TOOL_LIMIT = 128 * 1024 * 1024
DOCUMENT_LIMIT = 4 * 1024 * 1024
MANIFEST_LIMIT = 64 * 1024
HEX = re.compile(r"[a-f0-9]{64}\Z")
REQUEST_ID = re.compile(r"[a-f0-9]{32}\Z")
ASSET = re.compile(r"native-asset-([a-f0-9]{64})\Z")


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def wire(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def _keys(value, names):
    if type(value) is not dict or set(value) != set(names):
        raise ValueError()


def _hash(value):
    return type(value) is str and HEX.fullmatch(value) is not None


def _target(value):
    _keys(value, ("node_sha256", "node_version", "node_abi", "platform", "arch", "libc"))
    if (not _hash(value["node_sha256"]) or type(value["node_version"]) is not str
            or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", value["node_version"])
            or type(value["node_abi"]) is not str or not re.fullmatch(r"[1-9][0-9]{0,5}", value["node_abi"])
            or (value["platform"], value["arch"]) not in (("linux", "x64"), ("darwin", "arm64"))):
        raise ValueError()
    if value["platform"] == "linux":
        _keys(value["libc"], ("family", "version"))
        if (value["libc"]["family"] != "glibc" or type(value["libc"]["version"]) is not str
                or not re.fullmatch(r"[0-9]+\.[0-9]+", value["libc"]["version"])):
            raise ValueError()
    elif value["libc"] is not None:
        raise ValueError()
    return copy.deepcopy(value)


def _relative(value):
    if (type(value) is not str or not 1 <= len(value) <= 1024 or "\\" in value
            or any(ord(c) < 32 or ord(c) > 126 for c in value)
            or str(PurePosixPath(value)) != value or value.startswith("/")
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise ValueError()
    return value


def _http_namespace(raw):
    namespace = {"__name__": "toolkit_node_prebuilt_https"}
    exec(compile(raw, "toolkit_node_prebuilt_https", "exec"), namespace)
    return namespace


# Load current shipped owner code alongside this module once. Readback never
# reimports a mutable path or executes retained historical owner code. Live
# construction/verification additionally pin these exact bytes for the worker.
with Path(__file__).with_name("mcpb_acquisition.py").open("rb") as _shared:
    _HTTP_RAW = _shared.read(DOCUMENT_LIMIT + 1)
if len(_HTTP_RAW) > DOCUMENT_LIMIT:
    raise ValueError("node_prebuilt_shared_owner_too_large")
_HTTP = _http_namespace(_HTTP_RAW)


def _profile(value, http):
    _keys(value, ("schema_version", "scope", "python", "allowed_origins", "allowed_redirect_origins", "target"))
    if value["scope"] != SCOPE:
        raise ValueError()
    target = _target(value["target"])
    adapted = {key: copy.deepcopy(item) for key, item in value.items() if key != "target"}
    adapted["scope"] = http["SCOPE"]
    http["_profile"](adapted)
    return {**copy.deepcopy(value), "target": target}


def _fetch_profile(profile, http):
    return {**{key: value for key, value in profile.items() if key != "target"}, "scope": http["SCOPE"]}


def _arguments(request, profile, http):
    _keys(request, ("request_id", "source", "target", "payloads"))
    _keys(request["source"], ("job_id", "receipt_sha256"))
    if (type(request["request_id"]) is not str or not REQUEST_ID.fullmatch(request["request_id"])
            or type(request["source"]["job_id"]) is not str or not REQUEST_ID.fullmatch(request["source"]["job_id"])
            or not _hash(request["source"]["receipt_sha256"])
            or _target(request["target"]) != profile["target"]
            or type(request["payloads"]) is not list or not 1 <= len(request["payloads"]) <= 16):
        raise ValueError()
    assets, locations = set(), set()
    for row in request["payloads"]:
        _keys(row, ("package_location", "package_archive_sha256", "asset_id", "url", "files"))
        location = _relative(row["package_location"])
        if (not location.startswith("node_modules/") or not _hash(row["package_archive_sha256"])
                or type(row["asset_id"]) is not str or not ASSET.fullmatch(row["asset_id"])
                or row["asset_id"] in assets or location.casefold() in locations
                or http["artifact_url"](row["url"]) not in profile["allowed_origins"]
                or type(row["files"]) is not list or not 1 <= len(row["files"]) <= 64):
            raise ValueError()
        assets.add(row["asset_id"])
        locations.add(location.casefold())
        members, destinations = set(), set()
        for item in row["files"]:
            _keys(item, ("member", "destination"))
            member, destination = _relative(item["member"]), _relative(item["destination"])
            if ("node_modules" in destination.casefold().split("/")
                    or member.casefold() in members or destination.casefold() in destinations):
                raise ValueError()
            members.add(member.casefold())
            destinations.add(destination.casefold())
        if not any(item["destination"].endswith(".node") for item in row["files"]):
            raise ValueError()
    if len(wire(request)) > MANIFEST_LIMIT:
        raise ValueError()
    return copy.deepcopy(request)


def _manifest(request, payloads):
    return {"schema_version": 1, "scope": "selected_node_native_payloads",
            "source_receipt_sha256": request["source"]["receipt_sha256"],
            "target": request["target"], "payloads": payloads}


def _member_hashes(raw, selected):
    """Recompute selected member evidence without loading historical owner code.

    Full archive and installed graph semantics belong to the pinned installer.
    This bounded reader only hashes explicit regular members from exact bytes.
    """
    with gzip.GzipFile(fileobj=binary_io.BytesIO(raw)) as compressed:
        expanded = compressed.read(256 * 1024 * 1024 + 1)
    if len(expanded) > 256 * 1024 * 1024:
        raise ValueError()
    hashes, seen = {}, set()
    with tarfile.open(fileobj=binary_io.BytesIO(expanded), mode="r:") as archive:
        for index, member in enumerate(archive):
            if index >= 20000:
                raise ValueError()
            if member.name not in selected:
                continue
            if (member.name in seen or not member.isfile() or member.issparse()
                    or not 0 <= member.size <= FILE_LIMIT):
                raise ValueError()
            seen.add(member.name)
            with archive.extractfile(member) as stream:
                data = stream.read(FILE_LIMIT + 1)
            if len(data) != member.size:
                raise ValueError()
            hashes[member.name] = sha(data)
    if set(hashes) != set(selected):
        raise ValueError()
    return hashes


def _resolved_payload(row, hashes):
    return {**row, "files": [{**item, "sha256": hashes[item["member"]]} for item in row["files"]]}


def _source(io, raw, reference):
    value = io.decode(raw)
    if (len(raw) > DOCUMENT_LIMIT or sha(raw) != reference["receipt_sha256"]
            or type(value) is not dict or type(value.get("schema_version")) is not int
            or value["schema_version"] != 1 or value.get("scope") != "retained_node_package_inputs"
            or value.get("staged_id") != reference["job_id"] or type(value.get("files")) is not dict
            or not 3 <= len(value["files"]) <= 257):
        raise ValueError()
    for name, digest in value["files"].items():
        if (not _hash(digest) or name not in ("artifact.tgz", "closure/package-json", "closure/package-lock")
                and (not name.startswith("closure/archive-") or not _hash(name.removeprefix("closure/archive-")))):
            raise ValueError()
    return value


def _retained_files(io, root, expected, initial_bytes):
    files, total = {}, initial_bytes
    if total > TOTAL_LIMIT:
        raise ValueError()
    for name, expected_sha in expected.items():
        raw = io.read_file(root, name, FILE_LIMIT)
        total += len(raw)
        if total > TOTAL_LIMIT or sha(raw) != expected_sha:
            raise ValueError()
        files[name] = raw
    return files, total


def _inventory(path, limit):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        result = {}
        with os.scandir(fd) as entries:
            for entry in entries:
                mode = entry.stat(follow_symlinks=False).st_mode
                if len(result) >= limit or not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
                    raise ValueError()
                result[entry.name] = "directory" if stat.S_ISDIR(mode) else "file"
        return result
    finally:
        os.close(fd)


def _result(request, profile_sha, installer_sha, staged_id, source, files, downloads):
    return {"schema_version": 1, "scope": RESULT_SCOPE, "request": request,
            "host_profile_sha256": profile_sha, "installer_sha256": installer_sha,
            "staged_id": staged_id, "source_receipt_sha256": request["source"]["receipt_sha256"],
            "primary": source["primary"], "artifacts": source["artifacts"],
            "files": {name: sha(raw) for name, raw in files.items()},
            "native_target": request["target"], "downloads": downloads}


class NodePrebuiltAcquisition:
    def __init__(self, profile_path, profile_sha, staging_root, *, io, node_owner, source_reader, selected_node):
        self.io, self.workspace, self.profile_sha = io, None, profile_sha
        self.node_owner, self.source_reader = node_owner, source_reader
        try:
            if not callable(source_reader) or not node_owner.allow_acquisition:
                raise ValueError()
            self.profile_path, self.staging = Path(profile_path), Path(staging_root)
            self.profile_raw = self.read_path(self.profile_path, MANIFEST_LIMIT)
            self.fetch_path = Path(__file__).with_name("mcpb_acquisition.py").resolve()
            self.worker_path = Path(__file__).resolve()
            self.fetch_raw = self.read_path(self.fetch_path, DOCUMENT_LIMIT)
            self.worker_raw = self.read_path(self.worker_path, DOCUMENT_LIMIT)
            self.http = _HTTP
            if (self.fetch_raw != _HTTP_RAW or not _hash(profile_sha)
                    or sha(self.profile_raw) != profile_sha):
                raise ValueError()
            self.profile = _profile(io.decode(self.profile_raw), self.http)
            self.python, self.node, self.installer = node_owner.python, Path(selected_node), node_owner.installer
            self.inputs = {path: node_owner.inputs[path] for path in (self.python, self.installer)}
            self.inputs[self.node] = self.read_path(self.node, TOOL_LIMIT)
            self.installer_sha = sha(self.inputs[self.installer])
            if (self.profile["python"] != {"path": str(self.python), "sha256": sha(self.inputs[self.python])}
                    or self.profile["target"]["node_sha256"] != sha(self.inputs[self.node])
                    or self.staging != node_owner.staging or not os.access(self.node, os.X_OK)):
                raise ValueError()
            self.parser = {"__name__": "toolkit_pinned_node_prebuilt_inspector"}
            exec(compile(self.inputs[self.installer], str(self.installer), "exec"), self.parser)
            self.parser["checked_node_native_target"](self.profile["target"])
            io.canonical_directory(self.staging)
            self.frozen = {"profile.json": self.profile_raw, "installer.py": self.inputs[self.installer],
                           "fetch.py": self.fetch_raw, "worker.py": self.worker_raw}
        except (OSError, ValueError, TypeError, KeyError, AttributeError, io.Error):
            raise io.Error("invalid_node_prebuilt_acquisition_profile") from None

    def read_path(self, path, limit):
        if not path.is_absolute() or str(path) != str(path.resolve(strict=True)):
            raise ValueError()
        return self.io.read_file(self.io.canonical_directory(path.parent), path.name, limit)

    def arguments(self, request):
        try:
            return _arguments(request, self.profile, self.http)
        except (ValueError, TypeError, KeyError):
            raise self.io.Error("invalid_node_prebuilt_acquisition_arguments") from None

    def summary(self):
        return {"available": True, "scope": SCOPE, "staging_root": str(self.staging),
                "host_profile_sha256": self.profile_sha, "target": copy.deepcopy(self.profile["target"]),
                "target_scope": "host_selected_target_reobserved_before_acquisition",
                "allowed_origins": list(self.profile["allowed_origins"]),
                "allowed_redirect_origins": list(self.profile["allowed_redirect_origins"]),
                "max_payloads": 16, "max_files_per_payload": 64, "max_artifact_bytes": FILE_LIMIT,
                "max_total_bytes": TOTAL_LIMIT, "timeout_seconds": TIMEOUT}

    def bind(self, workspace):
        self.io.canonical_directory(workspace)
        if workspace == self.staging or workspace in self.staging.parents or self.staging in workspace.parents:
            raise self.io.Error("invalid_node_prebuilt_acquisition_workspace")
        self.workspace, self.assets = workspace, workspace / "node-prebuilt-acquisition"
        self.assets.mkdir(mode=0o700)
        for name, raw in self.frozen.items():
            self.io.write_new(self.assets / name, raw)
            (self.assets / name).chmod(0o400)

    def verify(self):
        try:
            self.node_owner.verify()
            if self.workspace is None or self.node_owner.workspace != self.workspace:
                raise ValueError()
            for path, raw in ((self.profile_path, self.profile_raw), (self.fetch_path, self.fetch_raw),
                              (self.worker_path, self.worker_raw), *self.inputs.items()):
                if self.read_path(path, TOOL_LIMIT) != raw:
                    raise ValueError()
            if _inventory(self.assets, 5) != {name: "file" for name in self.frozen}:
                raise ValueError()
            for name, raw in self.frozen.items():
                if self.io.read_file(self.assets, name, TOOL_LIMIT) != raw:
                    raise ValueError()
            self.io.canonical_directory(self.staging)
        except (OSError, ValueError, self.io.Error):
            raise self.io.Error("node_prebuilt_acquisition_host_changed", 409) from None

    def run(self, request, job_directory, run_callable, check_stop):
        request = self.arguments(request)
        deadline = time.monotonic() + TIMEOUT
        def checkpoint():
            check_stop()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise self.io.Error("node_prebuilt_acquisition_timeout", 409)
            return remaining
        try:
            self.verify()
            checkpoint()
            if (job_directory.parent != self.workspace / "acquisitions" or not REQUEST_ID.fullmatch(job_directory.name)
                    or job_directory.name == request["source"]["job_id"]):
                raise ValueError()
            self.io.canonical_directory(job_directory)
            if self.io.read_file(job_directory, "request.json", MANIFEST_LIMIT) != wire(request):
                raise ValueError()
            destination = self.staging / job_directory.name
            if destination.exists() or destination.is_symlink():
                raise ValueError()
            original = self.source_reader(request["source"])
            source_root = self.staging / request["source"]["job_id"]
            source_raw = self.io.read_file(source_root, "acquisition.json", DOCUMENT_LIMIT)
            source = _source(self.io, source_raw, request["source"])
            if original != {**source, "receipt_path": str(source_root / "acquisition.json"), "receipt_sha256": sha(source_raw)}:
                raise ValueError()
            files, total = _retained_files(self.io, source_root, source["files"], len(source_raw))
            self.parser["npm_archive_closure"](files)
            checkpoint()
            code, output = run_callable([str(self.node), "-p", self.parser["NODE_NATIVE_TARGET_EXPRESSION"]],
                                       capture=True, inspection_timeout=checkpoint())
            checkpoint()
            if code or type(output) is not bytes or len(output) > MANIFEST_LIMIT:
                raise self.io.Error("node_prebuilt_runtime_unsupported", 409)
            observed = self.io.decode(output)
            if type(observed) is not dict or "node_sha256" in observed:
                raise ValueError()
            observed["node_sha256"] = sha(self.inputs[self.node])
            self.parser["checked_node_native_target"](observed)
            if observed != request["target"]:
                raise self.io.Error("node_prebuilt_runtime_unsupported", 409)
            self.verify()
            self.io.write_new(job_directory / "native-target.json", wire(observed))
            files["closure/native-source-receipt"] = source_raw
            downloads, payload_rows = [], []
            if len(files) + 1 + len(request["payloads"]) > 257 or total > TOTAL_LIMIT:
                raise ValueError()
            proofs = {"request.json": wire(request), "native-target.json": wire(observed)}
            for row in request["payloads"]:
                checkpoint()
                directory = job_directory / row["asset_id"]
                directory.mkdir(mode=0o700)
                fetch_request = {"request_id": request["request_id"], "url": row["url"],
                                 "artifact_sha256": ASSET.fullmatch(row["asset_id"]).group(1)}
                profile = _fetch_profile(self.profile, self.http)
                settings = wire({"request": fetch_request, "profile": profile, "directory": str(directory)})
                self.io.write_new(directory / "worker.json", settings)
                code, _ = run_callable([str(self.python), "-I", "-S", "-B", str(self.assets / "fetch.py"),
                    "--fetch", str(directory / "worker.json"), sha(settings)],
                    capture=True, inspection_timeout=checkpoint())
                checkpoint()
                if code:
                    raise self.io.Error("node_prebuilt_download_failed", 409)
                report_raw = self.io.read_file(directory, "report.json", MANIFEST_LIMIT)
                report = self.io.decode(report_raw)
                primary = self.http["_report"](report, fetch_request, profile)
                payload = self.io.read_file(directory, "artifact.mcpb", FILE_LIMIT)
                if (sha(payload) != fetch_request["artifact_sha256"] or len(payload) != primary["byte_length"]
                        or self.io.read_file(directory, "worker.json", MANIFEST_LIMIT) != settings):
                    raise ValueError()
                total += len(payload)
                if total > TOTAL_LIMIT:
                    raise ValueError()
                files["closure/" + row["asset_id"]] = payload
                downloads.append({"asset_id": row["asset_id"], **primary})
                archive = self.parser["node_native_archive"](payload)
                hashes = {item["member"]: sha(archive[item["member"]]) for item in row["files"]}
                if _member_hashes(payload, hashes) != hashes:
                    raise ValueError()
                payload_rows.append(_resolved_payload(row, hashes))
                proofs.update({row["asset_id"] + "/worker.json": settings,
                               row["asset_id"] + "/report.json": report_raw,
                               row["asset_id"] + "/artifact.mcpb": payload})
            files["closure/native-payloads"] = wire(_manifest(request, payload_rows))
            if len(files["closure/native-payloads"]) > MANIFEST_LIMIT or sum(map(len, files.values())) > TOTAL_LIMIT:
                raise ValueError()
            self.parser["npm_archive_closure"](files)
            checkpoint()
            if self.source_reader(request["source"]) != original:
                raise ValueError()
            self.verify()
            destination.mkdir(mode=0o700)
            (destination / "closure").mkdir(mode=0o700)
            for name, raw in files.items():
                checkpoint()
                self.io.write_new(destination / name, raw)
            for name, raw in files.items():
                if self.io.read_file(destination, name, FILE_LIMIT) != raw:
                    raise ValueError()
            receipt = _result(request, self.profile_sha, self.installer_sha, job_directory.name, source, files, downloads)
            if (any(self.io.read_file(job_directory, name, FILE_LIMIT) != data for name, data in proofs.items())
                    or self.source_reader(request["source"]) != original):
                raise ValueError()
            self.verify()
            checkpoint()
            raw = wire(receipt)
            if len(raw) > DOCUMENT_LIMIT:
                raise ValueError()
            self.io.write_new(destination / "acquisition.json", raw)
            return {**receipt, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(raw)}
        except self.io.Error:
            raise
        except Exception:
            raise self.io.Error("node_prebuilt_acquisition_unconfirmed", 409) from None


def readback(io, request, job_directory):
    """Observe retained derived bytes only, without executing historical code."""
    try:
        request = copy.deepcopy(request)
        workspace, stage, job = Path(request["workspace"]), Path(request["staging_root"]), Path(job_directory)
        if (request["family"] != "selected-node-prebuilt" or not REQUEST_ID.fullmatch(request["job_id"])
                or job != workspace / "acquisitions" / request["job_id"]):
            raise ValueError()
        io.canonical_directory(job)
        assets = workspace / "node-prebuilt-acquisition"
        pins = request["host"]["assets"]
        _keys(pins, ("profile.json", "installer.py", "fetch.py", "worker.py"))
        frozen = {name: io.read_file(assets, name, TOOL_LIMIT) for name in pins}
        if any(not _hash(expected) or sha(frozen[name]) != expected for name, expected in pins.items()):
            raise ValueError()
        http = _HTTP
        profile = _profile(io.decode(frozen["profile.json"]), http)
        if pins["profile.json"] != request["host"]["profile_sha256"]:
            raise ValueError()
        args = _arguments(request["arguments"], profile, http)
        if (io.read_file(job, "request.json", MANIFEST_LIMIT) != wire(args)
                or io.read_file(job, "native-target.json", MANIFEST_LIMIT) != wire(args["target"])):
            raise ValueError()
        destination = stage / job.name
        io.canonical_directory(destination)
        raw = io.read_file(destination, "acquisition.json", DOCUMENT_LIMIT)
        source_raw = io.read_file(destination, "closure/native-source-receipt", DOCUMENT_LIMIT)
        source = _source(io, source_raw, args["source"])
        manifest_raw = io.read_file(destination, "closure/native-payloads", MANIFEST_LIMIT)
        files, total = _retained_files(io, destination, source["files"], len(source_raw) + len(manifest_raw))
        files["closure/native-source-receipt"] = source_raw
        files["closure/native-payloads"] = manifest_raw
        downloads, payload_rows, fetch_profile = [], [], _fetch_profile(profile, http)
        proofs = {"request.json": wire(args), "native-target.json": wire(args["target"])}
        for row in args["payloads"]:
            directory = job / row["asset_id"]
            fetch_request = {"request_id": args["request_id"], "url": row["url"],
                             "artifact_sha256": ASSET.fullmatch(row["asset_id"]).group(1)}
            settings = wire({"request": fetch_request, "profile": fetch_profile, "directory": str(directory)})
            report_raw = io.read_file(directory, "report.json", MANIFEST_LIMIT)
            report = io.decode(report_raw)
            primary = http["_report"](report, fetch_request, fetch_profile)
            payload = io.read_file(destination, "closure/" + row["asset_id"], FILE_LIMIT)
            if (sha(payload) != fetch_request["artifact_sha256"] or len(payload) != primary["byte_length"]
                    or io.read_file(directory, "artifact.mcpb", FILE_LIMIT) != payload
                    or io.read_file(directory, "worker.json", MANIFEST_LIMIT) != settings):
                raise ValueError()
            total += len(payload)
            if total > TOTAL_LIMIT:
                raise ValueError()
            files["closure/" + row["asset_id"]] = payload
            downloads.append({"asset_id": row["asset_id"], **primary})
            hashes = _member_hashes(payload, [item["member"] for item in row["files"]])
            payload_rows.append(_resolved_payload(row, hashes))
            proofs.update({row["asset_id"] + "/worker.json": settings,
                           row["asset_id"] + "/report.json": report_raw,
                           row["asset_id"] + "/artifact.mcpb": payload})
        if files["closure/native-payloads"] != wire(_manifest(args, payload_rows)):
            raise ValueError()
        if len(files) > 257 or sum(map(len, files.values())) > TOTAL_LIMIT:
            raise ValueError()
        root = _inventory(destination, 5)
        optional = {name: "file" for name in ("package.json", "activation.json") if name in root}
        if (root != {"artifact.tgz": "file", "acquisition.json": "file", "closure": "directory", **optional}
                or _inventory(destination / "closure", 257) !=
                    {name.split("/", 1)[1]: "file" for name in files if name.startswith("closure/")}):
            raise ValueError()
        for name in optional:
            io.read_file(destination, name, 1024 * 1024)
        expected = _result(args, pins["profile.json"], pins["installer.py"], job.name, source, files, downloads)
        if raw != wire(expected):
            raise ValueError()
        if (io.read_file(destination, "acquisition.json", DOCUMENT_LIMIT) != raw
                or any(io.read_file(destination, name, FILE_LIMIT) != data for name, data in files.items())
                or any(io.read_file(assets, name, TOOL_LIMIT) != data for name, data in frozen.items())
                or any(io.read_file(job, name, FILE_LIMIT) != data for name, data in proofs.items())):
            raise ValueError()
        return {**expected, "receipt_path": str(destination / "acquisition.json"), "receipt_sha256": sha(raw)}
    except Exception:
        raise io.Error("node_prebuilt_acquisition_readback_unconfirmed", 409) from None
