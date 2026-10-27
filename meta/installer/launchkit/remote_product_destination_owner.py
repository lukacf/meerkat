"""Stopped destination custody of accepted source and relocated product inputs.

This private intake stages bytes, not an installation or completed native setup.
The original package selection stays sealed. A later trusted setup owner uses
the separately checked local artifacts under its actual retained project lease.
No transferred application, SDK, installer or native program executes here.
"""
from __future__ import annotations

import asyncio
from contextlib import contextmanager
from copy import deepcopy
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import stat
import sys
import uuid
import zipfile

from . import remote_product_artifact_receiver as receiver
from .remote_product_artifacts import (_current, _decode, _joined, _json, _require,
    ProductArtifactCustody, product_selection_bytes, relocate_product_target)
from .runtime_candidates import _Inputs, _fingerprint, ArtifactFile
from .runtime_selection import (MetaToolkitRuntimeTarget, parse_candidate_selection,
    selected_runtime_artifacts, selected_runtime_marker, selected_runtime_platform,
    selected_runtime_version)
from .templates.storage_setup import project_lock


MAX_APPLICATION_BYTES = 64 * 1024 * 1024
SOURCE_DIRECTORY = ".mobkit/forge-product-source"
_CONTEXT_FIELDS = {"schema", "deployment_id", "guide_id", "build_id", "guide_version",
    "source_digest", "package_sha256", "package_size", "host", "artifact_transaction_id",
    "artifact_manifest_sha256", "artifact_receiver_sha256", "artifact_selection_sha256", "owner_sha256"}


def _gcp_disposition(value):
    """Retain original cloud identity as source data, never VM authority."""
    _require(type(value) is dict and set(value) == {
            "target", "project", "zone", "instance", "instance_id", "operation_id", "binding"}
        and value["target"] == "gcp"
        and isinstance(value["project"], str)
        and re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", value["project"])
        and all(isinstance(value[name], str)
            and re.fullmatch(r"[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?", value[name])
            for name in ("zone", "instance"))
        and isinstance(value["instance_id"], str)
        and re.fullmatch(r"[1-9][0-9]{0,19}", value["instance_id"])
        and int(value["instance_id"]) <= 2**64 - 1
        and isinstance(value["operation_id"], str)
        and str(uuid.UUID(value["operation_id"])) == value["operation_id"],
        "The original GCP source disposition is invalid.")
    binding = value["binding"]
    _require(type(binding) is dict and set(binding) == {
            "guide_id", "guide_version", "build_id", "blueprint_hash", "source_digest", "package_sha256"}
        and type(binding["guide_version"]) is int and 0 < binding["guide_version"] <= 10**12
        and all(isinstance(binding[key], str) and str(uuid.UUID(binding[key])) == binding[key]
            for key in ("guide_id", "build_id"))
        and all(isinstance(binding[key], str) and re.fullmatch(r"[a-f0-9]{64}", binding[key])
            for key in ("blueprint_hash", "source_digest", "package_sha256")),
        "The original cloud creation source binding is invalid.")
    return _decode(_json(value))


def _context(value, owner_source, *, observe_host=True):
    _require(type(value) is dict and type(value.get("schema")) is int
        and (value["schema"] == 1 and set(value) == _CONTEXT_FIELDS
            or value["schema"] == 2 and set(value) == _CONTEXT_FIELDS | {"source_disposition"}),
        "The original product source intake is invalid.")
    if value["schema"] == 2:
        disposition = _gcp_disposition(value["source_disposition"])
        _require(all(disposition["binding"][key] == value[key]
            for key in ("guide_id", "build_id", "source_digest", "package_sha256")),
            "The original cloud creation belongs to another accepted application source.")
    for name in ("deployment_id", "guide_id", "build_id", "artifact_transaction_id"):
        item = value[name]
        _require(isinstance(item, str) and str(uuid.UUID(item)) == item,
            "The original product source identity is invalid.")
    for name in ("source_digest", "package_sha256", "artifact_manifest_sha256",
            "artifact_receiver_sha256", "artifact_selection_sha256", "owner_sha256"):
        _require(isinstance(value[name], str) and re.fullmatch(r"[a-f0-9]{64}", value[name]),
            "The original product source byte binding is invalid.")
    _require(type(value["guide_version"]) is int and 0 < value["guide_version"] <= 10**12
        and type(value["package_size"]) is int and 0 < value["package_size"] <= MAX_APPLICATION_BYTES
        and type(owner_source) is bytes and owner_source
        and hashlib.sha256(owner_source).hexdigest() == value["owner_sha256"],
        "The original accepted package or frozen destination owner changed.")
    host = value["host"]
    _require(type(host) is dict and set(host) == {"uid", "home", "destination"}
        and type(host["uid"]) is int and host["uid"] >= 0
        and all(isinstance(host[name], str) and 0 < len(host[name]) <= 4096
            and "\0" not in host[name] and Path(host[name]).is_absolute()
            and str(Path(host[name])) == host[name] and ".." not in Path(host[name]).parts
            for name in ("home", "destination"))
        and host["destination"] not in {"/", host["home"]},
        "The original observed product source destination is invalid.")
    if observe_host:
        receiver._observed_host(host)
    return _decode(_json(value))


