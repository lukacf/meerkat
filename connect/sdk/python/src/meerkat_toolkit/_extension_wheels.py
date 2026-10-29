"""Private wheel checks extracted from an existing host's installed-artifact owner.

SPDX-License-Identifier: MIT OR Apache-2.0
See LICENSE-EXTRACTED and
EXTENSIONS.md. These checks are for trusted, fixed host-selected files, not
isolation from concurrently executing hostile code under the same OS account.
"""
from __future__ import annotations

import base64
from collections.abc import Mapping, Sequence
import csv
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
from typing import Any
import unicodedata
import zipfile

_INSTALLER_FILES = frozenset({"RECORD", "INSTALLER", "REQUESTED", "direct_url.json", "uv_cache.json"})
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
MAX_FILE_BYTES = 64 * 1024**2
MAX_TOTAL_BYTES = 256 * 1024**2

class ExtensionArtifactError(ValueError):
    """Host-selected software failed validation, before entrypoint import."""

def _reject_json_constant(value: str) -> None:
    raise ExtensionArtifactError(f"non-finite JSON number is forbidden: {value}")

def _load_json(data: bytes, *, label: str) -> Any:
    try:
        return json.loads(data, parse_constant=_reject_json_constant)
    except ExtensionArtifactError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ExtensionArtifactError(f"{label} is not valid UTF-8 JSON") from exc

def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()

def _record_hash(data: bytes) -> str:
    encoded = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=")
    return "sha256=" + encoded.decode("ascii")

def _canonical_name(value: str) -> str:
    return re.sub(r"[-_.]+", "-", value).lower()

def _dist_info_component(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9.]+", "_", value)

def _relative_path(value: object, *, label: str) -> str:
    if (not isinstance(value, str) or not value or "\\" in value
            or any(ord(char) < 32 or ord(char) == 127 for char in value)):
        raise ExtensionArtifactError(f"{label} is not a normalized relative path")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise ExtensionArtifactError(f"{label} escapes its artifact root")
    if path.as_posix() != value:
        raise ExtensionArtifactError(f"{label} is not normalized")
    return value

def _digest(value: object, *, label: str) -> str:
    if not isinstance(value, str) or _SHA256_RE.fullmatch(value) is None:
        raise ExtensionArtifactError(f"{label} is not a lowercase SHA-256 digest")
    return value

def _text(value: object, *, label: str, nonempty: bool = True) -> str:
    if not isinstance(value, str) or (nonempty and not value):
        raise ExtensionArtifactError(f"{label} must be a string")
    return value

def _regular_file(path: Path, *, label: str, max_bytes: int | None = None) -> bytes:
    limit = MAX_FILE_BYTES if max_bytes is None else max_bytes
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ExtensionArtifactError(f"{label} must be a regular non-symlink file")
            data = stream.read(limit + 1)
    except OSError as exc:
        raise ExtensionArtifactError(f"{label} cannot be read") from exc
    if len(data) > limit:
        raise ExtensionArtifactError(f"{label} exceeds its byte limit")
    return data


def _assert_path_chain(root: Path, relative: str) -> None:
    current = root
    for component in PurePosixPath(relative).parts:
        current = current / component
        try:
            mode = current.lstat().st_mode
        except OSError as exc:
            raise ExtensionArtifactError(f"installed path is missing: {relative}") from exc
        if stat.S_ISLNK(mode):
            raise ExtensionArtifactError(f"installed path contains a symlink: {relative}")

def _record_rows(data: bytes, *, label: str) -> dict[str, tuple[str, str]]:
    try:
        text = data.decode("utf-8")
        rows = list(csv.reader(io.StringIO(text, newline="")))
    except (UnicodeDecodeError, csv.Error) as exc:
        raise ExtensionArtifactError(f"{label} is not valid CSV") from exc
    result: dict[str, tuple[str, str]] = {}
    for row in rows:
        if len(row) != 3:
            raise ExtensionArtifactError(f"{label} row does not have three fields")
        name = _relative_path(row[0], label=f"{label} path")
        if name in result:
            raise ExtensionArtifactError(f"{label} contains a duplicate path")
        result[name] = (row[1], row[2])
    return result

