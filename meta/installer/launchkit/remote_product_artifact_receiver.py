"""Private destination custody of inert, complete product artifact inputs.

Only the frozen trusted helper bundle is imported. Received SDKs, resources and
native programs are byte inputs and are never imported, extracted or executed.
This store does not admit an application, install a service or create setup
authority. A future destination owner must join the original application source.
"""
from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import os
from pathlib import Path
import pwd
import re
import stat
import struct
import sys
import uuid

from .remote_product_artifacts import (ProductArtifactTransferError, _decode,
    _json, _require, parse_product_artifact_manifest, product_selection_bytes,
    relocate_product_target)
from .runtime_candidates import _Inputs, _fingerprint, MAX_PRODUCT_METADATA
from .runtime_selection import selected_runtime_artifacts


FRAME_PREFIX = b"MOBKIT_REMOTE_V1 "
MAX_RESPONSE_BYTES = 64 * 1024
MAX_REQUEST_BYTES = MAX_PRODUCT_METADATA
CHUNK_BYTES = 1024 * 1024


def _path(value):
    _require(isinstance(value, str) and 0 < len(value) <= 4096 and "\0" not in value,
        "The observed destination locator is invalid.")
    path = Path(value)
    _require(path.is_absolute() and str(path) == value and ".." not in path.parts
        and not any(item.is_symlink() for item in (path, *path.parents)),
        "The observed destination locator is unsafe.")
    return path


def _observed_host(expected):
    """Join the previously observed SSH account and destination, without setup."""
    _require(type(expected) is dict and set(expected) == {"uid", "home", "destination"}
        and type(expected["uid"]) is int and expected["uid"] >= 0,
        "The original observed destination host is invalid.")
    home, destination = _path(expected["home"]), _path(expected["destination"])
    actual_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    _require(expected["uid"] == os.getuid() and home == actual_home
        and destination not in (Path("/"), home),
        "The artifact receiver belongs to another observed destination host.")
    info = home.lstat()
    _require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid(),
        "The artifact receiver account home is not an owned ordinary directory.")
    return dict(expected)


def _context(value, receiver_source):
    fields = {"schema", "transaction_id", "guide_id", "build_id", "source_digest",
        "host", "manifest_sha256", "manifest_size", "receiver_sha256"}
    _require(type(value) is dict and set(value) == fields and type(value["schema"]) is int
        and value["schema"] == 1, "The artifact receiver ownership record is invalid.")
    for name in ("transaction_id", "guide_id", "build_id"):
        item = value[name]
        _require(isinstance(item, str) and str(uuid.UUID(item)) == item,
            "The original artifact transaction identity is invalid.")
    for name in ("source_digest", "manifest_sha256", "receiver_sha256"):
        _require(isinstance(value[name], str) and re.fullmatch(r"[a-f0-9]{64}", value[name]),
            "The original artifact transaction byte binding is invalid.")
    _require(type(value["manifest_size"]) is int and 0 < value["manifest_size"] <= MAX_PRODUCT_METADATA
        and type(receiver_source) is bytes and receiver_source
        and hashlib.sha256(receiver_source).hexdigest() == value["receiver_sha256"],
        "The original frozen artifact receiver or manifest binding changed.")
    _observed_host(value["host"])
    return _decode(_json(value))


def _directory(path, home):
    """Create only this account's private artifact store, never the project."""
    _require(path.is_relative_to(home) and path != home, "The artifact store escapes its account home.")
    _path(str(home))
    current = home
    for name in path.relative_to(home).parts:
        current /= name
        try:
            current.mkdir(mode=0o700)
        except FileExistsError:
            pass
        info = current.lstat()
        _require(stat.S_ISDIR(info.st_mode) and not current.is_symlink()
            and info.st_uid == os.getuid(), "The artifact store contains an unowned or linked directory.")
    info = path.lstat()
    _require(not info.st_mode & 0o077, "The artifact transaction directory is not private.")


def _read(path, content, *, keep=True):
    inputs = _Inputs(path.parent)
    _, observed = inputs.read(path, hashlib.sha256(content).hexdigest(), max(len(content), 1),
        size=len(content), keep=keep)
    inputs.finish()
    _require(not path.stat().st_mode & 0o077, "The artifact ownership input is not private.")
    return observed, inputs


def _publish(path, content):
    """Never replace an existing original transaction input or observation."""
    if path.exists() or path.is_symlink():
        _read(path, content, keep=False)
        return
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(content)
        output.flush()
        os.fsync(output.fileno())


