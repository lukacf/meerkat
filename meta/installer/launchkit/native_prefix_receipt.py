"""Retained native prefix inventory for product Starts.

A product Start proves that the installed native prefix equals the selected
product's inventory before the native runtime runs with the user's
credentials. Re-deriving that inventory hashes and inflates the whole product
archive on every Start. After a successful full verification Forge keeps the
verified inventory in a private content-addressed file. receipt.json names it
by sha256, and its entries are the exact preimage of the installed proof's
prefix_inventory_sha256. A later Start compares the installed prefix, file by
file as before, with that inventory and does not read the archive.

The anchor stays receipt.json, which a Start already trusts for its package,
selection and installed proof. Forged or changed bytes refuse. A missing,
stale or legacy inventory keeps the full verification, and deleting the file
only ever forces that full path.
"""
from __future__ import annotations

from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import time
import uuid

from .runtime_candidates import (ProductPrefixEntry, RetainedPrefixInventory, _fingerprint,
    prefix_inventory_sha256)
from .runtime_dependencies import RuntimeDependencyError

RECEIPT_FIELD = "native_prefix_receipt_sha256"
SCHEMA = 2
MAX_BYTES = 32 * 1024 * 1024
STALE_PENDING_SECONDS = 3600
# A file changed within this window of its observation could keep the same
# coarse ctime ("racy clean"); such a file is always hashed again.
RACY_NS = 2_000_000_000
# Local disk filesystems where any write, including one through a shared
# mapping after writeback, moves ctime. Elsewhere every file is hashed.
LOCAL_DISK_FILESYSTEMS = frozenset({"ext4", "xfs", "btrfs"})
HEX = re.compile(r"[a-f0-9]{64}")
NAME = re.compile(r"native-prefix-receipt-([a-f0-9]{64})\.json")
PENDING = re.compile(r"\.native-prefix-receipt-[a-f0-9]{32}\.pending")
KEYS = frozenset({"schema", "prefix", "inventory_sha256", "entries", "runtime_candidate_sha256",
    "assembly_sha256", "archive_sha256", "archive_bytes", "fingerprints"})
ENTRY_KEYS = frozenset(ProductPrefixEntry.__dataclass_fields__)


class RetainedInventoryError(RuntimeDependencyError):
    """A present inventory is not the one receipt.json names."""


def _path(directory, named):
    return Path(directory) / f"native-prefix-receipt-{named}.json"


def _refuse(path, problem):
    raise RetainedInventoryError(f"Forge's saved native runtime inventory {path} {problem}. Remove that "
        "file and start again: the Start then runs the full native runtime verification and saves a new one.")


def selection_sha256(runtime_candidate):
    return hashlib.sha256(json.dumps(runtime_candidate, sort_keys=True, separators=(",", ":"),
        ensure_ascii=True, allow_nan=False).encode()).hexdigest()


def installed_inventory_sha256(receipt):
    proof = receipt.get("installed_candidate_runtime_proof")
    value = proof.get("prefix_inventory_sha256") if type(proof) is dict else None
    return value if type(value) is str and HEX.fullmatch(value) else None


def same_selection(receipt, named_sha256):
    try:
        return selection_sha256(receipt.get("runtime_candidate")) == named_sha256
    except (TypeError, ValueError, RecursionError):
        return False


def _unique(pairs):
    result = {}
    for name, value in pairs:
        if name in result:
            raise ValueError("A saved inventory has a duplicate key.")
        result[name] = value
    return result


def _no_constant(value):
    raise ValueError("A saved inventory has a non-finite number.")


def _entry(value):
    if (type(value) is not dict or set(value) != ENTRY_KEYS or type(value["name"]) is not str
            or value["kind"] not in {"directory", "file", "link"}
            or type(value["mode"]) is not int or not 0 <= value["mode"] <= 0o777
            or type(value["size"]) is not int or value["size"] < 0
            or (value["kind"] == "file") != (type(value["sha256"]) is str and bool(HEX.fullmatch(value["sha256"])))
            or (value["kind"] != "file" and value["sha256"] is not None)
            or (value["kind"] == "link") != (type(value["link"]) is str)
            or (value["kind"] != "link" and value["link"] is not None)):
        raise ValueError("A saved inventory entry is malformed.")
    return ProductPrefixEntry(**value)