def _platform():
    return {("Linux", "x86_64"): "x86_64-unknown-linux-gnu"}.get(
        (platform.system(), platform.machine()), "unsupported")


def _package_source(payload, context, origin, *, check_platform=True):
    """Private intake reader, never an exception to ordinary remote Install."""
    _require(type(payload) is bytes and len(payload) == context["package_size"]
        and len(payload) <= MAX_APPLICATION_BYTES
        and hashlib.sha256(payload).hexdigest() == context["package_sha256"],
        "The original accepted application archive changed.")
    files, folded, size = {}, set(), 0
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        for item in archive.infolist():
            name, mode = item.filename, item.external_attr >> 16
            parts = name.split("/")
            _require(name and not PurePosixPath(name).is_absolute() and "\\" not in name
                and all(part not in {"", ".", ".."} for part in parts)
                and not any(ord(char) < 32 or ord(char) == 127 for char in name)
                and name not in files and name.casefold() not in folded
                and stat.S_IFMT(mode) in {0, stat.S_IFREG} and not item.is_dir()
                and parts[0] not in {".venv", ".bin", ".mobkit", "data", ".git"}
                and parts[-1] != ".env" and "native-team-source.json" != name,
                "The accepted application archive contains an unsafe source entry.")
            size += item.file_size
            _require(size <= MAX_APPLICATION_BYTES and len(files) < 2048,
                "The accepted application archive exceeded its original source bound.")
            content = archive.read(item)
            _require(len(content) == item.file_size, "The accepted application source is incomplete.")
            files[name] = (content, 0o755 if mode & 0o111 else 0o644)
            folded.add(name.casefold())
    required = {"deployment.json", "implementation-report.json", "runtime-candidate.json",
        "app.py", "install.sh", "config/mob.toml", "application/tool-policy.json",
        "application/configuration.json", "application/console.json", "config/access.toml", "SHA256SUMS"}
    _require(required <= set(files), "The accepted product application source is incomplete.")
    declared = {}
    for row in files["SHA256SUMS"][0].decode("utf-8").splitlines():
        match = re.fullmatch(r"([a-f0-9]{64})  (.+)", row)
        _require(match is not None and match[2] not in declared,
            "The accepted application source manifest is invalid.")
        declared[match[2]] = match[1]
    _require(declared == {name: hashlib.sha256(content).hexdigest()
        for name, (content, _) in files.items() if name != "SHA256SUMS"},
        "The accepted application source manifest differs from its complete bytes.")
    selection = _decode(files["runtime-candidate.json"][0])
    report = _decode(files["implementation-report.json"][0])
    definition = _decode(files["deployment.json"][0])
    _require(type(selection) is dict and parse_candidate_selection(selection) == origin
        and type(report) is dict and report.get("build_id") == context["build_id"]
        and report.get("guide_version") == context["guide_version"]
        and report.get("verification", {}).get("digest") == context["source_digest"]
        and report.get("framework", {}).get("runtime_candidate") == selection,
        "The accepted application differs from the original implementation-owned runtime source.")
    disposition = context.get("source_disposition")
    deployment_target = "gcp" if disposition is not None else "linux"
    if disposition is not None:
        disposition = _gcp_disposition(disposition)
        blueprint = _decode(files["blueprint.json"][0]) if "blueprint.json" in files else None
        _require(type(definition) is dict and type(blueprint) is dict and blueprint.get("target") == "gcp"
            and type(blueprint.get("gcp")) is dict
            and all(blueprint["gcp"].get(key) == disposition[key] for key in ("project", "zone"))
            and definition.get("slug") == disposition["instance"]
            and report.get("blueprint_hash") == disposition["binding"]["blueprint_hash"]
            and selected_runtime_platform(origin) == "x86_64-unknown-linux-gnu",
            "The accepted cloud plan differs from its original verified VM source disposition.")
    _require(type(definition) is dict and definition.get("schemaVersion") == 2
        and definition.get("target") == deployment_target
        and re.fullmatch(r"[a-z][a-z0-9-]{1,39}", definition.get("slug", ""))
        and definition.get("mobkitVersion") == selected_runtime_version(origin)
        and definition.get("runtimeCandidate") == selected_runtime_marker(origin)
        and (not check_platform or selected_runtime_platform(origin) == _platform()),
        "The accepted product source does not select this actual supported destination target.")
    return files, selection