@contextmanager
def _transaction(context, receiver_source, *, create):
    home = Path(context["host"]["home"])
    store = home / ".local/state/meta-forge/product-artifacts" / context["transaction_id"]
    if create:
        _directory(store, home)
    else:
        _path(str(store))
        _require(store.is_dir(), "The original artifact transfer has not begun.")
    _require(store.stat().st_uid == os.getuid() and not store.stat().st_mode & 0o077,
        "The original artifact transaction is not private.")
    lock_path = store / "receiver.lock"
    descriptor = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    with os.fdopen(descriptor, "r+b") as lock:
        info = os.fstat(lock.fileno())
        _require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1
            and not info.st_mode & 0o077 and _fingerprint(info) == _fingerprint(lock_path.lstat()),
            "The artifact transaction lock is not privately owned.")
        fcntl.flock(lock, fcntl.LOCK_EX)
        store_identity = (store.stat().st_dev, store.stat().st_ino)
        if create:
            _publish(store / "owner.json", _json(context))
            _publish(store / "receiver.py", receiver_source)
            _directory(store / "inputs", home)
            _directory(store / "partial", home)
        owner, owner_inputs = _read(store / "owner.json", _json(context))
        _, receiver_inputs = _read(store / "receiver.py", receiver_source, keep=False)
        def current():
            _observed_host(context["host"])
            _path(str(store))
            _require((store.stat().st_dev, store.stat().st_ino) == store_identity
                and store.stat().st_uid == os.getuid() and not store.stat().st_mode & 0o077
                and _fingerprint(os.fstat(lock.fileno())) == _fingerprint(lock_path.lstat()),
                "The original artifact receiver directory or lock changed.")
            owner_inputs.finish()
            receiver_inputs.finish()
        current()
        yield store, current
        current()


def _exact(stream, size):
    output = bytearray()
    while len(output) < size:
        chunk = stream.read(size - len(output))
        _require(type(chunk) is bytes and chunk, "The artifact stream ended before its exact byte input.")
        output.extend(chunk)
    return bytes(output)


def _stream_file(store, destination, stream, binding, current):
    _require(type(binding) is dict and set(binding) == {"size", "sha256"}
        and type(binding["size"]) is int and 0 < binding["size"] <= 3 * 1024 * 1024 * 1024
        and isinstance(binding["sha256"], str) and re.fullmatch(r"[a-f0-9]{64}", binding["sha256"]),
        "The artifact stream has an invalid exact byte binding.")
    partial = store / "partial" / (str(uuid.uuid4()) + ".partial")
    descriptor = os.open(partial, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    hashed, remaining = hashlib.sha256(), binding["size"]
    # A failed stream remains a private partial input, never a usable complete
    # record. Local cancellation does not assert remote cleanup or completion.
    with os.fdopen(descriptor, "wb") as output:
        while remaining:
            current()
            chunk = stream.read(min(CHUNK_BYTES, remaining))
            _require(type(chunk) is bytes and chunk, "The artifact stream is incomplete.")
            _require(len(chunk) <= remaining, "The artifact stream exceeds its exact byte count.")
            hashed.update(chunk)
            output.write(chunk)
            remaining -= len(chunk)
        _require(not stream.read(1) and hashed.hexdigest() == binding["sha256"],
            "The artifact stream differs from its exact byte binding.")
        output.flush()
        os.fsync(output.fileno())
    current()
    inputs = _Inputs(store / "partial")
    inputs.read(partial, binding["sha256"], binding["size"], size=binding["size"], keep=False)
    inputs.finish()
    if destination.exists() or destination.is_symlink():
        held = _Inputs(destination.parent)
        held.read(destination, binding["sha256"], binding["size"], size=binding["size"], keep=False)
        held.finish()
    else:
        # An exclusive link plus partial unlink publishes complete bytes without
        # replacing another file. The transaction lock owns destination creation.
        os.link(partial, destination, follow_symlinks=False)
    partial.unlink()
    current()
    held = _Inputs(destination.parent)
    held.read(destination, binding["sha256"], binding["size"], size=binding["size"], keep=False)
    held.finish()


def _manifest(store, context):
    inputs = _Inputs(store)
    _, content = inputs.read(store / "manifest.json", context["manifest_sha256"], MAX_PRODUCT_METADATA,
        size=context["manifest_size"])
    inputs.finish()
    origin, rows, source_name = parse_product_artifact_manifest(content)
    return content, rows, inputs


def _inventory(root, expected):
    actual = set()
    required_directories = {root}
    for name in expected:
        for parent in (root / name).parents:
            if parent.is_relative_to(root):
                required_directories.add(parent)
    for directory, children, files in os.walk(root, followlinks=False):
        path = Path(directory)
        info = path.lstat()
        _require(path in required_directories and stat.S_ISDIR(info.st_mode)
            and info.st_uid == os.getuid() and not info.st_mode & 0o077,
            "The received artifact closure has an unexpected directory.")
        for name in children:
            child = path / name
            _require(not child.is_symlink() and child in required_directories,
                "The received artifact closure has a linked or unexpected directory.")
        for name in files:
            child = path / name
            info = child.lstat()
            _require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
                and info.st_nlink == 1 and not info.st_mode & 0o077,
                "The received artifact closure has a non-private or linked file.")
            actual.add(str(child.relative_to(root)))
    _require(actual == set(expected), "The received artifact closure has missing or extra inputs.")