def load(directory, receipt):
    """Return the inventory receipt.json names for its current selection, or None.

    None (no field, missing file, stale proof or selection, another schema)
    keeps today's full verification. Bytes that differ from the named sha256,
    a file that is not private, or entries that differ from their own digest
    refuse: corruption and forgery look the same.
    """
    named = receipt.get(RECEIPT_FIELD)
    current = installed_inventory_sha256(receipt)
    if type(named) is not str or not HEX.fullmatch(named) or current is None:
        return None
    path = _path(directory, named)
    try:
        before = path.lstat()
    except FileNotFoundError:
        return None
    if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid() or before.st_nlink != 1
            or stat.S_IMODE(before.st_mode) != 0o600 or before.st_size > MAX_BYTES):
        _refuse(path, "is not a private ordinary file")
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        # Removed after the lstat: the full verification runs, as for a missing file.
        return None
    except OSError:
        _refuse(path, "cannot be read")
    try:
        with os.fdopen(descriptor, "rb") as handle:
            opened = os.fstat(handle.fileno())
            content = handle.read(MAX_BYTES + 1)
            unchanged = _fingerprint(before) == _fingerprint(opened) == _fingerprint(os.fstat(handle.fileno()))
    except OSError:
        _refuse(path, "cannot be read")
    if not unchanged or hashlib.sha256(content).hexdigest() != named:
        _refuse(path, "differs from the copy receipt.json names")
    try:
        value = json.loads(content, object_pairs_hook=_unique, parse_constant=_no_constant)
    except (UnicodeError, ValueError, RecursionError):
        _refuse(path, "is not a readable inventory")
    if type(value) is not dict or value.get("schema") != SCHEMA:
        return None
    try:
        if (set(value) != KEYS or type(value["entries"]) is not list or type(value["prefix"]) is not str
                or any(type(value[name]) is not str or not HEX.fullmatch(value[name]) for name in
                    ("inventory_sha256", "runtime_candidate_sha256", "assembly_sha256", "archive_sha256"))
                or type(value["archive_bytes"]) is not int or value["archive_bytes"] <= 0):
            raise ValueError("A saved inventory is malformed.")
        entries = tuple(_entry(item) for item in value["entries"])
        fingerprints = _fingerprints(value["fingerprints"], entries)
    except ValueError:
        _refuse(path, "is not a readable inventory")
    if prefix_inventory_sha256(entries) != value["inventory_sha256"]:
        _refuse(path, "has entries that differ from its own inventory digest")
    if value["inventory_sha256"] != current or not same_selection(receipt, value["runtime_candidate_sha256"]):
        return None
    return RetainedPrefixInventory(entries, value["prefix"], value["assembly_sha256"],
        value["archive_sha256"], value["archive_bytes"], current, named, value["runtime_candidate_sha256"],
        fingerprints)


def _fingerprints(value, entries):
    files = {entry.name for entry in entries if entry.kind == "file"}
    if type(value) is not dict or not set(value) <= files:
        raise ValueError("A saved inventory has fingerprints for no regular file.")
    result = []
    for name in sorted(value):
        row = value[name]
        if type(row) is not list or len(row) != 8 or any(type(item) is not int for item in row):
            raise ValueError("A saved inventory fingerprint is malformed.")
        result.append((name, tuple(row)))
    return tuple(result)


def known_fingerprints(artifacts, prefix):
    """Fingerprints a prefix walk may trust for ``artifacts``, or None to hash all.

    Only an artifacts object read with a kept inventory carries them, and only
    a prefix on an allowed local disk uses them. A file recorded on another
    device than the prefix, and the executed gateway itself, are always hashed.
    """
    fingerprints = getattr(artifacts, "_known_fingerprints", ())
    if (getattr(artifacts, "_inventory_source", "archive") != "retained" or not fingerprints
            or not local_disk_filesystem(prefix)):
        return None
    device, gateway = os.lstat(prefix).st_dev, artifacts.prefix + "/bin/toolkit_gateway"
    known = {name: fingerprint for name, fingerprint in fingerprints
        if fingerprint[0] == device and name != gateway}
    return known or None


def same_device_fingerprints(record, prefix):
    """Keep only a recording walk's fingerprints on the prefix's own device."""
    device = os.lstat(prefix).st_dev
    return {name: value for name, value in record.items() if value[0][0] == device}


