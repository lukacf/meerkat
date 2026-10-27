"""Retain an installed runtime before a stopped, caller-leased dependency refresh.

This module reads receipts and filesystem bytes. Candidate SDK verification uses
the selected Python's stdlib helpers, without importing the SDK or executing a
native tool. It never downloads dependencies or copies application/native
databases. The caller owns the stopped-process boundary and project lease.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import stat
import uuid

from .bootstrap import platform_target

ROOTS = (".bin", ".venv")
INVENTORY = "runtime-inventory.json"
COMPLETED = "runtime-retained.json"
BACKUP = "runtime-before"
FORMAT = "mobkit-launch-runtime-retention-v1"
MAX_RUNTIME_BYTES = 2 * 1024 * 1024 * 1024
MAX_RUNTIME_ENTRIES = 50_000
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_INVENTORY_BYTES = 16 * 1024 * 1024
MAX_RECEIPT_BYTES = 1024 * 1024
HEX = re.compile(r"[a-f0-9]{64}")
PYTHON_NAME = re.compile(r"python(?:3(?:\.[0-9]+)?)?")
PYTHON314_ALIAS = "\U0001d70bthon"
BINARIES = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}


class RuntimeDependencyError(RuntimeError):
    """Runtime bytes or the exact retained transaction could not be verified."""


def _unique(pairs):
    result = {}
    for name, value in pairs:
        if name in result:
            raise RuntimeDependencyError("The runtime receipt or inventory has duplicate keys.")
        result[name] = value
    return result


def _directory(path, *, private=False, device=None):
    path = Path(path).absolute()
    if any(parent.is_symlink() for parent in (path, *path.parents)):
        raise RuntimeDependencyError("The runtime or transaction path contains a link.")
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or private and info.st_mode & 0o077 or device is not None and info.st_dev != device):
        raise RuntimeDependencyError("The runtime and transaction need owned ordinary folders on one filesystem.")
    return info


def _fingerprint(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _file(path, limit, *, private=False, executable=False):
    before = Path(path).lstat()
    if not stat.S_ISREG(before.st_mode):
        raise RuntimeDependencyError("The runtime receipt or inventory is not an ordinary file.")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if (_fingerprint(before) != _fingerprint(info) or not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or not 0 < info.st_size <= limit or private and info.st_mode & 0o077
                or executable and not info.st_mode & stat.S_IXUSR):
            raise RuntimeDependencyError("The runtime receipt or executable is not an owned bounded ordinary file.")
        value = bytearray()
        while chunk := os.read(descriptor, min(1024 * 1024, limit + 1 - len(value))):
            value.extend(chunk)
            if len(value) > limit:
                raise RuntimeDependencyError("The runtime receipt or executable exceeds its bound.")
        if len(value) != info.st_size or _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
            raise RuntimeDependencyError("The runtime file changed during verification.")
        return bytes(value)
    finally:
        os.close(descriptor)


def _json(path, limit):
    if path.name in (INVENTORY, COMPLETED):
        _reconcile_publication(path)
    try:
        return json.loads(_file(path, limit, private=True), object_pairs_hook=_unique)
    except (ValueError, UnicodeError):
        raise RuntimeDependencyError("The saved runtime receipt or inventory is invalid.") from None


def _hash_executable(path):
    before = Path(path).lstat()
    if not stat.S_ISREG(before.st_mode):
        raise RuntimeDependencyError("The runtime executable is not an ordinary file.")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if (_fingerprint(before) != _fingerprint(info) or not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or not 0 < info.st_size <= MAX_FILE_BYTES or info.st_mode & 0o077
                or not info.st_mode & stat.S_IXUSR):
            raise RuntimeDependencyError("The runtime executable is not an owned private bounded ordinary file.")
        hashed, size = hashlib.sha256(), 0
        while chunk := os.read(descriptor, 1024 * 1024):
            size += len(chunk)
            if size > info.st_size:
                raise RuntimeDependencyError("The runtime executable changed during verification.")
            hashed.update(chunk)
        if size != info.st_size or _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
            raise RuntimeDependencyError("The runtime executable changed during verification.")
        return hashed.hexdigest()
    finally:
        os.close(descriptor)


def _verify_candidate_runtime(destination, expected_version, runtime_target):
    """Verify an explicit host-accepted target, never admit an installed claim."""
    from subprocess import SubprocessError
    from .runtime_candidates import CandidateBinding, read_candidate_artifacts
    from .runtime_selection import RuntimeCandidateTarget, candidate_selection, parse_candidate_selection
    from .templates import install_candidate as adapter

    def exact(left, right):
        return json.dumps(left, sort_keys=True, separators=(",", ":"), allow_nan=False) == json.dumps(
            right, sort_keys=True, separators=(",", ":"), allow_nan=False)

    try:
        # The caller's independently held target is authority. Installed source
        # must match it; a receipt or selection document cannot choose another.
        if not isinstance(runtime_target, RuntimeCandidateTarget):
            raise ValueError("A candidate target needs an explicit host-owned selection.")
        binding = runtime_target.accepted
        if (not isinstance(binding, CandidateBinding) or binding.native_version != "0.8.48"
                or binding.core_version != "0.8.51"):
            raise ValueError("The selected candidate must be the qualified .48/Core .51 tuple.")
        # Keep the full tuple result only within this synchronous verification.
        # The reader still qualifies all three native archives, including repair.
        artifacts = read_candidate_artifacts(runtime_target.tuple_path,
            accepted=binding, artifact_root=runtime_target.artifact_root)
        selection = candidate_selection(runtime_target, verified_artifacts=artifacts)
        identity = selection["accepted"]
        target = platform_target()
        if (expected_version != identity["native_version"] or expected_version != "0.8.48"
                or identity["core_version"] != "0.8.51" or identity["platform"] != target):
            raise RuntimeDependencyError("The selected candidate does not match this exact native version, Core and local platform.")
        destination = Path(destination).absolute()
        _directory(destination, private=True)
        directory = destination / ".bin"
        _directory(directory, private=True)
        selection_path = destination / "runtime-candidate.json"
        deployment_path = destination / "deployment.json"
        selection_bytes = _file(selection_path, MAX_RECEIPT_BYTES)
        deployment_bytes = _file(deployment_path, MAX_RECEIPT_BYTES)
        installed_selection = adapter.unique_json(selection_bytes)
        parse_candidate_selection(installed_selection)
        deployment = adapter.unique_json(deployment_bytes)
        if (not exact(installed_selection, selection)
                or deployment.get("mobkitVersion") != expected_version
                or deployment.get("target") != ("mac" if target.endswith("-apple-darwin") else "linux")
                or not exact(deployment.get("runtimeCandidate"), {"source": "signed-candidate",
                    "selection": "runtime-candidate.json", "tuple_sha256": identity["tuple_sha256"]})):
            raise RuntimeDependencyError("The installed candidate selection or deployment differs from the independently accepted target.")
        binding = {"candidate_binding": identity, "candidate_tuple_sha256": identity["tuple_sha256"],
            "sdk_wheel_sha256": identity["sdk_wheel_sha256"], "source_sha": identity["source_sha"],
            "native_run_id": identity["native_run_id"], "native_run_attempt": identity["native_run_attempt"]}
        receipts = {name: _file(directory / (name + ".receipt.json"), MAX_RECEIPT_BYTES, private=True)
                    for name in BINARIES}
        actual = adapter.verify_installed_candidate_runtime(selection_path,
            expected_native_version=expected_version, expected_platform=target, verified_artifacts=artifacts)
        if (actual.get("source") != "signed-candidate" or actual.get("version") != expected_version
                or actual.get("platform") != target
                or any(not exact(actual.get(key), value) for key, value in binding.items())
                or set(actual.get("native", {})) != set(BINARIES)):
            raise RuntimeDependencyError("The installed candidate proof differs from the independently accepted identity.")
        sdk = actual["sdk"]
        sdk_receipt = adapter.unique_json(_file(destination / ".venv/forge-sdk.receipt.json",
            MAX_RECEIPT_BYTES, private=True))
        if (not exact(sdk, sdk_receipt) or sdk.get("source") != "local-build-from-exact-frozen-source"
                or sdk.get("signed") is not False or sdk.get("published") is not False
                or sdk.get("version") != expected_version
                or any(not exact(sdk.get(key), value) for key, value in binding.items())):
            raise RuntimeDependencyError("The installed candidate SDK proof does not retain its exact source and payload identity.")
        proofs = {}
        for name in BINARIES:
            record = adapter.unique_json(receipts[name])
            if (not exact(record, actual["native"][name]) or record.get("source") != "signed-candidate"
                    or record.get("version") != expected_version or record.get("platform") != target
                    or any(not exact(record.get(key), value) for key, value in binding.items())):
                raise RuntimeDependencyError("The installed candidate native receipt changed or identifies another target.")
            hashed = _hash_executable(directory / name)
            if (hashed != record["binary_sha256"]
                    or _file(directory / (name + ".receipt.json"), MAX_RECEIPT_BYTES, private=True) != receipts[name]):
                raise RuntimeDependencyError("The verified candidate native bytes changed during verification.")
            proofs[name] = {"receipt_sha256": hashlib.sha256(receipts[name]).hexdigest(),
                "binary_sha256": hashed, "archive": record["archive"], "archive_sha256": record["archive_sha256"]}
        late_sdk = adapter.verify_installed_candidate_sdk(selection_path,
            expected_native_version=expected_version, expected_platform=target, verified_artifacts=artifacts)
        if not exact(late_sdk, sdk):
            raise RuntimeDependencyError("The candidate SDK payload or metadata changed during runtime verification.")
        if (_file(selection_path, MAX_RECEIPT_BYTES) != selection_bytes
                or _file(deployment_path, MAX_RECEIPT_BYTES) != deployment_bytes):
            raise RuntimeDependencyError("The protected installed candidate source changed during verification.")
        candidate_selection(runtime_target, verified_artifacts=artifacts)
        return {"version": expected_version, "platform": target, "core_version": identity["core_version"],
            "source": "signed-candidate", "binaries": proofs, **binding, "sdk": sdk}
    except RuntimeDependencyError:
        raise
    except (OSError, ValueError, TypeError, KeyError, RuntimeError, SubprocessError) as error:
        raise RuntimeDependencyError("The explicit candidate SDK/native runtime could not be verified.") from error


def verify_runtime(destination, expected_version, *, runtime_target=None, sdk_python=None,
        site_packages=None, source_custody=None, report_progress=None, retained=None, observation=None):
    """Check an explicit candidate or the unchanged ordinary native receipts.

    Ordinary SDK proof remains caller-owned. An independently held candidate
    target requires exact SDK payload proof as well as both native receipts.
    Neither branch runs a native binary or admits an installed source claim.
    """
    if runtime_target is not None:
        from .runtime_selection import MetaToolkitRuntimeTarget
        if type(runtime_target) is MetaToolkitRuntimeTarget:
            return _verify_meta_toolkit_runtime(destination, expected_version, runtime_target,
                sdk_python=sdk_python, site_packages=site_packages, source_custody=source_custody,
                report_progress=report_progress, retained=retained, observation=observation)
        if source_custody is not None:
            raise RuntimeDependencyError("Relocated custody cannot select an ordinary or private candidate runtime.")
    if retained is not None:
        raise RuntimeDependencyError("A retained native inventory belongs to a selected native product only.")
    if runtime_target is not None:
        return _verify_candidate_runtime(destination, expected_version, runtime_target)
    if source_custody is not None:
        raise RuntimeDependencyError("Relocated custody needs its original admitted native product selection.")
    if expected_version not in ("0.8.42", "0.8.46", "0.8.47", "0.8.48"):
        raise RuntimeDependencyError("This native dependency version is not supported.")
    destination = Path(destination).absolute()
    _directory(destination, private=True)
    directory = destination / ".bin"
    _directory(directory, private=True)
    target = platform_target()
    release = f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{expected_version}"
    proofs = {}
    for name, prefix in BINARIES.items():
        receipt = _file(directory / (name + ".receipt.json"), MAX_RECEIPT_BYTES, private=True)
        try:
            value = json.loads(receipt, object_pairs_hook=_unique)
        except (ValueError, UnicodeError):
            raise RuntimeDependencyError("The official native receipt is invalid.") from None
        if (not isinstance(value, dict) or value.get("version") != expected_version
                or value.get("archive") != f"{prefix}-{expected_version}-{target}.tar.gz"
                or value.get("release_url") != release or value.get("source") is not None
                or not isinstance(value.get("archive_sha256"), str) or not HEX.fullmatch(value["archive_sha256"])
                or not isinstance(value.get("binary_sha256"), str) or not HEX.fullmatch(value["binary_sha256"])
                or expected_version in {"0.8.46", "0.8.47", "0.8.48"} and "core_version" in value and value["core_version"] != "0.8.51"):
            raise RuntimeDependencyError("The native receipt does not identify this exact official release and platform.")
        hashed = _hash_executable(directory / name)
        if hashed != value["binary_sha256"]:
            raise RuntimeDependencyError("The native executable changed outside its official receipt.")
        if _file(directory / (name + ".receipt.json"), MAX_RECEIPT_BYTES, private=True) != receipt:
            raise RuntimeDependencyError("The native receipt changed during verification.")
        proofs[name] = {"receipt_sha256": hashlib.sha256(receipt).hexdigest(), "binary_sha256": hashed,
            "archive": value["archive"], "archive_sha256": value["archive_sha256"], "release_url": release}
    result = {"version": expected_version, "platform": target, "source": "official-release", "binaries": proofs}
    if expected_version in {"0.8.46", "0.8.47", "0.8.48"}:
        result["core_version"] = "0.8.51"
    return result


def _product_installed_observation(artifacts, native_root, site, facts, source_paths):
    """Retain just the selected installed trees across this readback.

    Full byte/mode/inventory admission remains in the original comparators.
    These filesystem observations keep their results current through completion;
    unrelated dependency packages and environment files are outside this fence.
    """
    directories, nodes, distributions = {}, {}, {}
    roots = [native_root / ".bin" / artifacts.prefix]
    for fact in facts:
        package = fact.distribution.replace("-", "_")
        roots.extend((site / package, site / f"{package}-{fact.version}.dist-info"))
        pattern = re.compile(r"(?i)" + r"[-_.]+".join(package.split("_"))
            + r"(?:-.*)?\.(dist-info|egg-info)")
        distributions[pattern.pattern] = tuple(sorted(path.name for path in site.iterdir()
            if pattern.fullmatch(path.name)))
    def observe(path):
        info = path.lstat()
        if info.st_uid != os.getuid() or len(nodes) >= MAX_RUNTIME_ENTRIES:
            raise RuntimeDependencyError("The selected installed runtime observation is unowned or oversized.")
        nodes[path] = _fingerprint(info)
        if stat.S_ISDIR(info.st_mode):
            for child in sorted(path.iterdir()):
                observe(child)
    for root in roots:
        # Parent identity/access stays fixed without admitting other site packages
        # or coupling their unrelated directory timestamps to selected SDK proof.
        for parent in (root.parent, *root.parent.parents):
            info = parent.lstat()
            if not stat.S_ISDIR(info.st_mode):
                raise RuntimeDependencyError("The selected installed runtime has a changed enclosing path.")
            directories[parent] = (info.st_dev, info.st_ino, info.st_mode, info.st_uid)
        observe(root)
    for path in source_paths:
        nodes[path] = _fingerprint(path.lstat())
    return directories, nodes, distributions


def _require_product_installed_observation(observed, artifacts, native_root, site, facts, source_paths):
    if _product_installed_observation(artifacts, native_root, site, facts, source_paths) != observed:
        raise RuntimeDependencyError("The selected installed source, native prefix or SDK changed during verification.")


def _verify_meta_toolkit_runtime(destination, expected_version, target, *, sdk_python=None, site_packages=None,
                                previous_source=None, native_root=None, sdk_root=None, source_custody=None,
                                report_progress=None, retained=None, observation=None):
    """Read exact SDK and full prefix bytes; native startup keeps its authority.

    ``retained`` (a Start without a carrier) replaces only the archive inflate.
    ``observation`` receives the verified artifacts for a later inventory mint.
    """
    from dataclasses import asdict
    from .runtime_candidates import prefix_inventory_sha256
    from .runtime_selection import (candidate_selection, selected_runtime_artifacts,
        selected_runtime_marker, selected_runtime_platform)
    from .sdk_wheel_integrity import WheelIntegrityFacts
    from .templates import install_candidate as adapter
    artifacts = selected_runtime_artifacts(target, source_custody=source_custody, destination=destination,
        **({"retained": retained} if retained is not None else {}))
    selected = candidate_selection(target, verified_artifacts=artifacts,
        source_custody=source_custody, destination=destination)
    if expected_version != target.meta.version or selected_runtime_platform(target) != platform_target():
        raise RuntimeDependencyError("The selected native product differs from this exact local target.")
    destination = Path(destination).absolute()
    _directory(destination, private=True)
    source_paths = () if previous_source is not None else (
        destination / "deployment.json", destination / "runtime-candidate.json")
    source_observation = {path: _fingerprint(path.lstat()) for path in source_paths}
    if previous_source is None:
        deployed = adapter.unique_json(_file(destination / "deployment.json", MAX_RECEIPT_BYTES))
        installed = adapter.unique_json(_file(destination / "runtime-candidate.json", MAX_INVENTORY_BYTES))
    else:
        if (type(previous_source) is not dict
                or set(previous_source) != {"deployment.json", "runtime-candidate.json"}):
            raise RuntimeDependencyError("Retention lost its original admitted product source.")
        deployed = adapter.unique_json(previous_source["deployment.json"])
        installed = adapter.unique_json(previous_source["runtime-candidate.json"])
    if (installed != selected or deployed.get("mobkitVersion") != expected_version
            or deployed.get("runtimeCandidate") != selected_runtime_marker(target)):
        raise RuntimeDependencyError("The installed native product differs from the admitted source selection.")
    native_root, sdk_root = Path(native_root or destination), Path(sdk_root or destination)
    facts = tuple(WheelIntegrityFacts(**asdict(item)) for item in (target.meta, target.toolkit))
    python, site, _ = adapter._sdk_location(sdk_root, sdk_python or sdk_root / ".venv/bin/python", site_packages)
    observed = _product_installed_observation(artifacts, native_root, site, facts, source_paths)
    if any(observed[1][path] != identity for path, identity in source_observation.items()):
        raise RuntimeDependencyError("The original installed product selection changed during verification.")
    # F2: a kept inventory's fingerprints skip re-reading unchanged files, and a
    # full walk that may be kept records them, only on an allowed local disk.
    from .native_prefix_receipt import known_fingerprints, local_disk_filesystem, same_device_fingerprints
    prefix = native_root / ".bin" / artifacts.prefix
    known = known_fingerprints(artifacts, prefix)
    record = ({} if observation is not None and artifacts._inventory_source == "archive"
        and local_disk_filesystem(prefix) else None)
    adapter.verify_meta_toolkit_prefix(artifacts, deployment_root=native_root,
        **({"report_progress": report_progress} if report_progress is not None else {}),
        **({"known": known} if known is not None else {}), **({"record": record} if record is not None else {}))
    actual = adapter.compare_installed_meta_toolkit_wheels(*facts, deployment_root=sdk_root,
        site_packages=site, sdk_python=python)
    selected_runtime_artifacts(target, verified_artifacts=artifacts, source_custody=source_custody,
        destination=destination)
    # These are the existing transaction's readback fields, not a new receipt.
    inventory = prefix_inventory_sha256(artifacts.entries)
    from .runtime_selection import _json_paths
    sdk = _json_paths({"meta": asdict(actual[0]), "toolkit": asdict(actual[1])})
    result = {"source": "meta-toolkit-product", "version": expected_version,
        "platform": selected_runtime_platform(target), "assembly_sha256": target.assembly.sha256,
        "archive_sha256": artifacts.archive.sha256, "resources_sha256": artifacts.candidate["resources_sha256"],
        "prefix_inventory_sha256": inventory, "catalog_sha256": target.model_catalog.sha256,
        "binaries": {"toolkit_gateway": {"binary_sha256": artifacts.candidate["binary"]["sha256"]}},
        "sdk": sdk}
    _require_product_installed_observation(observed, artifacts, native_root, site, facts, source_paths)
    if observation is not None:
        observation.update(artifacts=artifacts, inventory_source=artifacts._inventory_source,
            fingerprints=same_device_fingerprints(record, prefix) if record else {})
    return result


def _interpreter_link(path, target):
    if (path.parent.name != "bin" or path.parent.parent.name != ".venv"
            or not (PYTHON_NAME.fullmatch(path.name) or path.name == PYTHON314_ALIAS)
            or not target or len(target) > 4096):
        raise RuntimeDependencyError("Only normal Python interpreter links in .venv/bin can be retained.")
    # Python 3.14's UTF-8 venv setup adds this exact interpreter alias.
    if path.name == PYTHON314_ALIAS and target != "python3.14":
        raise RuntimeDependencyError("The Python 3.14 interpreter alias must point exactly to python3.14.")
    selected = Path(target)
    if not selected.is_absolute() and (selected.name != target or not PYTHON_NAME.fullmatch(target)):
        raise RuntimeDependencyError("A relative interpreter link must stay inside .venv/bin.")
    try:
        resolved = path.resolve(strict=True)
        info = resolved.lstat()
        if (not PYTHON_NAME.fullmatch(resolved.name) or not stat.S_ISREG(info.st_mode)
                or info.st_uid not in {0, os.getuid()} or not info.st_mode & 0o111):
            raise ValueError
    except (OSError, ValueError, RuntimeError):
        raise RuntimeDependencyError("The retained interpreter link does not resolve to an ordinary Python executable.") from None


def _scan(root, name, device, *, product_links=None):
    """Inventory one directory without following any runtime directory links."""
    result, total = {}, 0
    def walk(path, relative, depth):
        nonlocal total
        if depth > 64 or len(relative) > 4096 or len(result) >= MAX_RUNTIME_ENTRIES:
            raise RuntimeDependencyError("The runtime inventory exceeds its bounded size.")
        info = path.lstat()
        if info.st_uid != os.getuid() or info.st_dev != device:
            raise RuntimeDependencyError("The runtime contains an unowned or cross-filesystem path.")
        entry = {"mode": stat.S_IMODE(info.st_mode), "device": info.st_dev, "inode": info.st_ino}
        if stat.S_ISDIR(info.st_mode):
            entry["kind"] = "directory"
            result[relative] = entry
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
            try:
                if _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
                    raise RuntimeDependencyError("The runtime folder changed during inventory.")
                names = sorted(item.name for item in os.scandir(descriptor))
                for child in names:
                    walk(path / child, relative + "/" + child, depth + 1)
                if _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
                    raise RuntimeDependencyError("The runtime folder changed during inventory.")
            finally:
                os.close(descriptor)
        elif stat.S_ISREG(info.st_mode):
            if info.st_nlink != 1 or not 0 <= info.st_size <= MAX_FILE_BYTES:
                raise RuntimeDependencyError("The runtime contains a hardlink or oversized file.")
            total += info.st_size
            if total > MAX_RUNTIME_BYTES:
                raise RuntimeDependencyError("The runtime inventory exceeds its bounded size.")
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            try:
                if _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
                    raise RuntimeDependencyError("The runtime file changed during inventory.")
                hashed, count = hashlib.sha256(), 0
                while chunk := os.read(descriptor, 1024 * 1024):
                    count += len(chunk)
                    if count > info.st_size:
                        raise RuntimeDependencyError("The runtime file changed during inventory.")
                    hashed.update(chunk)
                if count != info.st_size or _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
                    raise RuntimeDependencyError("The runtime file changed during inventory.")
            finally:
                os.close(descriptor)
            entry.update(kind="file", size=info.st_size, sha256=hashed.hexdigest())
            result[relative] = entry
        elif stat.S_ISLNK(info.st_mode):
            if info.st_nlink != 1:
                raise RuntimeDependencyError("The runtime contains a hardlinked interpreter link.")
            target = os.readlink(path)
            selected = (product_links or {}).get(relative)
            if selected is not None:
                if (name != ".bin" or selected.kind != "link" or target != selected.link
                        or stat.S_IMODE(info.st_mode) != selected.mode):
                    raise RuntimeDependencyError("The selected retained product link changed.")
                kind = "selected-product-link"
            elif path.name == "lib64" and path.parent.name == ".venv":
                if target != "lib":
                    raise RuntimeDependencyError("The .venv/lib64 alias must point exactly to its owned lib directory.")
                _directory(path.parent / "lib", device=device)
                kind = "venv-library-link"
            else:
                _interpreter_link(path, target)
                kind = "interpreter-link"
            if _fingerprint(info) != _fingerprint(path.lstat()):
                raise RuntimeDependencyError("The interpreter link changed during inventory.")
            entry.update(kind=kind, target=target)
            result[relative] = entry
        else:
            raise RuntimeDependencyError("The runtime contains a special file. Its bytes were not moved.")
    walk(root, name, 0)
    return result, total


def _fsync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _create_json(path, value):
    content = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(content) > MAX_INVENTORY_BYTES:
        raise RuntimeDependencyError("The runtime inventory exceeds its bounded size.")
    digest = hashlib.sha256(content).hexdigest()
    temporary = path.with_name(f".{path.name}.publish-{uuid.uuid4().hex}-{digest}")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    identity = os.fstat(descriptor)
    published = False
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        # link() atomically refuses an existing authority file. A crash between
        # publication and temp unlink is reconciled only by exact inode/hash.
        os.link(temporary, path, follow_symlinks=False)
        published = True
        _fsync_directory(path.parent)
        _unlink_publication(temporary, identity)
        _fsync_directory(path.parent)
    finally:
        if not published:
            _unlink_publication(temporary, identity)
    return digest


def _unlink_publication(path, identity):
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077
            or (info.st_dev, info.st_ino) != (identity.st_dev, identity.st_ino)):
        raise RuntimeDependencyError("The private runtime publication temporary file changed. It was preserved.")
    path.unlink()


def _reconcile_publication(path):
    """Finish only this authority's exact interrupted exclusive publication."""
    info = path.lstat()
    if info.st_nlink == 1:
        return
    if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 2 or info.st_uid != os.getuid()
            or info.st_mode & 0o077 or not 0 < info.st_size <= MAX_INVENTORY_BYTES):
        raise RuntimeDependencyError("The runtime authority contains an unexpected hardlink.")
    _directory(path.parent, private=True)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        if _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
            raise RuntimeDependencyError("The runtime publication changed before recovery.")
        content = bytearray()
        while chunk := os.read(descriptor, min(1024 * 1024, MAX_INVENTORY_BYTES + 1 - len(content))):
            content.extend(chunk)
            if len(content) > MAX_INVENTORY_BYTES:
                raise RuntimeDependencyError("The runtime publication exceeds its bounded size.")
        if len(content) != info.st_size or _fingerprint(info) != _fingerprint(os.fstat(descriptor)):
            raise RuntimeDependencyError("The runtime publication changed during recovery.")
        digest = hashlib.sha256(content).hexdigest()
        pattern = re.compile(re.escape("." + path.name + ".publish-") + r"[a-f0-9]{32}-" + digest)
        candidates = []
        for index, temporary in enumerate(path.parent.glob("." + path.name + ".publish-*")):
            if index >= 64:
                raise RuntimeDependencyError("The runtime publication has too many pending temporary files.")
            candidate = temporary.lstat()
            if (candidate.st_dev, candidate.st_ino) == (info.st_dev, info.st_ino):
                if (not pattern.fullmatch(temporary.name) or not stat.S_ISREG(candidate.st_mode)
                        or candidate.st_uid != os.getuid() or candidate.st_nlink != 2
                        or candidate.st_mode & 0o077):
                    raise RuntimeDependencyError("The runtime authority hardlink is not its exact publication temporary file.")
                candidates.append(temporary)
        if len(candidates) != 1 or _fingerprint(info) != _fingerprint(path.lstat()):
            raise RuntimeDependencyError("The runtime authority has an unrelated hardlink. It was preserved.")
        _unlink_publication(candidates[0], info)
        _fsync_directory(path.parent)
    finally:
        os.close(descriptor)