def receive(request, stream=None, *, receiver_source):
    _require(type(request) is dict and request.get("action") in {"begin", "manifest", "file", "finish"},
        "The artifact receiver action is invalid.")
    action = request["action"]
    expected = {"action", "context"}
    if action in {"manifest", "file"}:
        expected.add("transfer_file")
    if action == "file":
        expected.add("name")
    _require(set(request) == expected and (stream is not None) == (action in {"manifest", "file"}),
        "The artifact receiver request has an invalid shape.")
    context = _context(request["context"], receiver_source)
    with _transaction(context, receiver_source, create=action == "begin") as (store, current):
        result = {"transaction_id": context["transaction_id"], "host": context["host"],
            "manifest_sha256": context["manifest_sha256"], "receiver_sha256": context["receiver_sha256"]}
        if action == "begin":
            return {**result, "status": "artifact-transfer-owned"}
        if action == "manifest":
            binding = {"size": context["manifest_size"], "sha256": context["manifest_sha256"]}
            _require(request["transfer_file"] == binding, "The original manifest stream binding changed.")
            _stream_file(store, store / "manifest.json", stream, binding, current)
            _manifest(store, context)
            return {**result, "status": "artifact-manifest-received"}
        manifest, rows, manifest_inputs = _manifest(store, context)
        if action == "file":
            row = next((item for item in rows if item["name"] == request["name"]), None)
            _require(row is not None, "The artifact stream is outside the original complete manifest.")
            binding = {name: row[name] for name in ("size", "sha256")}
            _require(request["transfer_file"] == binding, "The selected file stream binding changed.")
            destination = store / "inputs" / row["name"]
            _directory(destination.parent, Path(context["host"]["home"]))
            def file_current():
                current()
                manifest_inputs.finish()
                _path(str(destination.parent))
            _stream_file(store, destination, stream, binding, file_current)
            return {**result, "status": "artifact-file-received"}
        root = store / "inputs"
        target = relocate_product_target(manifest, root)
        artifacts = selected_runtime_artifacts(target)
        observed = {str(item.path.relative_to(root)): {"name": str(item.path.relative_to(root)),
            "size": item.size, "sha256": item.sha256} for item in artifacts.evidence}
        _require(len(observed) == len(artifacts.evidence) and observed == {row["name"]: row for row in rows},
            "The fresh relocated product reader did not consume the exact complete artifact closure.")
        _inventory(root, observed)
        manifest_inputs.finish()
        current()
        selected_runtime_artifacts(target, verified_artifacts=artifacts)
        selection = product_selection_bytes(target)
        selection_hash = hashlib.sha256(selection).hexdigest()
        final = {**result, "status": "runtime-artifacts-transferred", "artifact_root": str(root),
            "selection_sha256": selection_hash}
        _require(len(_json(final)) <= MAX_RESPONSE_BYTES, "The artifact custody response exceeds its bound.")
        _publish(store / "selection.json", selection)
        selected_runtime_artifacts(target, verified_artifacts=artifacts)
        _publish(store / "custody.json", _json(final))
        current()
        manifest_inputs.finish()
        selected_runtime_artifacts(target, verified_artifacts=artifacts)
        return final


def read_request(stream):
    first = _exact(stream, 1)
    if first == b"{":
        rest = stream.read(MAX_REQUEST_BYTES)
        _require(len(rest) < MAX_REQUEST_BYTES and not stream.read(1), "The artifact request exceeds its bound.")
        return _decode(first + rest), None
    size = struct.unpack("!I", first + _exact(stream, 3))[0]
    _require(0 < size <= MAX_REQUEST_BYTES, "The artifact stream header exceeds its bound.")
    content = _exact(stream, size)
    _require(content.isascii(), "The artifact stream header must be ASCII JSON.")
    return _decode(content), stream


def main():
    try:
        request, stream = read_request(sys.stdin.buffer)
        result = receive(request, stream, receiver_source=CONTROLLER_SOURCE)
    except Exception:
        # Error details and private input locators never enter remote output.
        # Partial private inputs remain uncertain, without a success record.
        result = {"error": "artifact-transfer-unavailable"}
    content = _json(result)
    if len(content) > MAX_RESPONSE_BYTES:
        content = _json({"error": "artifact-transfer-unavailable"})
    sys.stdout.buffer.write(FRAME_PREFIX + content)
    sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()
