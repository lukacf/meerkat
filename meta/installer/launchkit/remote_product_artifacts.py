"""Inert complete product artifact transfer for a future destination owner.

The implementation owner supplies the original accepted runtime. Relocation
changes local locators only. It does not rewrite application source, install a
runtime, qualify a release or select any native setup/bootstrap input.
"""
from __future__ import annotations

import asyncio
from dataclasses import asdict, dataclass
import hashlib
import inspect
import json
import math
import os
from pathlib import Path
import re
import stat
import uuid

from .runtime_candidates import (ArtifactFile, MetaToolkitRuntimeTarget,
    MetaToolkitRuntimeArtifacts, _Inputs, MAX_PRODUCT_METADATA)
from .runtime_selection import (PRODUCT_SELECTION_FORMAT, _json_paths,
    candidate_selection, parse_candidate_selection, selected_runtime_artifacts)


class ProductArtifactTransferError(ValueError):
    pass


def _require(value, message):
    if not value:
        raise ProductArtifactTransferError(message)


def _json(value):
    return json.dumps(value, ensure_ascii=True, allow_nan=False,
        sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"


def _unique(pairs):
    result = {}
    for name, value in pairs:
        _require(name not in result, "The artifact input has duplicate fields.")
        result[name] = value
    return result


def _decode(content):
    _require(type(content) is bytes and 0 < len(content) <= MAX_PRODUCT_METADATA,
        "The product artifact manifest exceeds its bound.")
    def invalid(_):
        raise ProductArtifactTransferError("The product artifact input has an invalid number.")
    try:
        return json.loads(content, object_pairs_hook=_unique, parse_constant=invalid)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise ProductArtifactTransferError("The product artifact input is malformed.") from error


def _current(callback):
    _require(callable(callback), "Artifact transfer needs its original synchronous source owner.")
    result = callback()
    if inspect.isawaitable(result):
        if inspect.iscoroutine(result):
            result.close()
        raise ProductArtifactTransferError("Artifact transfer needs a synchronous source owner.")
    _require(result is not False, "The original artifact transfer source owner changed.")


async def _joined(function, *args, _owner_current=None, **kwargs):
    pending = asyncio.create_task(asyncio.to_thread(function, *args, **kwargs))
    try:
        while not pending.done():
            if _owner_current is not None:
                _current(_owner_current)
            await asyncio.wait((pending,), timeout=0.05)
        if _owner_current is not None:
            _current(_owner_current)
        return pending.result()
    except BaseException:
        while not pending.done():
            try:
                await asyncio.shield(pending)
            except asyncio.CancelledError:
                continue
            except Exception:
                break
        if not pending.cancelled():
            pending.exception()
        raise


@dataclass(frozen=True)
class ProductArtifactTransferPlan:
    origin: MetaToolkitRuntimeTarget
    artifacts: MetaToolkitRuntimeArtifacts
    manifest: bytes
    manifest_sha256: str
    files: tuple[ArtifactFile, ...]


@dataclass(frozen=True)
class ProductArtifactCustody:
    """Destination byte custody only, never application or native admission."""
    transaction_id: str
    manifest_sha256: str
    receiver_sha256: str
    origin: MetaToolkitRuntimeTarget
    relocated: MetaToolkitRuntimeTarget
    selection_sha256: str


def prepare_product_artifact_transfer(origin, *, require_current):
    _current(require_current)
    _require(type(origin) is MetaToolkitRuntimeTarget,
        "Artifact transfer requires the implementation owner's exact product selection.")
    artifacts = selected_runtime_artifacts(origin)
    selection = candidate_selection(origin, verified_artifacts=artifacts)
    evidence = {item.path: item for item in artifacts.evidence}
    _require(len(evidence) == len(artifacts.evidence), "The selected artifact closure has duplicate paths.")
    if origin.source_manifest is not None:
        source = origin.source_manifest
    else:
        build_pin = evidence.get(origin.assembly.path.parent / "build-record.json")
        _require(build_pin is not None, "The selected producer build input is missing.")
        inputs = _Inputs(origin.artifact_root)
        _, content = inputs.read(build_pin.path, build_pin.sha256, MAX_PRODUCT_METADATA, size=build_pin.size)
        inputs.finish()
        producer = _decode(content)["source_manifest"]
        source = evidence.get(Path(producer["path"]))
        _require(source is not None and source.sha256 == producer["sha256"],
            "The selected exact source manifest is missing from the complete closure.")
    _require(evidence.get(source.path) == source,
        "The selected source manifest is outside the complete artifact closure.")
    files = tuple(sorted(evidence.values(), key=lambda item: str(item.path)))
    rows = [{"name": str(item.path.relative_to(origin.artifact_root)),
        "size": item.size, "sha256": item.sha256} for item in files]
    manifest = _json({"schema": 1, "origin": selection, "files": rows,
        "source_manifest": str(source.path.relative_to(origin.artifact_root))})
    _require(len(manifest) <= MAX_PRODUCT_METADATA, "The product artifact manifest exceeds its bound.")
    # Check the original selection's explicit pins and safe relative locators.
    # The destination's fresh reader checks the relocated producer joins.
    parse_product_artifact_manifest(manifest)
    selected_runtime_artifacts(origin, verified_artifacts=artifacts)
    _current(require_current)
    return ProductArtifactTransferPlan(origin, artifacts, manifest,
        hashlib.sha256(manifest).hexdigest(), files)


def parse_product_artifact_manifest(content):
    value = _decode(content)
    _require(type(value) is dict and set(value) == {"schema", "origin", "files", "source_manifest"}
        and type(value["schema"]) is int and value["schema"] == 1,
        "The product artifact manifest has an invalid shape.")
    origin = parse_candidate_selection(value["origin"])
    _require(type(origin) is MetaToolkitRuntimeTarget, "The artifact manifest does not select a complete product.")
    _require(type(value["files"]) is list and 0 < len(value["files"]) <= 270,
        "The complete artifact input list is invalid.")
    rows, names = [], set()
    for row in value["files"]:
        _require(type(row) is dict and set(row) == {"name", "size", "sha256"},
            "A product artifact input has an invalid shape.")
        name = row["name"]
        _require(isinstance(name, str) and 0 < len(name) <= 4096 and "\0" not in name
            and not Path(name).is_absolute() and str(Path(name)) == name
            and ".." not in Path(name).parts and name not in {".", ""}
            and name not in names and name.casefold() not in {item.casefold() for item in names},
            "A product artifact input name is unsafe or duplicated.")
        _require(type(row["size"]) is int and 0 < row["size"] <= 3 * 1024 * 1024 * 1024
            and isinstance(row["sha256"], str) and re.fullmatch(r"[a-f0-9]{64}", row["sha256"]),
            "A product artifact byte binding is invalid.")
        names.add(name)
        rows.append(dict(row))
    _require(value["source_manifest"] in names, "The explicit local source manifest input is missing.")
    pins = [origin.assembly, origin.meta_wheel, origin.toolkit_wheel,
        origin.dependency_requirements, origin.model_catalog, *origin.dependency_wheels]
    if origin.source_manifest is not None:
        pins.append(origin.source_manifest)
    inputs = {row["name"]: row for row in rows}
    for pin in pins:
        _require(pin.path.is_relative_to(origin.artifact_root), "An origin input escapes its selected artifact root.")
        name = str(pin.path.relative_to(origin.artifact_root))
        _require(inputs.get(name) == {"name": name, "size": pin.size, "sha256": pin.sha256},
            "The transferred artifact closure differs from its original explicit selection.")
    return origin, tuple(rows), value["source_manifest"]


def relocate_product_target(manifest, destination_root):
    """Compute explicit local pins, preserving all producer bytes and facts."""
    origin, rows, source_name = parse_product_artifact_manifest(manifest)
    root = Path(destination_root)
    _require(root.is_absolute() and str(root) == str(destination_root) and ".." not in root.parts,
        "The destination artifact root must be canonical and absolute.")
    values = _json_paths(asdict(origin))
    values["artifact_root"] = str(root)
    def local(pin):
        return {"path": str(root / pin.path.relative_to(origin.artifact_root)),
            "size": pin.size, "sha256": pin.sha256}
    for name in ("assembly", "meta_wheel", "toolkit_wheel", "dependency_requirements", "model_catalog"):
        values[name] = local(getattr(origin, name))
    values["dependency_wheels"] = [local(item) for item in origin.dependency_wheels]
    source = next(row for row in rows if row["name"] == source_name)
    values["source_manifest"] = {"path": str(root / source_name),
        "size": source["size"], "sha256": source["sha256"]}
    return parse_candidate_selection({"format": PRODUCT_SELECTION_FORMAT, **values})


def product_selection_bytes(target):
    return _json({"format": PRODUCT_SELECTION_FORMAT, **_json_paths(asdict(target))})


def _private_directory(path):
    _require(path.is_absolute() and ".." not in path.parts
        and not any(item.is_symlink() for item in (path, *path.parents)),
        "The local artifact transfer store is unsafe.")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    _require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077,
        "The local artifact transfer store is not privately owned.")