def _verify_record(
    *,
    data: bytes,
    record_path: str,
    payloads: Mapping[str, bytes],
    label: str,
) -> None:
    rows = _record_rows(data, label=label)
    if set(rows) != set(payloads):
        raise ExtensionArtifactError(f"{label} names do not match the payload")
    for name, content in payloads.items():
        hash_field, size_field = rows[name]
        if name == record_path:
            if hash_field or size_field:
                raise ExtensionArtifactError(f"{label} own row must have empty hash and size")
        elif hash_field != _record_hash(content) or size_field != str(len(content)):
            raise ExtensionArtifactError(f"{label} hash or size differs for {name}")

def _generated_bytecode_source(
    path: PurePosixPath,
    *,
    manifest: Mapping[str, str],
    package_roots: Sequence[str],
) -> PurePosixPath | None:
    """Resolve one current-interpreter cache path to a manifest-owned source."""

    if path.suffix != ".pyc" or path.parent.name != "__pycache__":
        return None
    cache_tag = sys.implementation.cache_tag
    if not cache_tag:
        return None
    marker = f".{cache_tag}"
    stem, separator, suffix = path.name.partition(marker)
    if not separator or not stem:
        return None
    if suffix != ".pyc" and re.fullmatch(r"\.opt-[1-9][0-9]*\.pyc", suffix) is None:
        return None

    source = path.parent.parent / f"{stem}.py"
    source_name = source.as_posix()
    if source_name not in manifest:
        return None
    if len(source.parts) == 1:
        return source
    if source.parts[0] not in package_roots:
        return None
    return source

def _zip_mode(info: zipfile.ZipInfo) -> int:
    return (info.external_attr >> 16) & 0xFFFF