def _artifact_custody(context, retained=None):
    """Read completed original artifact custody without replaying receiver finish."""
    home = Path(context["host"]["home"])
    store = home / ".local/state/meta-forge/product-artifacts" / context["artifact_transaction_id"]
    inputs = _Inputs(store)
    def read(name, maximum=32 * 1024 * 1024, sha256=None, size=None):
        if sha256 is None:
            with inputs.open(store / name, maximum) as (handle, _):
                sha256 = hashlib.sha256(handle.read(maximum + 1)).hexdigest()
        pin, content = inputs.read(store / name, sha256, maximum, size=size)
        _require(not pin.path.stat().st_mode & 0o077, "The original artifact observation is not private.")
        return content
    native_receiver = read("receiver.py", sha256=context["artifact_receiver_sha256"])
    original = _decode(read("owner.json"))
    receiver._context(original, native_receiver)
    _require(original["transaction_id"] == context["artifact_transaction_id"]
        and original["guide_id"] == context["guide_id"] and original["build_id"] == context["build_id"]
        and original["source_digest"] == context["source_digest"] and original["host"] == context["host"]
        and original["manifest_sha256"] == context["artifact_manifest_sha256"],
        "The completed product artifacts belong to another original application or destination.")
    manifest = read("manifest.json", sha256=original["manifest_sha256"], size=original["manifest_size"])
    from .remote_product_artifacts import parse_product_artifact_manifest
    origin, rows, _ = parse_product_artifact_manifest(manifest)
    relocated = relocate_product_target(manifest, store / "inputs")
    selection = product_selection_bytes(relocated)
    _require(hashlib.sha256(selection).hexdigest() == context["artifact_selection_sha256"]
        and read("selection.json", sha256=context["artifact_selection_sha256"], size=len(selection)) == selection,
        "The original relocated product selection changed.")
    expected = {"transaction_id": context["artifact_transaction_id"], "host": context["host"],
        "manifest_sha256": context["artifact_manifest_sha256"],
        "receiver_sha256": context["artifact_receiver_sha256"], "status": "runtime-artifacts-transferred",
        "artifact_root": str(store / "inputs"), "selection_sha256": context["artifact_selection_sha256"]}
    _require(read("custody.json", receiver.MAX_RESPONSE_BYTES) == _json(expected),
        "The original complete artifact observation is absent or changed.")
    artifacts = selected_runtime_artifacts(relocated, retained=retained)
    observed = {str(pin.path.relative_to(relocated.artifact_root)):
        {"name": str(pin.path.relative_to(relocated.artifact_root)), "size": pin.size, "sha256": pin.sha256}
        for pin in artifacts.evidence}
    _require(len(observed) == len(artifacts.evidence) and observed == {row["name"]: row for row in rows},
        "The complete local product closure differs from its original manifest.")
    receiver._inventory(relocated.artifact_root, observed)
    inputs.finish()
    return origin, relocated, artifacts, inputs