def _retain_file(path, content):
    if path.exists() or path.is_symlink():
        inputs = _Inputs(path.parent)
        inputs.read(path, hashlib.sha256(content).hexdigest(), max(len(content), 1), size=len(content))
        inputs.finish()
        _require(not path.stat().st_mode & 0o077, "The retained artifact input is not private.")
        return
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())


def _receiver_program(content):
    import base64
    import zlib
    encoded = base64.b64encode(zlib.compress(content, level=9)).decode("ascii")
    return ("import base64,hashlib,zlib; s=zlib.decompress(base64.b64decode(" + repr(encoded)
        + ")); assert hashlib.sha256(s).hexdigest()==" + repr(hashlib.sha256(content).hexdigest())
        + "; exec(compile(s,'<forge-product-artifact-receiver>','exec'),"
        + "{'__name__':'__main__','CONTROLLER_SOURCE':s})")


async def transfer_product_artifacts(implementation, guide_id, *, build_id, source_digest,
        transaction_id, transaction_directory, destination_host, transport, endpoint,
        expected_fingerprint, require_current, timeout=600):
    """Concrete host-owned transfer path, outside ordinary remote Install.

    This constructor-level call is not an HTTP runtime selector. The accepted
    implementation owner is the only supplier of the original runtime target.
    The durable result is inert input custody for a later destination owner.
    """
    from .remote_interface_bundle import product_artifact_receiver_bytes
    from .ssh_transport import _operation_deadline, _remaining
    from . import ssh_transport
    _current(require_current)
    _require(isinstance(guide_id, str) and str(uuid.UUID(guide_id)) == guide_id
        and isinstance(build_id, str) and str(uuid.UUID(build_id)) == build_id
        and isinstance(transaction_id, str) and str(uuid.UUID(transaction_id)) == transaction_id
        and isinstance(source_digest, str) and re.fullmatch(r"[a-f0-9]{64}", source_digest),
        "Artifact transfer requires exact original application and transaction identities.")
    _require(type(destination_host) is dict and set(destination_host) == {"uid", "home", "destination"},
        "Artifact transfer requires the exact observed destination host.")
    _require(type(destination_host["uid"]) is int and destination_host["uid"] >= 0
        and all(isinstance(destination_host[name], str) and 0 < len(destination_host[name]) <= 4096
            and "\0" not in destination_host[name] and Path(destination_host[name]).is_absolute()
            and str(Path(destination_host[name])) == destination_host[name]
            and ".." not in Path(destination_host[name]).parts for name in ("home", "destination")),
        "Artifact transfer requires canonical observed destination locators.")
    _require(not isinstance(timeout, bool) and isinstance(timeout, (int, float))
        and timeout > 0 and math.isfinite(timeout), "The artifact transfer deadline is invalid.")
    destination_host = dict(destination_host)
    endpoint = dict(endpoint)
    _require(callable(getattr(implementation, "selected_runtime_target", None))
        and callable(getattr(transport, "request", None))
        and callable(getattr(transport, "transfer_file", None)),
        "The original implementation selection or selected file transport is unavailable.")
    _require(ssh_transport._fingerprint_digest(expected_fingerprint) is not None,
        "Artifact transfer requires the previously observed SSH server identity.")
    deadline = _operation_deadline(timeout, None)
    origin = await _joined(implementation.selected_runtime_target,
        guide_id, build_id=build_id, source_digest=source_digest, _owner_current=require_current)
    plan = await _joined(prepare_product_artifact_transfer, origin, require_current=lambda: True,
        _owner_current=require_current)
    _remaining(deadline)
    receiver = product_artifact_receiver_bytes()
    receiver_hash = hashlib.sha256(receiver).hexdigest()
    context = {"schema": 1, "transaction_id": transaction_id, "guide_id": guide_id,
        "build_id": build_id, "source_digest": source_digest, "host": dict(destination_host),
        "manifest_sha256": plan.manifest_sha256, "manifest_size": len(plan.manifest),
        "receiver_sha256": receiver_hash}
    directory = Path(transaction_directory)
    _private_directory(directory)
    _retain_file(directory / "manifest.json", plan.manifest)
    _retain_file(directory / "receiver.py", receiver)
    _retain_file(directory / "intent.json", _json(context))
    manifest_pin = ArtifactFile(directory / "manifest.json", len(plan.manifest), plan.manifest_sha256)
    program = _receiver_program(receiver)

    retained = _Inputs(directory)
    for name, data in (("manifest.json", plan.manifest), ("receiver.py", receiver), ("intent.json", _json(context))):
        retained.read(directory / name, hashlib.sha256(data).hexdigest(), max(len(data), 1),
            size=len(data), keep=False)
    retained.finish()
    clock = asyncio.get_running_loop().time

    def files_current():
        _require(clock() < deadline, "The original artifact transfer deadline elapsed.")
        retained.finish()

    def stream_current():
        _current(require_current)
        files_current()

    def current():
        files_current()
        observed = implementation.selected_runtime_target(guide_id, build_id=build_id, source_digest=source_digest)
        _require(observed == origin, "The original implementation-owned runtime selection changed.")
        selected_runtime_artifacts(origin, verified_artifacts=plan.artifacts)

    def checked(value):
        _require(type(value) is dict and value.get("transaction_id") == transaction_id
            and value.get("manifest_sha256") == plan.manifest_sha256
            and value.get("receiver_sha256") == receiver_hash
            and value.get("host") == destination_host and "error" not in value,
            "The artifact receiver result could not be joined to this original transfer.")
        metadata = value.get("_transport")
        _require(type(metadata) is dict and set(metadata) == {"host_key_fingerprint"},
            "The artifact transfer did not retain its exact SSH observation.")
        observed = metadata["host_key_fingerprint"]
        _require(ssh_transport._fingerprint_digest(observed)
            == ssh_transport._fingerprint_digest(expected_fingerprint),
            "The artifact transfer SSH identity changed.")
        return value

    # A failed or cancelled request can leave destination bytes behind. The
    # immutable intent remains; only an exact final returned observation yields
    # custody. No local timeout claims remote cleanup or completion.
    await _joined(current, _owner_current=require_current)
    checked(await transport.request(endpoint, program, {"action": "begin", "context": context},
        _remaining(deadline), expected_fingerprint=expected_fingerprint, deadline=deadline))
    checked(await transport.transfer_file(endpoint, program, {"action": "manifest", "context": context},
        manifest_pin, _remaining(deadline), expected_fingerprint=expected_fingerprint,
        deadline=deadline, require_current=stream_current))
    for artifact in plan.files:
        await _joined(current, _owner_current=require_current)
        name = str(artifact.path.relative_to(origin.artifact_root))
        checked(await transport.transfer_file(endpoint, program,
            {"action": "file", "context": context, "name": name}, artifact,
            _remaining(deadline), expected_fingerprint=expected_fingerprint,
            deadline=deadline, require_current=stream_current))
    await _joined(current, _owner_current=require_current)
    value = checked(await transport.request(endpoint, program, {"action": "finish", "context": context},
        _remaining(deadline), expected_fingerprint=expected_fingerprint, deadline=deadline))
    await _joined(current, _owner_current=require_current)
    root = Path(destination_host["home"]) / ".local/state/meta-forge/product-artifacts" / transaction_id / "inputs"
    relocated = relocate_product_target(plan.manifest, root)
    selection_hash = hashlib.sha256(product_selection_bytes(relocated)).hexdigest()
    _require(value.get("status") == "runtime-artifacts-transferred" and value.get("artifact_root") == str(root)
        and value.get("selection_sha256") == selection_hash,
        "The complete relocated artifact observation differs from the original transfer.")
    _retain_file(directory / "received.json", _json({"context": context, "artifact_root": str(root),
        "selection_sha256": selection_hash, "status": "runtime-artifacts-transferred"}))
    return ProductArtifactCustody(transaction_id, plan.manifest_sha256, receiver_hash,
        origin, relocated, selection_hash)