def verify_installed(
    site_packages: Path,
    *,
    distribution: str,
    version: str,
    installed_files: Sequence[Mapping[str, str]],
) -> tuple[str, ...]:
    distribution = _text(distribution, label="distribution")
    version = _text(version, label="version")
    expected_dist_info = (
        f"{_dist_info_component(distribution)}-{_dist_info_component(version)}.dist-info"
    )
    dist_info_relative = _relative_path(expected_dist_info, label="dist-info directory")
    dist_info = site_packages / dist_info_relative
    try:
        mode = dist_info.lstat().st_mode
    except OSError as exc:
        raise ExtensionArtifactError("expected dist-info directory is missing") from exc
    if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
        raise ExtensionArtifactError("expected dist-info must be a non-symlink directory")

    if isinstance(installed_files, (str, bytes)) or not isinstance(installed_files, Sequence):
        raise ExtensionArtifactError("installed_files must be a list")
    manifest: dict[str, str] = {}
    for row in installed_files:
        if not isinstance(row, Mapping) or set(row) != {"file", "sha256"}:
            raise ExtensionArtifactError("installed_files row has an invalid shape")
        relative = _relative_path(row.get("file"), label="installed_files.file")
        digest = _digest(row.get("sha256"), label=f"installed digest for {relative}")
        if relative in manifest:
            raise ExtensionArtifactError("installed_files contains a duplicate path")
        manifest[relative] = digest
    if not manifest:
        raise ExtensionArtifactError("installed_files must not be empty")

    installer_paths = {f"{dist_info_relative}/{name}": name for name in _INSTALLER_FILES}
    installer_paths = {
        path: name for path, name in installer_paths.items()
        if name in {"RECORD", "INSTALLER"}
        or (site_packages / path).exists() or (site_packages / path).is_symlink()
    }
    if set(manifest) & set(installer_paths):
        raise ExtensionArtifactError("immutable manifest includes installer-owned files")

    payloads: dict[str, bytes] = {}
    for relative, expected_digest in manifest.items():
        _assert_path_chain(site_packages, relative)
        data = _regular_file(
            site_packages / relative,
            label=f"immutable installed file {relative}",
        )
        if _sha256(data) != expected_digest:
            raise ExtensionArtifactError(f"installed payload digest differs for {relative}")
        payloads[relative] = data

    installer_payloads: dict[str, bytes] = {}
    for relative, name in installer_paths.items():
        _assert_path_chain(site_packages, relative)
        installer_payloads[relative] = _regular_file(
            site_packages / relative,
            label=f"installer-owned file {name}",
            max_bytes=64 * 1024,
        )
    if installer_payloads[f"{dist_info_relative}/INSTALLER"].strip() not in {b"uv", b"pip"}:
        raise ExtensionArtifactError("unsupported installed distribution owner")
    if installer_payloads.get(f"{dist_info_relative}/REQUESTED", b"") != b"":
        raise ExtensionArtifactError("REQUESTED must be empty")
    for name in ("direct_url.json", "uv_cache.json"):
        if f"{dist_info_relative}/{name}" in installer_payloads:
            _load_json(installer_payloads[f"{dist_info_relative}/{name}"], label=name)

    roots = {PurePosixPath(name).parts[0] for name in manifest}
    if dist_info_relative not in roots:
        raise ExtensionArtifactError(
            "immutable manifest does not own the expected dist-info"
        )
    actual: set[str] = set()
    generated_bytecode: list[Path] = []
    for root_name in roots:
        root = site_packages / root_name
        try:
            root_mode = root.lstat().st_mode
        except OSError as exc:
            raise ExtensionArtifactError(f"owned root is missing: {root_name}") from exc
        if stat.S_ISLNK(root_mode):
            raise ExtensionArtifactError(f"owned root is a symlink: {root_name}")
        if stat.S_ISREG(root_mode):
            actual.add(root_name)
            continue
        if not stat.S_ISDIR(root_mode):
            raise ExtensionArtifactError(f"owned root has an invalid file type: {root_name}")
        for directory, dirnames, filenames in os.walk(root, followlinks=False):
            directory_path = Path(directory)
            for dirname in dirnames:
                if (directory_path / dirname).is_symlink():
                    raise ExtensionArtifactError(
                        "owned roots may not contain symlink directories"
                    )
            for filename in filenames:
                path = directory_path / filename
                mode = path.lstat().st_mode
                if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
                    raise ExtensionArtifactError(
                        "owned roots may contain only regular files"
                    )
                actual.add(path.relative_to(site_packages).as_posix())

    package_roots = tuple(sorted(roots - {dist_info_relative}))
    expected = set(manifest) | set(installer_paths)
    for relative in actual - expected:
        path = PurePosixPath(relative)
        if path.suffix != ".pyc":
            raise ExtensionArtifactError(f"unexpected file in owned roots: {relative}")
        if (
            _generated_bytecode_source(
                path,
                manifest=manifest,
                package_roots=package_roots,
            )
            is None
        ):
            raise ExtensionArtifactError(f"foreign bytecode in owned roots: {relative}")
        generated_bytecode.append(site_packages / relative)
    if not expected <= actual:
        raise ExtensionArtifactError("installed owned roots are missing required files")

    # A top-level module writes its cache under site-packages/__pycache__,
    # outside the module file's owned root. Inspect only cache names that
    # can belong to a manifest-owned top-level source; other distributions'
    # caches are outside this verifier's authority.
    top_level_sources = tuple(
        PurePosixPath(relative)
        for relative in manifest
        if len(PurePosixPath(relative).parts) == 1 and PurePosixPath(relative).suffix == ".py"
    )
    cache_root = site_packages / "__pycache__"
    if top_level_sources and cache_root.exists():
        if cache_root.is_symlink() or not cache_root.is_dir():
            raise ExtensionArtifactError("top-level bytecode cache root is invalid")
        cache_files = tuple(cache_root.iterdir())
        for source in top_level_sources:
            prefix = f"{source.stem}."
            for candidate in cache_files:
                if not candidate.name.startswith(prefix) or not candidate.name.endswith(".pyc"):
                    continue
                if candidate.is_symlink() or not candidate.is_file():
                    raise ExtensionArtifactError(
                        f"invalid bytecode cache file: {candidate.name}"
                    )
                cache_relative = PurePosixPath("__pycache__", candidate.name)
                if (
                    not candidate.name.startswith(prefix)
                    or _generated_bytecode_source(
                        cache_relative,
                        manifest=manifest,
                        package_roots=package_roots,
                    )
                    != source
                ):
                    raise ExtensionArtifactError(
                        f"foreign bytecode for installed module: {candidate.name}"
                    )
                generated_bytecode.append(candidate)

    record_path = f"{dist_info_relative}/RECORD"
    all_record_payloads = {**payloads, **installer_payloads}
    # pip records its generated bytecode with empty hash/size. Such rows
    # describe caches rather than artifact payload, and remain after we remove
    # the caches below. Accept only paths derived from a verified source in
    # this distribution, never arbitrary unrecorded or hashed payload.
    rows = _record_rows(installer_payloads[record_path], label="installed RECORD")
    for relative in set(rows) - set(all_record_payloads):
        if (rows[relative] != ("", "") or _generated_bytecode_source(
                PurePosixPath(relative), manifest=manifest, package_roots=package_roots) is None):
            raise ExtensionArtifactError("installed RECORD contains a foreign cache or payload")
        del rows[relative]
    normalized_record = io.StringIO(newline="")
    writer = csv.writer(normalized_record)
    writer.writerows((name, *fields) for name, fields in rows.items())
    _verify_record(
        data=normalized_record.getvalue().encode("utf-8"),
        record_path=record_path,
        payloads=all_record_payloads,
        label="installed RECORD",
    )
    # Timestamp-based pyc files are executable and can remain valid after
    # verified source bytes are restored. They are therefore cache only,
    # never certification evidence: remove every valid manifest-derived
    # cache after all checks and before extension import. Python may safely
    # regenerate it from the verified source; an ordinary later boot will
    # repeat this same bounded cleanup.
    for path in generated_bytecode:
        try:
            path.unlink()
        except OSError as exc:
            raise ExtensionArtifactError(
                f"cannot remove generated bytecode before import: {path}"
            ) from exc
    return package_roots