def local_disk_filesystem(path):
    """Whether ``path`` lies on a local disk filesystem from the allow-list.

    The type comes from /proc/self/mountinfo by the device's major:minor.
    Anything unknown (no /proc, a btrfs subvolume device, overlay, tmpfs,
    FUSE or network mounts) is not allowed, and its files are always hashed.
    """
    try:
        device = os.stat(path).st_dev
        wanted = f"{os.major(device)}:{os.minor(device)}"
        with open("/proc/self/mountinfo", encoding="utf-8") as mounts:
            for line in mounts:
                fields = line.split()
                if len(fields) > 2 and fields[2] == wanted and " - " in line:
                    return line.split(" - ", 1)[1].split()[0] in LOCAL_DISK_FILESYSTEMS
    except (OSError, UnicodeError, ValueError, IndexError):
        return False
    return False


def mint(directory, artifacts, *, proof, runtime_candidate, fingerprints=None):
    """Keep the inventory of a successful full verification; return its sha256.

    Callers mint only after that verification (and an install's application
    check) returned, never on a failure path. They name the returned sha256 in
    receipt.json only after this call, when the file is already durable.
    Inventories read from a kept copy are never minted again.
    ``fingerprints`` are that hashed walk's (fingerprint, observed time) pairs,
    taken after an fsync; a racy one (ctime within RACY_NS of its
    observation) is left out, so that file is always hashed again.
    """
    inventory = prefix_inventory_sha256(artifacts.entries)
    if (artifacts._inventory_source != "archive" or type(proof) is not dict
            or proof.get("prefix_inventory_sha256") != inventory
            or proof.get("archive_sha256") != artifacts.archive.sha256
            or proof.get("assembly_sha256") != artifacts.selection.assembly.sha256):
        return None
    files = {entry.name for entry in artifacts.entries if entry.kind == "file"}
    kept = {name: list(fingerprint) for name, (fingerprint, observed) in (fingerprints or {}).items()
        if name in files and fingerprint[7] < observed - RACY_NS}
    content = (json.dumps({"schema": SCHEMA, "prefix": artifacts.prefix, "inventory_sha256": inventory,
        "entries": [asdict(entry) for entry in artifacts.entries],
        "runtime_candidate_sha256": selection_sha256(runtime_candidate),
        "assembly_sha256": artifacts.selection.assembly.sha256,
        "archive_sha256": artifacts.archive.sha256, "archive_bytes": artifacts.archive.size,
        "fingerprints": kept},
        sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False) + "\n").encode()
    if len(content) > MAX_BYTES:
        return None
    named = hashlib.sha256(content).hexdigest()
    directory = Path(directory)
    pending = directory / f".native-prefix-receipt-{uuid.uuid4().hex}.pending"
    descriptor = os.open(pending, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(pending, _path(directory, named))
        folder = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(folder)
        finally:
            os.close(folder)
    finally:
        pending.unlink(missing_ok=True)
    return named


def record(directory, named, *, inventory_sha256, runtime_candidate):
    """Name a kept inventory in receipt.json if it still selects that product.

    Like update_receipt(), this changes only the field and updated_at under
    the receipt lock, so a Start's journal detail and progress stay intact.
    """
    from .deployment import now, read_receipt, receipt_lock
    from .templates.storage_setup import write_private
    with receipt_lock(directory):
        current = read_receipt(directory)
        if (installed_inventory_sha256(current) != inventory_sha256
                or not same_selection(current, selection_sha256(runtime_candidate))):
            return False
        current.update({RECEIPT_FIELD: named}, updated_at=now())
        write_private(Path(directory) / "receipt.json", current)
        _prune(directory, named)
    return True


def prune(directory):
    """Remove kept inventories receipt.json no longer names."""
    from .deployment import read_receipt, receipt_lock
    with receipt_lock(directory):
        _prune(directory, read_receipt(directory).get(RECEIPT_FIELD))


def _prune(directory, keep):
    cutoff = time.time() - STALE_PENDING_SECONDS
    with os.scandir(directory) as items:
        for item in items:
            match = NAME.fullmatch(item.name)
            if ((match is not None and match.group(1) != keep)
                    or (PENDING.fullmatch(item.name) and item.stat(follow_symlinks=False).st_mtime < cutoff)):
                os.unlink(item.path)