@contextmanager
def _intake(context, owner_source, *, create=False):
    home = Path(context["host"]["home"])
    store = home / ".local/state/meta-forge/product-sources" / context["deployment_id"]
    if create:
        receiver._directory(store, home)
        receiver._directory(store / "partial", home)
    receiver._path(str(store))
    _require(store.is_dir() and store.stat().st_uid == os.getuid() and not store.stat().st_mode & 0o077,
        "The original source intake is not privately owned.")
    lock_path = store / "intake.lock"
    descriptor = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    with os.fdopen(descriptor, "r+b") as lock:
        info = os.fstat(lock.fileno())
        _require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1
            and not info.st_mode & 0o077 and _fingerprint(info) == _fingerprint(lock_path.lstat()),
            "The original source intake lock is unsafe.")
        fcntl.flock(lock, fcntl.LOCK_EX)
        if create:
            receiver._publish(store / "owner.json", _json(context))
            receiver._publish(store / "source-owner.py", owner_source)
        _, retained = receiver._read(store / "owner.json", _json(context), keep=False)
        _, frozen = receiver._read(store / "source-owner.py", owner_source, keep=False)
        directory_identity = _fingerprint(store.stat())[:2]
        def current():
            receiver._observed_host(context["host"])
            receiver._path(str(store))
            _require(_fingerprint(store.stat())[:2] == directory_identity
                and _fingerprint(os.fstat(lock.fileno())) == _fingerprint(lock_path.lstat()),
                "The original source intake directory or lock changed.")
            retained.finish()
            frozen.finish()
        current()
        yield store, current
        current()


def _source_owner(context):
    return {"schema": 1, "scope": "staged-product-application-source",
        "context": context, "origin": "runtime-candidate.json",
        "relocated_selection_sha256": context["artifact_selection_sha256"]}


def _result(context):
    return {"deployment_id": context["deployment_id"], "host": context["host"],
        "package_sha256": context["package_sha256"], "owner_sha256": context["owner_sha256"],
        "artifact_transaction_id": context["artifact_transaction_id"],
        "artifact_selection_sha256": context["artifact_selection_sha256"], "status": "product-source-staged"}


def _staged_inputs(context, store, *, owner_source, installed_view=None, retained=None):
    original, relocated, artifacts, evidence = _artifact_custody(context, retained)
    inputs = _Inputs(store)
    _, payload = inputs.read(store / "package.zip", context["package_sha256"], MAX_APPLICATION_BYTES,
        size=context["package_size"])
    for name, content in (("owner.json", _json(context)), ("source-owner.py", owner_source),
            ("entry.json", _json({"schema": 1, "context": context,
                "scope": "original-stopped-source-staging-attempt"})),
            ("staged.json", _json(_result(context)))):
        # During initial publication staged.json does not exist yet. The
        # continuing carrier always observes its already completed record.
        if name == "staged.json" and not (store / name).exists():
            continue
        pin, _ = inputs.read(store / name, hashlib.sha256(content).hexdigest(), len(content),
            size=len(content), keep=False)
        _require(not pin.path.stat().st_mode & 0o077, "The retained original source intake is not private.")
    files, selection = _package_source(payload, context, original)
    root = Path(context["host"]["destination"])
    receiver._path(str(root))
    _require(root.is_dir() and root.stat().st_uid == os.getuid() and not root.stat().st_mode & 0o022,
        "The original staged project is unsafe.")
    installed = _Inputs(root)
    marker = _json(_source_owner(context))
    marker_pin, _ = installed.read(root / SOURCE_DIRECTORY / "owner.json", hashlib.sha256(marker).hexdigest(), len(marker),
        size=len(marker), keep=False)
    local_pin, local = installed.read(root / SOURCE_DIRECTORY / "relocated-selection.json",
        context["artifact_selection_sha256"], 32 * 1024 * 1024)
    _require(not marker_pin.path.stat().st_mode & 0o077 and not local_pin.path.stat().st_mode & 0o077,
        "The retained staged source owner and artifact selection are not private.")
    _require(local == product_selection_bytes(relocated), "The staged local product selection changed.")
    for name, (content, mode) in files.items():
        path = installed.path(root / name)
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as handle:
            info = os.fstat(handle.fileno())
            _require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1
                and info.st_size == len(content) and stat.S_IMODE(info.st_mode) == mode,
                "The staged source has changed ownership, size or access.")
            observed = handle.read(len(content) + 1)
            _require(_fingerprint(info) == _fingerprint(os.fstat(handle.fileno()))
                == _fingerprint(path.lstat()), "The staged source changed during its original read.")
        pin = ArtifactFile(path, len(content), hashlib.sha256(content).hexdigest())
        installed.files[path] = pin, _fingerprint(info)
        _require(observed == content,
            "The staged application source differs from its accepted bytes or modes.")
    receiver._read(store / "source-owner.py", owner_source, keep=False)[1].finish()
    evidence.finish()
    inputs.finish()
    installed.finish()
    _source_inventory(root, files, installed_view=installed_view)
    selected_runtime_artifacts(relocated, verified_artifacts=artifacts)
    return files, original, relocated, artifacts, (evidence, inputs, installed)