def read_wheel_payload(data: bytes) -> dict[str, bytes]:
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
    except (OSError, zipfile.BadZipFile) as exc:
        raise ExtensionArtifactError("wheel is not a valid zip archive") from exc
    with archive:
        if len(archive.infolist()) > 20000 or sum(i.file_size for i in archive.infolist()) > MAX_TOTAL_BYTES:
            raise ExtensionArtifactError("wheel expansion exceeds its limit")
        files: dict[str, bytes] = {}
        seen_names: set[str] = set()
        normalized_names: dict[str, str] = {}
        for info in archive.infolist():
            if info.file_size > MAX_FILE_BYTES:
                raise ExtensionArtifactError("wheel member exceeds its limit")
            raw_name = info.filename
            trimmed = raw_name[:-1] if raw_name.endswith("/") else raw_name
            name = _relative_path(trimmed, label="wheel member")
            if raw_name in seen_names:
                raise ExtensionArtifactError("wheel contains a duplicate member")
            seen_names.add(raw_name)
            collision_key = unicodedata.normalize("NFC", name).casefold()
            previous = normalized_names.get(collision_key)
            if previous is not None and previous != name:
                raise ExtensionArtifactError("wheel contains NFC/case-fold-colliding paths")
            normalized_names[collision_key] = name
            mode = _zip_mode(info)
            if stat.S_ISLNK(mode):
                raise ExtensionArtifactError("wheel contains a symlink")
            # ZIP writers commonly store only POSIX permission bits for
            # ordinary files (for example Hatchling emits 0o644 for dist-info
            # members).  A missing file-type field is regular-by-container;
            # explicit symlink and other special Unix file types stay closed.
            file_type = stat.S_IFMT(mode)
            if file_type not in {0, stat.S_IFREG, stat.S_IFDIR}:
                raise ExtensionArtifactError("wheel contains a non-regular member")
            if info.flag_bits & 0x1:
                raise ExtensionArtifactError("wheel contains an encrypted member")
            if info.is_dir():
                continue
            folded_parts = tuple(part.casefold() for part in PurePosixPath(name).parts)
            leaf = folded_parts[-1]
            if leaf.endswith(".pth"):
                raise ExtensionArtifactError("wheel contains a .pth file")
            if any(part.endswith(".data") for part in folded_parts):
                raise ExtensionArtifactError("wheel contains a .data payload tree")
            if any(
                part in {"sitecustomize", "usercustomize", "sitecustomize.py", "usercustomize.py"}
                for part in folded_parts
            ):
                raise ExtensionArtifactError("wheel contains a startup customization module")
            try:
                files[name] = archive.read(info)
            except (OSError, RuntimeError, zipfile.BadZipFile) as exc:
                raise ExtensionArtifactError(f"wheel member cannot be read: {name}") from exc

    return files