def _rename_root(source, retained):
    # The caller holds the stopped project's exclusive lease. Retained roots
    # are never removed, replaced, or treated as a new previous runtime.
    if retained.exists() or retained.is_symlink():
        raise RuntimeDependencyError("A retained runtime root already exists. No bytes were overwritten.")
    os.rename(source, retained)
    _fsync_directory(source.parent)
    _fsync_directory(retained.parent)


def _verify_previous_candidate(destination, native_root, sdk_root, version, target, source):
    """Check retained bytes against the caller's admitted previous.zip selection.

    Source activation can replace the current package before old runtime files
    move. This uses the original predecessor fields, not the successor source
    or an installed receipt as a selector. Native and SDK integrity reuse the
    existing selected-byte readers; no runtime or qualification is executed.
    """
    from .runtime_selection import candidate_selection
    from .templates import install_candidate as adapter
    selected = candidate_selection(target)
    if (not isinstance(source, dict) or set(source) != {"deployment.json", "runtime-candidate.json"}
            or adapter.unique_json(source["runtime-candidate.json"]) != selected):
        raise RuntimeDependencyError("Retention lost the admitted predecessor source selection.")
    deployment = adapter.unique_json(source["deployment.json"])
    identity = selected["accepted"]
    if (version != identity["native_version"] or identity["platform"] != platform_target()
            or deployment.get("mobkitVersion") != version
            or deployment.get("runtimeCandidate") != {"source": "signed-candidate",
                "selection": "runtime-candidate.json", "tuple_sha256": identity["tuple_sha256"]}):
        raise RuntimeDependencyError("Retention changed the admitted predecessor identity.")
    from .runtime_selection import selected_runtime_artifacts
    artifacts = selected_runtime_artifacts(target)
    context = adapter.CandidateSelection(Path(sdk_root) / "runtime-candidate.json", Path(sdk_root),
        target.tuple_path, target.artifact_root, target.accepted, artifacts)
    # The SDK receipt keeps its original installation path after the roots move.
    # Read actual retained bytes there, then compare the original logical path.
    python, site, _ = adapter.sdk_location(context)
    sdk = adapter.sdk_proof(context, site, python)
    sdk["site_packages"] = str(Path(destination) / site.relative_to(sdk_root))
    if adapter.unique_json(_file(Path(sdk_root) / ".venv/forge-sdk.receipt.json",
            MAX_RECEIPT_BYTES, private=True)) != sdk:
        raise RuntimeDependencyError("The admitted predecessor SDK bytes or receipt changed.")
    proofs = {}
    for artifact in artifacts.native:
        if artifact.binary not in BINARIES:
            continue
        path = Path(native_root) / ".bin" / artifact.binary
        receipt = path.with_name(path.name + ".receipt.json")
        raw = _file(receipt, MAX_RECEIPT_BYTES, private=True)
        record = adapter.unique_json(raw)
        if record != adapter.native_receipt(context, artifact) or _hash_executable(path) != artifact.inner_sha256:
            raise RuntimeDependencyError("The admitted predecessor native bytes or receipt changed.")
        proofs[artifact.binary] = {"receipt_sha256": hashlib.sha256(raw).hexdigest(),
            "binary_sha256": artifact.inner_sha256, "archive": artifact.archive.path.name,
            "archive_sha256": artifact.archive.sha256}
    if set(proofs) != set(BINARIES):
        raise RuntimeDependencyError("The admitted predecessor lost one selected native tool.")
    return {"version": version, "platform": identity["platform"], "core_version": identity["core_version"],
        "source": "signed-candidate", "binaries": proofs, **adapter.binding_fields(context), "sdk": sdk}