def _source_inventory(root, files, *, installed_view=None):
    """Keep authored source closed, leaving real private lifecycle to its owner."""
    if installed_view is not None:
        # Only the trusted installed reader supplies this finite view. Frozen
        # source staging retains its original strict inventory and imports no
        # installed helper or received application code.
        from .product_installed_source import require_installed_inventory
        return require_installed_inventory(root, files, installed_view)
    actual = set()
    private_roots = {".mobkit", ".venv", ".bin", ".env", "data"}
    for directory, children, entries in os.walk(root, followlinks=False):
        path = Path(directory)
        children[:] = [name for name in children if path != root or name not in private_roots]
        for name in children:
            info = (path / name).lstat()
            _require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o022,
                "The retained source contains a linked, unowned or writable directory.")
        for name in entries:
            if path == root and name in private_roots:
                continue
            selected = path / name
            info = selected.lstat()
            _require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1,
                "The retained source contains an unowned, linked or special file.")
            actual.add(str(selected.relative_to(root)))
            _require(len(actual) <= 2048, "The retained source exceeds its original inventory bound.")
    _require(actual == set(files), "The retained source contains missing or unselected application files.")


def _materialize(root, files, context, relocated):
    home = Path(context["host"]["home"])
    # The observed, trusted destination is never a locator authored by the ZIP.
    parent = root
    while not parent.exists():
        parent = parent.parent
    _require(parent.stat().st_uid == os.getuid() and parent.is_dir(),
        "The observed destination ancestor is not owned.")
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    receiver._path(str(root))
    _require(not any(root.iterdir()), "The observed destination must remain empty for this original intake.")
    with project_lock(root) as lease:
        directory = root / SOURCE_DIRECTORY
        receiver._directory(directory, home) if directory.is_relative_to(home) else _private_project_directory(directory, root)
        receiver._publish(directory / "owner.json", _json(_source_owner(context)))
        receiver._publish(directory / "relocated-selection.json", product_selection_bytes(relocated))
        for name, (content, mode) in files.items():
            lease.require_current()
            path = root / name
            _private_project_directory(path.parent, root)
            receiver._publish(path, content)
            path.chmod(mode)
        lease.require_current()


def _private_project_directory(path, root):
    _require(path.is_relative_to(root), "The accepted source directory escapes the observed project.")
    current = root
    for name in path.relative_to(root).parts:
        current /= name
        current.mkdir(mode=0o700, exist_ok=True)
        receiver._path(str(current))
        info = current.stat()
        _require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077,
            "The accepted source directory is not privately owned.")


def receive(request, stream=None, *, owner_source):
    _require(type(request) is dict and request.get("action") in {"begin", "package", "finish", "observe"},
        "The private source intake action is invalid.")
    action = request["action"]
    _require(set(request) == ({"action", "context", "transfer_file"} if action == "package"
        else {"action", "context"}), "The private source intake request has unexpected fields.")
    context = _context(request["context"], owner_source)
    if action == "begin":
        _artifact_custody(context)
    with _intake(context, owner_source, create=action == "begin") as (store, current):
        base = {key: value for key, value in _result(context).items() if key != "status"}
        if action == "begin":
            _require(not (store / "entry.json").exists() or (store / "staged.json").exists(),
                "This original source intake entered staging without a confirmed result. Preserve its partial state.")
            if (store / "staged.json").exists():
                root = Path(context["host"]["destination"])
                with project_lock(root) as lease:
                    _staged_inputs(context, store, owner_source=owner_source)
                    lease.require_current()
                    receiver._read(store / "staged.json", _json(_result(context)), keep=False)[1].finish()
                current()
                return _result(context)
            return {**base, "status": "product-source-intake-retained"}
        if action == "package":
            _require(not (store / "entry.json").exists(),
                "This original source staging already entered. Preserve its retained package and original result.")
            expected = {"size": context["package_size"], "sha256": context["package_sha256"]}
            _require(request["transfer_file"] == expected, "The original accepted package stream changed.")
            receiver._stream_file(store, store / "package.zip", stream, expected, current)
            return {**base, "status": "product-source-package-received"}
        root = Path(context["host"]["destination"])
        if (store / "staged.json").exists():
            with project_lock(root) as lease:
                _staged_inputs(context, store, owner_source=owner_source)
                lease.require_current()
                receiver._read(store / "staged.json", _json(_result(context)), keep=False)[1].finish()
            current()
            return _result(context)
        _require(action == "finish" and not (store / "entry.json").exists(),
            "The original source staging has no confirmed result. Preserve its original partial attempt.")
        original, relocated, artifacts, evidence = _artifact_custody(context)
        inputs = _Inputs(store)
        _, payload = inputs.read(store / "package.zip", context["package_sha256"], MAX_APPLICATION_BYTES,
            size=context["package_size"])
        files, _ = _package_source(payload, context, original)
        # The existing location lock coordinates admission before the project
        # exists. The actual source writer then holds the canonical project lease.
        from .remote_destination import _lock
        with _lock(root, context["host"]):
            receiver._path(str(root))
            _require(not root.exists() or root.is_dir() and not any(root.iterdir()),
                "The original observed destination is no longer empty.")
            inputs.finish()
            evidence.finish()
            current()
            selected_runtime_artifacts(relocated, verified_artifacts=artifacts)
            receiver._publish(store / "entry.json", _json({"schema": 1, "context": context,
                "scope": "original-stopped-source-staging-attempt"}))
            _materialize(root, files, context, relocated)
            with project_lock(root) as lease:
                _staged_inputs(context, store, owner_source=owner_source)
                current()
                lease.require_current()
                receiver._publish(store / "staged.json", _json(_result(context)))
            current()
            return _result(context)


class StoppedProductSource:
    """Full staged source fence supplied to a later original setup owner.

    The caller supplies its real stopped-owner check and already held project
    lease. This carrier neither acquires a lease nor interprets native history.
    Its receipt-shaped projection explicitly describes source staging only.
    """

    def __init__(self, context, store, owner_source, lease, require_current, *,
            installed_view=None, file_lane=False, retained=None):
        self._context, self._store = deepcopy(context), store
        self._owner_source, self._lease, self._owner_current = owner_source, lease, require_current
        self._closed = self._invalidated = False
        self._installed_view = installed_view
        self._root = Path(context["host"]["destination"])
        if file_lane:
            self._lease.require_current()
        else:
            self.require_owner_current()
        files, self._origin, self._relocated, self._artifacts, self._observed = _staged_inputs(
            context, store, owner_source=owner_source, installed_view=installed_view, retained=retained)
        self._files = {name: ArtifactFile(self._root / name, len(content), hashlib.sha256(content).hexdigest())
            for name, (content, _) in files.items()}
        self._receipt = {"scope": "staged-product-application-source", "status": "product-source-staged",
            "destination": str(self._root), "guide_id": context["guide_id"],
            "deployment_id": context["deployment_id"], "build_id": context["build_id"],
            "package_sha256": context["package_sha256"], "source_digest": context["source_digest"]}
        if file_lane:
            self.require_filesystem_current()
        else:
            self.require_current()

    @property
    def origin(self):
        return self._origin

    @property
    def relocated(self):
        return self._relocated

    @property
    def artifacts(self):
        return self._artifacts

    @property
    def receipt(self):
        return deepcopy(self._receipt)

    @property
    def files(self):
        return tuple(self._files)

    def require_owner_current(self):
        try:
            _require(not self._closed and not self._invalidated and self._lease.root == self._root,
                "The original stopped source lease is no longer current.")
            self._lease.require_current()
            _current(self._owner_current)
        except BaseException:
            self._invalidated = True
            raise

    def require_filesystem_current(self):
        try:
            _require(not self._closed and not self._invalidated and self._lease.root == self._root,
                "The original source custody is closed or changed.")
            self._lease.require_current()
            for observed in self._observed:
                observed.finish()
            selected_runtime_artifacts(self.relocated, verified_artifacts=self.artifacts)
            _source_inventory(self._root, self._files, installed_view=self._installed_view)
            self._lease.require_current()
        except BaseException:
            self._invalidated = True
            raise

    def require_current(self):
        self.require_owner_current()
        self.require_filesystem_current()
        self.require_owner_current()

    async def require_current_async(self):
        self.require_owner_current()
        await _joined(self.require_filesystem_current, _owner_current=self.require_owner_current)
        self.require_owner_current()

    def source_file(self, name):
        """Select original application bytes, never a private native input pin."""
        self.require_current()
        _require(name in self._files, "Select a file from the original staged application source.")
        return self._files[name]

    def read_source(self, name):
        """Read canonical source under its full owner, byte and mode fences.

        A genuine later setup supplier retains its own private input projection.
        This source read neither creates that projection nor supplies ExactFile.
        Empty resources retain their original accepted bytes and access modes.
        """
        self.source_file(name)
        pin = self._files[name]
        expected = self._observed[-1].files[pin.path][1]
        descriptor = os.open(pin.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as handle:
            info = os.fstat(handle.fileno())
            _require(_fingerprint(info) == expected and stat.S_ISREG(info.st_mode)
                and info.st_uid == os.getuid() and info.st_nlink == 1
                and info.st_size == pin.size <= MAX_APPLICATION_BYTES,
                "The original staged source file changed before its read.")
            content = handle.read(pin.size + 1)
            _require(_fingerprint(info) == _fingerprint(os.fstat(handle.fileno()))
                == _fingerprint(pin.path.lstat()) and len(content) == pin.size
                and hashlib.sha256(content).hexdigest() == pin.sha256,
                "The original staged source file changed during its read.")
        self.require_current()
        return content

    def close(self):
        self._closed = True


@contextmanager
def open_stopped_product_source(context, *, project_lease, require_current, owner_source):
    """Continue the exact staged owner under its caller's original live lease."""
    context = _context(context, owner_source)
    _current(require_current)
    _require(getattr(project_lease, "root", None) == Path(context["host"]["destination"])
        and callable(getattr(project_lease, "require_current", None)),
        "Continue only this original project's retained lease.")
    with _intake(context, owner_source) as (store, current):
        receiver._read(store / "staged.json", _json(_result(context)), keep=False)[1].finish()
        source = StoppedProductSource(context, store, owner_source, project_lease, require_current)
        try:
            current()
            yield source
            if not source._invalidated:
                source.require_current()
            current()
        finally:
            source.close()


read_request = receiver.read_request


def main():
    try:
        request, stream = read_request(sys.stdin.buffer)
        result = receive(request, stream, owner_source=CONTROLLER_SOURCE)
    except Exception:
        result = {"error": "product-source-intake-unavailable"}
    content = _json(result)
    if len(content) > receiver.MAX_RESPONSE_BYTES:
        content = _json({"error": "product-source-intake-unavailable"})
    sys.stdout.buffer.write(receiver.FRAME_PREFIX + content)
    sys.stdout.buffer.flush()


async def stage_product_destination_source(implementation, guide_id, *, build_id, source_digest,
        guide_version, deployment_id, custody, transaction_directory, destination_host, transport,
        endpoint, expected_fingerprint, require_current, timeout=600, source_disposition=None):
    """Trusted callable intake from the accepted implementation owner only."""
    from .remote_interface_bundle import product_destination_owner_bytes
    from .remote_product_artifacts import _private_directory, _retain_file, _receiver_program
    from .ssh_transport import _operation_deadline, _remaining, _fingerprint_digest, SshTransportError
    _current(require_current)
    _require(type(custody) is ProductArtifactCustody and callable(getattr(implementation, "package", None))
        and callable(getattr(implementation, "selected_runtime_target", None))
        and callable(getattr(implementation, "status", None))
        and callable(getattr(transport, "request", None)) and callable(getattr(transport, "transfer_file", None))
        and _fingerprint_digest(expected_fingerprint) is not None,
        "Source staging needs its original accepted implementation, complete custody and pinned transport.")
    # Retain the original clock for worker-side deadline facts. The production
    # _remaining helper and original owner callback stay on the calling loop.
    operation_clock = asyncio.get_running_loop().time
    deadline = _operation_deadline(timeout, None)
    checked = await implementation.status(guide_id)
    _require(checked.get("build_id") == build_id and checked.get("guide_version") == guide_version
        and checked.get("source_digest") == source_digest and checked.get("current") is True
        and checked.get("acceptance_current") is True and checked.get("acceptance_trusted") is True
        and checked.get("acceptance", {}).get("status") == "pass" and checked.get("ready_to_package") is True,
        "Source staging requires the current independently accepted application.")
    _current(require_current)
    origin = await _joined(implementation.selected_runtime_target, guide_id,
        build_id=build_id, source_digest=source_digest, _owner_current=require_current)
    _require(origin == custody.origin, "The accepted implementation and original artifact selection differ.")
    payload, _ = await implementation.package(guide_id, build_id)
    _current(require_current)
    source = product_destination_owner_bytes()
    # Hashing, retaining and rereading the whole package must not hold the
    # event loop.
    package_sha256 = await _joined(lambda: hashlib.sha256(payload).hexdigest())
    context = {"schema": 1, "deployment_id": deployment_id, "guide_id": guide_id,
        "build_id": build_id, "guide_version": guide_version, "source_digest": source_digest,
        "package_sha256": package_sha256, "package_size": len(payload),
        "host": deepcopy(destination_host), "artifact_transaction_id": custody.transaction_id,
        "artifact_manifest_sha256": custody.manifest_sha256, "artifact_receiver_sha256": custody.receiver_sha256,
        "artifact_selection_sha256": custody.selection_sha256, "owner_sha256": hashlib.sha256(source).hexdigest()}
    if source_disposition is not None:
        context.update(schema=2, source_disposition=_gcp_disposition(source_disposition))
    context = _context(context, source, observe_host=False)
    # The actual destination account/platform check belongs to its frozen owner.
    _require(type(payload) is bytes and 0 < len(payload) <= MAX_APPLICATION_BYTES,
        "The accepted application package exceeds its original source bound.")
    # Source admission joins its original selection on the implementation host;
    # the frozen destination separately enforces its actual native platform.
    await _joined(_package_source, payload, context, origin, check_platform=False,
        _owner_current=require_current)
    directory = Path(transaction_directory)
    _private_directory(directory)
    def retained_inputs():
        _retain_file(directory / "package.zip", payload)
        _retain_file(directory / "source-owner.py", source)
        _retain_file(directory / "intent.json", _json(context))
        retained = _Inputs(directory)
        for name, content in (("package.zip", payload), ("source-owner.py", source), ("intent.json", _json(context))):
            retained.read(directory / name, hashlib.sha256(content).hexdigest(), len(content), size=len(content), keep=False)
        retained.finish()
        return retained
    retained = await _joined(retained_inputs)
    program = _receiver_program(source)
    package = ArtifactFile(directory / "package.zip", len(payload), context["package_sha256"])
    def files_current():
        if deadline - operation_clock() <= 0:
            raise SshTransportError("timeout")
        retained.finish()
        _require(implementation.selected_runtime_target(guide_id,
            build_id=build_id, source_digest=source_digest) == origin,
            "The original accepted runtime source changed during intake.")
        if deadline - operation_clock() <= 0:
            raise SshTransportError("timeout")
    def stream_current():
        _current(require_current)
        _remaining(deadline)
        retained.finish()
    def returned(value, expected_status):
        _require(type(value) is dict and value.get("status") == expected_status
            and all(value.get(key) == item for key, item in _result(context).items() if key != "status")
            and type(value.get("_transport")) is dict
            and set(value["_transport"]) == {"host_key_fingerprint"}
            and _fingerprint_digest(value["_transport"]["host_key_fingerprint"])
                == _fingerprint_digest(expected_fingerprint),
            "The original product source intake did not provide its exact pinned observation.")
    await _joined(files_current, _owner_current=require_current)
    begun = await transport.request(endpoint, program, {"action": "begin", "context": context},
        _remaining(deadline), expected_fingerprint=expected_fingerprint, deadline=deadline)
    await _joined(files_current, _owner_current=require_current)
    if begun.get("status") == "product-source-staged":
        returned(begun, "product-source-staged")
        _current(require_current)
        _retain_file(directory / "staged.json", _json({"context": context, "observation": _result(context)}))
        return _result(context)
    returned(begun, "product-source-intake-retained")
    await _joined(files_current, _owner_current=require_current)
    returned(await transport.transfer_file(endpoint, program, {"action": "package", "context": context},
        package, _remaining(deadline), expected_fingerprint=expected_fingerprint,
        deadline=deadline, require_current=stream_current), "product-source-package-received")
    await _joined(files_current, _owner_current=require_current)
    value = await transport.request(endpoint, program, {"action": "finish", "context": context},
        _remaining(deadline), expected_fingerprint=expected_fingerprint, deadline=deadline)
    await _joined(files_current, _owner_current=require_current)
    returned(value, "product-source-staged")
    _current(require_current)
    _retain_file(directory / "staged.json", _json({"context": context, "observation": _result(context)}))
    return _result(context)


if __name__ == "__main__":
    main()