def retain_runtime(destination, transaction_root, *, expected_version="0.8.42",
                   runtime_target=None, next_runtime_target=None, previous_source=None, require_current=None):
    """Retain the qualified previous roots once, resuming interrupted moves.

    New candidate roots can exist only after the exact retained backup has a
    durable completion marker. A malformed journal refuses without discarding
    either runtime. The SDK version is intentionally not inferred here.
    """
    from .runtime_selection import MetaToolkitRuntimeTarget
    product_update = type(next_runtime_target) is MetaToolkitRuntimeTarget
    product_predecessor = product_update and type(runtime_target) is MetaToolkitRuntimeTarget
    product_links = None
    def require_retention_current():
        if require_current is not None and require_current() is False:
            raise RuntimeDependencyError("The original runtime retention owner changed.")
    def previous_proof(native_root, sdk_root):
        require_retention_current()
        if product_predecessor:
            proof = _verify_meta_toolkit_runtime(destination, expected_version, runtime_target,
                previous_source=previous_source, native_root=native_root, sdk_root=sdk_root)
        elif product_update and runtime_target is not None:
            proof = _verify_previous_candidate(destination, native_root, sdk_root,
                expected_version, runtime_target, previous_source)
        else:
            proof = verify_runtime(native_root, expected_version, runtime_target=runtime_target)
        require_retention_current()
        return proof
    require_retention_current()
    if not product_predecessor and expected_version not in {"0.8.42", "0.8.46", "0.8.47"} and not (
            expected_version == "0.8.48" and product_update):
        raise RuntimeDependencyError("The retained predecessor release is unsupported.")
    if product_predecessor:
        if previous_source is None or expected_version != runtime_target.meta.version:
            raise RuntimeDependencyError("The product predecessor requires its original admitted source/version.")
        from .runtime_selection import selected_runtime_artifacts
        previous_artifacts = selected_runtime_artifacts(runtime_target)
        product_links = {".bin/" + entry.name: entry for entry in previous_artifacts.entries if entry.kind == "link"}
    destination, transaction_root = Path(destination).absolute(), Path(transaction_root).absolute()
    device = _directory(destination, private=True).st_dev
    _directory(transaction_root, private=True, device=device)
    if transaction_root == destination or transaction_root.is_relative_to(destination / ".bin") or transaction_root.is_relative_to(destination / ".venv"):
        raise RuntimeDependencyError("The runtime transaction must be outside the retained runtime roots.")
    backup = transaction_root / BACKUP
    inventory_path, completed_path = transaction_root / INVENTORY, transaction_root / COMPLETED
    if not inventory_path.exists() and not inventory_path.is_symlink():
        if completed_path.exists() or completed_path.is_symlink():
            raise RuntimeDependencyError("The retained runtime marker is missing its exact inventory.")
        if backup.exists() or backup.is_symlink():
            _directory(backup, private=True, device=device)
            if any(backup.iterdir()):
                raise RuntimeDependencyError("An unbound retained runtime already exists. Preserve it before recovery.")
        previous_native = previous_proof(destination, destination)
        roots, total, count = {}, 0, 0
        for name in ROOTS:
            _directory(destination / name, device=device)
            roots[name], size = _scan(destination / name, name, device, product_links=product_links)
            total += size
            count += len(roots[name])
        if total > MAX_RUNTIME_BYTES or count > MAX_RUNTIME_ENTRIES:
            raise RuntimeDependencyError("The runtime inventory exceeds its bounded size.")
        if previous_proof(destination, destination) != previous_native:
            raise RuntimeDependencyError("The previous native receipt changed before retention.")
        inventory = {"format": FORMAT, "destination": str(destination), "device": device,
            "roots": roots, "previous_native": previous_native}
        backup.mkdir(mode=0o700, exist_ok=True)
        _fsync_directory(transaction_root)
        _create_json(inventory_path, inventory)
    _reconcile_publication(inventory_path)
    inventory_bytes = _file(inventory_path, MAX_INVENTORY_BYTES, private=True)
    try:
        inventory = json.loads(inventory_bytes, object_pairs_hook=_unique)
    except (ValueError, UnicodeError):
        raise RuntimeDependencyError("The saved runtime receipt or inventory is invalid.") from None
    if (not isinstance(inventory, dict) or set(inventory) != {"format", "destination", "device", "roots", "previous_native"}
            or inventory["format"] != FORMAT or inventory["destination"] != str(destination)
            or inventory["device"] != device or not isinstance(inventory["roots"], dict)
            or set(inventory["roots"]) != set(ROOTS)):
        raise RuntimeDependencyError("The saved runtime inventory belongs to another exact transaction.")
    _directory(backup, private=True, device=device)
    if set(item.name for item in backup.iterdir()) - set(ROOTS):
        raise RuntimeDependencyError("The retained runtime has an unexpected root. Nothing was overwritten.")
    proof = {"format": FORMAT, "destination": str(destination), "backup": str(backup),
        "inventory_sha256": hashlib.sha256(inventory_bytes).hexdigest(), "retained": True,
        "previous_native": inventory["previous_native"]}
    complete = completed_path.exists() or completed_path.is_symlink()
    if complete and _json(completed_path, MAX_RECEIPT_BYTES) != proof:
        raise RuntimeDependencyError("The retained runtime completion marker changed.")
    locations = {}
    for name in ROOTS:
        current, saved = destination / name, backup / name
        saved_exists, current_exists = saved.exists() or saved.is_symlink(), current.exists() or current.is_symlink()
        if complete:
            if not saved_exists:
                raise RuntimeDependencyError("The completed retained runtime is missing original bytes.")
            if current_exists:
                _directory(current, device=device)
            location = saved
        else:
            if saved_exists == current_exists:
                raise RuntimeDependencyError("Runtime retention cannot distinguish the original and candidate roots.")
            location = saved if saved_exists else current
        _directory(location, device=device)
        scanned, _ = _scan(location, name, device, product_links=product_links)
        if scanned != inventory["roots"][name]:
            raise RuntimeDependencyError("The previous runtime inventory changed. Preserve both roots before recovery.")
        locations[name] = location
    native_location = backup if locations[".bin"] == backup / ".bin" else destination
    sdk_location = backup if locations[".venv"] == backup / ".venv" else destination
    if previous_proof(native_location, sdk_location) != inventory["previous_native"]:
        raise RuntimeDependencyError("The retained native receipt no longer matches its original proof.")
    if not complete:
        for name in ROOTS:
            if locations[name] == destination / name:
                require_retention_current()
                _rename_root(destination / name, backup / name)
        for name in ROOTS:
            scanned, _ = _scan(backup / name, name, device, product_links=product_links)
            if scanned != inventory["roots"][name]:
                raise RuntimeDependencyError("The moved runtime no longer matches its retained inventory.")
        require_retention_current()
        _create_json(completed_path, proof)
    require_retention_current()
    return proof
