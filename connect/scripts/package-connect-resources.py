#!/usr/bin/env python3
"""Build a selected Connect resource closure from pinned, already acquired inputs."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import runpy
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unicodedata

ROOT = Path(__file__).resolve().parents[1]
PREPARE = ROOT / "sdk/consumer-setup/product/prepare.py"
# Exact previously acquired pip 26.2.1, shared by both CPython 3.11 targets.
# Packaging consumes local pinned bytes; it never fetches or executes pip.
PIP = ("pip.pyz", "91d5fd9f6f25549fd839c60536c6f1b945316ce3588d34a605635b6071c91526", 1759056)
PIP_LICENSE = "634300a669d49aeae65b12c6c48c924c51a4cdf3d1ff086dc3456dc8bcaa2104"
TARGETS = {
    "macos-arm64": {
        "python": ("cpython-3.11.16+20260901-aarch64-apple-darwin-pgo+lto-full.tar.zst",
                   "19b8d2af1719e3fe8cd4edfa2e5030dab776233c05044f4348dbe3c0af7d1b2f", 41154354),
        "node": ("node-v24.19.0-darwin-arm64.tar.gz",
                 "8294b7aa9b03997481c06babf1e8b270c859358f27da57a11509afe537ac381d", 52234372),
        "library": ("toolkit-library-20261009-library-support-selection-macos-arm64.tar.gz",
                    "49482af8016a76de90826a1362030319f27063b92f9ee0c87de970801f04cb6b", 128566016),
        "library_dir": "toolkit-library-20261009-library-support-selection-macos-arm64",
        "library_url": "https://example.invalid/connect-resources/20261010/setup-descriptor-48256eb8/toolkit-library-20261009-library-support-selection-macos-arm64.tar.gz?generation=1791603887971933",
        "starter": "toolkit-setup-20261010-resource-selection-export",
    },
    "linux-x86_64": {
        "python": ("cpython-3.11.16+20260901-x86_64-unknown-linux-gnu-pgo+lto-full.tar.zst",
                   "b48f6e7b70f366018b586a6d6234c551d05b172a3e6c79ddaf33fb90e7038acb", 74939721),
        "node": ("node-v22.23.2-linux-x64.tar.gz",
                 "b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a", 56851233),
        # The library Connect 0.9.11 ships, with the starter exported for it.
        # Its release built it from this repository, so it has no hosted URL.
        "library": ("toolkit-library-20261109-connect-ad31c1fd-linux-x86_64.tar.gz",
                    "de622a11f8789592fec5f5c5959be7692a57b8d7eb688a9c9e3ecc63516d3bd4", 120640110),
        "library_dir": "toolkit-library-20261109-connect-ad31c1fd-linux-x86_64",
        "starter": "toolkit-setup-20261109-connect-ad31c1fd-export",
    },
}
# This declares the consuming host, not the machine performing packaging.
# The installer still probes the real host. Linux equality is this frozen
# library's contract, not the upstream runtimes' minimum glibc requirement.
SUPPORTED_HOSTS = {
    "macos-arm64": {"platform": "darwin", "machine": "arm64", "pointer_bits": 64},
    "linux-x86_64": {"platform": "linux", "machine": "x86_64", "pointer_bits": 64,
                     "glibc": "glibc 2.39"},
}
# The six files of every starter export.
STARTER_FILES = ("README.md", "bootstrap_host.py", "current-distribution.json",
                 "prepare_host.py", "release-manifest.json", "start.py")
# The exact files of each target's default starter export (its "starter",
# under dist/). Its release manifest names that target's default library.
STARTERS = {
    "toolkit-setup-20261010-resource-selection-export": {
        "README.md": "45ad8666ef32a3db9d5186a4848332adb4726381b76353dd978d2dacadc4a5de",
        "bootstrap_host.py": "06249f980076e1648109c7f6cf5e9ed647440a67d63ed59ea950c5874ca0cb56",
        "current-distribution.json": "3dcc5271f0ddc16df5e96e6a32540871a953bdb842ff1dae7f1fcb1b1d7203e7",
        "prepare_host.py": "a31de9e408930c0e05ec8ec0994a0a879d546bdbdd16703439502295377895fa",
        "release-manifest.json": "e9950a90f0ec4fc65e707065ad6c08964851506629053c8a9be31cdfadfc5589",
        "start.py": "4b504d98f51621d0d08bc1813a0aca4151747f22fa99bd29b4db50311310107c",
    },
    "toolkit-setup-20261109-connect-ad31c1fd-export": {
        "README.md": "da7c0f800676143e5de573142578f7841f79b511ab930105442a1dae04972a9f",
        "bootstrap_host.py": "f669afb76366c09c60ae1378bb529f8c2a891174344d7cbaf71ca2cd43c1f7ee",
        "current-distribution.json": "63f1625ee96431453cd7d2ee77488956e6e148c1927018a0a616fec5a773740f",
        "prepare_host.py": "24e8a8e3d1537983f9cd8bcf7d26ebeb773224ffa05bacf7dcb20be5aeea178e",
        "release-manifest.json": "b5b58a8d48509133c82be415956381162f0c440cd7783a5ad20b2d5e6b5b3498",
        "start.py": "9aecf1729270cb4cd2d568f44c4fb4141e447e6c1f1aee713a896fff9842264a",
    },
}
MAX_RUNTIME_ARCHIVE_ENTRIES = 50000
MAX_RUNTIME_ARCHIVE_BYTES = 1024 * 1024 * 1024


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def pin(path, expected, size=None):
    if path.is_symlink() or not path.is_file() or digest(path) != expected or (size and path.stat().st_size != size):
        raise ValueError("packaging input does not match its pin")


@contextmanager
def captured_archive(path, expected, size):
    """Yield a private archive verified before any decompression or inspection."""
    if type(size) is not int or not 0 < size <= 512 * 1024 * 1024:
        raise ValueError("invalid archive capture bound")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size != size:
            raise ValueError("archive capture size or type mismatch")
        with tempfile.TemporaryDirectory(prefix="connect-resource-input-") as scratch:
            captured = Path(scratch) / path.name
            total, observed = 0, hashlib.sha256()
            with captured.open("xb") as output:
                while block := source.read(1024 * 1024):
                    total += len(block)
                    if total > size:
                        raise ValueError("archive capture size mismatch")
                    observed.update(block)
                    output.write(block)
            if total != size or observed.hexdigest() != expected:
                raise ValueError("archive capture pin mismatch")
            captured.chmod(0o400)
            yield captured


def captured_input(path, expected, size):
    """Bind a maintainer's explicit pin to the bytes later extracted or copied."""
    if (not isinstance(path, Path) or not path.is_absolute()
            or not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected)
            or type(size) is not int or not 0 < size <= 128 * 1024 * 1024):
        raise ValueError("invalid explicit npm resource pin")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size != size:
            raise ValueError("explicit npm resource type or size mismatch")
        raw = source.read(size + 1)
    if len(raw) != size or hashlib.sha256(raw).hexdigest() != expected:
        raise ValueError("explicit npm resource pin mismatch")
    return raw


def captured_node_build_profile(path, expected, *, selected_node):
    """Capture only the bounded profile; do not relocate or execute its tools."""
    if path is None and expected is None:
        return None
    if (not selected_node or not isinstance(path, Path) or not path.is_absolute()
            or not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected)):
        raise ValueError("Node build profile requires an exact pair and selected npm resources")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= 65536:
            raise ValueError("invalid Node build profile input")
        raw = source.read(65537)
    if len(raw) != info.st_size or hashlib.sha256(raw).hexdigest() != expected:
        raise ValueError("Node build profile pin mismatch")
    return raw


def selected_npm_inputs(target, node_archive, node_sha256, node_bytes, npm_archive, npm_sha256, npm_bytes):
    fields = (node_archive, node_sha256, node_bytes, npm_archive, npm_sha256, npm_bytes)
    if not any(value is not None for value in fields):
        return None
    if not all(value is not None for value in fields):
        raise ValueError("complete explicit npm resource group required")
    platform = {"macos-arm64": "darwin-arm64", "linux-x86_64": "linux-x64"}[target]
    if (not isinstance(node_archive, Path)
            or not re.fullmatch(r"node-v[0-9]+\.[0-9]+\.[0-9]+-" + platform + r"\.tar\.gz", node_archive.name)):
        raise ValueError("selected acquisition Node archive target mismatch")
    node_raw = captured_input(node_archive, node_sha256, node_bytes)
    npm_raw = captured_input(npm_archive, npm_sha256, npm_bytes)
    owner = runpy.run_path(str(PREPARE.parent.parent / "node_acquisition.py"))
    owner["tool_payloads"](npm_raw)
    return {"node_raw": node_raw, "npm_raw": npm_raw, "owner": owner,
            "evidence": {"node_archive": {"path": str(node_archive), "sha256": node_sha256, "bytes": node_bytes},
                         "npm_archive": {"path": str(npm_archive), "sha256": npm_sha256, "bytes": npm_bytes}}}


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate selected metadata key")
        result[key] = value
    return result


def selected_resource_inputs(target, library, library_sha256, library_bytes, starter,
                             release_manifest_sha256):
    """Select an already exported starter and its exact library, without running either."""
    fields = (library, library_sha256, library_bytes, starter, release_manifest_sha256)
    if not all(value is not None for value in fields):
        raise ValueError("complete explicit library and starter selection required")
    if (not library.is_absolute() or not starter.is_absolute()
            or library.is_symlink() or starter.is_symlink()
            or not isinstance(library_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", library_sha256)
            or not isinstance(release_manifest_sha256, str)
            or not re.fullmatch(r"[0-9a-f]{64}", release_manifest_sha256)
            or type(library_bytes) is not int or not 0 < library_bytes <= 512 * 1024 * 1024):
        raise ValueError("invalid explicit resource selection")
    library, starter = library.resolve(strict=True), starter.resolve(strict=True)
    if (not re.fullmatch(r"toolkit-library-[A-Za-z0-9_.-]+-" + re.escape(target) + r"\.tar\.gz", library.name)
            or not starter.is_dir() or {p.name for p in starter.iterdir()} != set(STARTER_FILES)):
        raise ValueError("selected library target or starter closure mismatch")
    pin(library, library_sha256, library_bytes)
    files = {}
    for name in STARTER_FILES:
        path = starter / name
        if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 2 * 1024 * 1024:
            raise ValueError("invalid selected starter file")
        files[name] = {"sha256": digest(path), "bytes": path.stat().st_size}
    metadata = {}
    for name in ("current-distribution.json", "release-manifest.json"):
        # Parse the exact bounded bytes whose hashes were selected, never a
        # second pathname read that could differ from the copied payload pin.
        fd = os.open(starter / name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= 2 * 1024 * 1024:
                raise ValueError("invalid selected starter metadata")
            raw = stream.read(2 * 1024 * 1024 + 1)
        if {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)} != files[name]:
            raise ValueError("selected starter metadata changed")
        metadata[name] = raw
    if files["release-manifest.json"]["sha256"] != release_manifest_sha256:
        raise ValueError("selected release manifest pin mismatch")
    # Executable setup source is selected from this checkout, not an arbitrary
    # script with a newly supplied hash. The exported README is retained data.
    for name in ("start.py", "bootstrap_host.py", "prepare_host.py"):
        source = ROOT / "sdk/consumer-setup" / name
        pin(source, files[name]["sha256"], files[name]["bytes"])
    pointer = json.loads(metadata["current-distribution.json"], object_pairs_hook=unique_object)
    if pointer != {"schema_version": 1, "manifest": "release-manifest.json",
                   "manifest_sha256": release_manifest_sha256} or type(pointer.get("schema_version")) is not int:
        raise ValueError("selected starter pointer mismatch")
    manifest = json.loads(metadata["release-manifest.json"], object_pairs_hook=unique_object)
    if (not isinstance(manifest, dict) or type(manifest.get("schema_version")) is not int
            or manifest["schema_version"] != 1 or not isinstance(manifest.get("assets"), list)
            or not 0 < len(manifest["assets"]) <= 16
            or any(not isinstance(row, dict) or not isinstance(row.get("upload_name"), str)
                   for row in manifest["assets"])):
        raise ValueError("invalid selected release manifest")
    rows = [row for row in manifest["assets"] if row["upload_name"].endswith("-" + target + ".tar.gz")]
    if (len(rows) != 1 or rows[0]["upload_name"] != library.name
            or rows[0].get("sha256") != library_sha256
            or type(rows[0].get("bytes")) is not int or rows[0]["bytes"] != library_bytes):
        raise ValueError("selected release asset mismatch")
    evidence = {"kind": "explicit_pinned_connect_resource_inputs",
                "library": {"path": str(library), "sha256": library_sha256, "bytes": library_bytes},
                "starter": {"path": str(starter), "manifest_sha256": release_manifest_sha256, "files": files}}
    return library, (library.name, library_sha256, library_bytes), starter, files, evidence


def require_case_sensitive(directory):
    """Linux runtime paths require a staging filesystem that preserves case."""
    with tempfile.TemporaryDirectory(prefix=".connect-case-check-", dir=directory) as scratch:
        lower = Path(scratch) / "case-probe"
        upper = Path(scratch) / "CASE-PROBE"
        lower.write_bytes(b"lower")
        if upper.exists():
            raise ValueError("Linux resources require a case-sensitive staging filesystem")
        with upper.open("xb") as output:
            output.write(b"upper")
        if lower.read_bytes() != b"lower" or upper.read_bytes() != b"upper":
            raise ValueError("Linux resources require a case-sensitive staging filesystem")


def extract(archive, root, mapping, *, target="macos-arm64"):
    """Extract only mapped runtime/notices with bounded, confined links."""
    def path_key(value):
        value = unicodedata.normalize("NFC", value)
        return value if target == "linux-x86_64" else value.casefold()

    if root.is_symlink() or not root.is_dir():
        raise ValueError("resource extraction requires an existing real directory")
    root = root.resolve()
    rows, seen, total = [], set(), 0
    with tarfile.open(archive) as source:
        for member in source:
            name = member.name.rstrip("/")
            if not name or PurePosixPath(name).is_absolute() or ".." in name.split("/"):
                raise ValueError("invalid upstream archive path")
            rel = mapping(name)
            if rel is None:
                continue
            if (not isinstance(rel, str) or not rel or "\x00" in rel or "\\" in rel
                    or PurePosixPath(rel).is_absolute() or ".." in rel.split("/")):
                raise ValueError("invalid mapped archive path")
            # Normalize filesystem aliases before checking duplicate paths and
            # parent ownership. Keep original case and Unicode spelling on disk.
            rel = PurePosixPath(rel).as_posix()
            if rel == ".":
                raise ValueError("invalid mapped archive path")
            key = path_key(rel)
            if (key in seen or len(rows) >= MAX_RUNTIME_ARCHIVE_ENTRIES
                    or member.size < 0 or total + member.size > MAX_RUNTIME_ARCHIVE_BYTES):
                raise ValueError("upstream path collision or size limit")
            seen.add(key)
            if not (member.isfile() or member.isdir() or member.issym()):
                raise ValueError("unsupported upstream archive entry")
            total += member.size
            rows.append((member, rel))
        declared = {path_key(rel): member for member, rel in rows}
        for member, rel in rows:
            path = root / rel
            for parent in PurePosixPath(rel).parents:
                owner = declared.get(path_key(parent.as_posix()))
                if owner is not None and not owner.isdir():
                    raise ValueError("archive descends through a non-directory")
                existing = root / parent
                if existing.is_symlink() or (existing.exists() and not existing.is_dir()):
                    raise ValueError("archive descends through a non-directory")
            if path.is_symlink() or (path.exists() and not (member.isdir() and path.is_dir())):
                raise ValueError("upstream path collides with existing resources")
            if member.issym():
                if (not member.linkname or "\x00" in member.linkname or "\\" in member.linkname
                        or PurePosixPath(member.linkname).is_absolute()):
                    raise ValueError("upstream link escapes packaged resources")

        # Resolve the complete planned link graph before creating any entry.
        # Path.resolve() alone cannot see links that have not been created yet.
        for member, rel in rows:
            if not member.issym():
                continue
            parts = list(PurePosixPath(rel).parent.parts)
            pending = list(reversed(member.linkname.split("/")))
            traversed = 0
            while pending:
                part = pending.pop()
                if part in ("", "."):
                    continue
                if part == "..":
                    if not parts:
                        raise ValueError("upstream link escapes packaged resources")
                    parts.pop()
                    continue
                parts.append(part)
                planned = declared.get(path_key("/".join(parts)))
                existing = root.joinpath(*parts)
                link = (planned.linkname if planned is not None and planned.issym()
                        else os.readlink(existing) if existing.is_symlink() else None)
                if link is not None:
                    traversed += 1
                    if traversed > 40:
                        raise ValueError("upstream link cycle or depth limit")
                    if PurePosixPath(link).is_absolute():
                        raise ValueError("upstream link escapes packaged resources")
                    parts.pop()
                    pending.extend(reversed(link.split("/")))
        for member, rel in rows:
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            if member.isdir():
                path.mkdir(exist_ok=True)
            elif member.isfile():
                with path.open("xb") as output, source.extractfile(member) as payload:
                    shutil.copyfileobj(payload, output)
                path.chmod(0o755 if member.mode & 0o111 else 0o644)
        for member, rel in rows:
            if member.issym():
                path = root / rel
                path.symlink_to(member.linkname)
                if target == "linux-x86_64":
                    # Linux links always report 0777. Set the actual staging
                    # link mode on hosts such as macOS before pinning it.
                    if stat.S_IMODE(path.lstat().st_mode) != 0o777:
                        os.chmod(path, 0o777, follow_symlinks=False)
                    if stat.S_IMODE(path.lstat().st_mode) != 0o777:
                        raise ValueError("target symlink mode was not applied")


def python_mapping(name):
    if name.startswith("python/install/"):
        return "runtime/python/" + name.removeprefix("python/install/")
    if name == "python/PYTHON.json" or name.startswith("python/licenses/"):
        return "notices/python/" + name.removeprefix("python/")
    return None


def metadata_bytes(path, limit):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > limit:
        raise ValueError("runtime metadata must be a bounded regular file")
    return path.read_bytes()


def binary_target(path, target):
    """Check only the two supported executable headers; never run target code."""
    if path.is_symlink() or not path.is_file():
        raise ValueError("runtime executable must be a regular file")
    with path.open("rb") as stream:
        header = stream.read(64)
    if target == "macos-arm64":
        matches = (len(header) == 64 and header[:8] == bytes.fromhex("cffaedfe0c000001")
                   and int.from_bytes(header[12:16], "little") == 2)
    elif target == "linux-x86_64":
        matches = (len(header) == 64 and header[:7] == b"\x7fELF\x02\x01\x01"
                   and int.from_bytes(header[16:18], "little") in (2, 3)
                   and int.from_bytes(header[18:20], "little") == 62)
    else:
        matches = False
    if not matches:
        raise ValueError("runtime executable architecture differs from packaging target")


def runtime_metadata(resources, target, decode):
    """Facts from the already hash-checked archives, without target execution."""
    metadata = decode(metadata_bytes(resources / "notices/python/PYTHON.json", 4 * 1024 * 1024))
    if not isinstance(metadata, dict):
        raise ValueError("unsupported Python archive metadata")
    platform, machine, triple, node_arch = {
        "macos-arm64": ("darwin", "arm64", "aarch64-apple-darwin", "arm64"),
        "linux-x86_64": ("linux", "x86_64", "x86_64-unknown-linux-gnu", "x64"),
    }[target]
    config = metadata.get("python_config_vars", {})
    version = metadata.get("python_version")
    if (metadata.get("version") != "8" or metadata.get("target_triple") != triple
            or metadata.get("python_implementation_name") != "cpython"
            or metadata.get("python_major_minor_version") != "3.11"
            or metadata.get("python_exe") != "install/bin/python3.11"
            or not isinstance(version, str) or not re.fullmatch(r"3\.11\.[0-9]+", version)
            or not isinstance(config, dict) or config.get("SIZEOF_VOID_P") != "8"
            or config.get("HOST_GNU_TYPE") != triple or config.get("MACHDEP") != platform):
        raise ValueError("unsupported or inconsistent Python archive metadata")
    binary_target(resources / "runtime/python/bin/python3.11", target)
    return {
        "python": {"implementation": "cpython", "version": version, "platform": platform,
                   "machine": machine, "pointer_bits": 64},
        "node": node_metadata(resources / "runtime/node", target, decode),
    }


def node_metadata(node_root, target, decode):
    platform, node_arch = {"macos-arm64": ("darwin", "arm64"),
                           "linux-x86_64": ("linux", "x64")}[target]
    binary_target(node_root / "bin/node", target)
    header = metadata_bytes(node_root / "include/node/node_version.h", 64 * 1024).decode("ascii")
    values = []
    for name in ("MAJOR_VERSION", "MINOR_VERSION", "PATCH_VERSION", "VERSION_IS_RELEASE"):
        found = re.findall(r"^#define NODE_" + name + r" ([0-9]+)$", header, re.MULTILINE)
        if len(found) != 1:
            raise ValueError("missing or ambiguous Node archive version metadata")
        values.append(found[0])
    if values[3] != "1":
        raise ValueError("Node archive is not a release")
    npm = decode(metadata_bytes(node_root / "lib/node_modules/npm/package.json", 1024 * 1024))
    if (not isinstance(npm, dict) or npm.get("name") != "npm" or not isinstance(npm.get("version"), str)
            or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", npm["version"])):
        raise ValueError("unsupported npm archive metadata")
    return {"version": "v" + ".".join(values[:3]), "platform": platform,
            "arch": node_arch, "npm_version": npm["version"]}


def runtime_compatibility(archive, resources, target, supported_host, decode, *, fileobj=None):
    """Honor the selected library's existing schema, including its Mac fallback."""
    member_name = archive.name.removesuffix(".tar.gz") + "/consumers/CONTENTS.json"
    with tarfile.open(archive if fileobj is None else None, fileobj=fileobj) as source:
        members = [member for member in source if member.name == member_name]
        if len(members) != 1 or not members[0].isfile() or members[0].size > 1024 * 1024:
            raise ValueError("selected library needs one bounded regular CONTENTS.json")
        contents_raw = source.extractfile(members[0]).read()
    contents = decode(contents_raw)
    if not isinstance(contents, dict) or type(contents.get("schema_version")) is not int:
        raise ValueError("unsupported selected library runtime contract")
    observed = runtime_metadata(resources, target, decode)
    python = observed["python"]
    selected_python = python
    host_shape = {key: python[key] for key in ("platform", "machine", "pointer_bits")}
    if target == "linux-x86_64":
        if (set(supported_host) != set(host_shape) | {"glibc"}
                or not isinstance(supported_host.get("glibc"), str)
                or not re.fullmatch(r"glibc [0-9]+\.[0-9]+", supported_host["glibc"])):
            raise ValueError("complete declared Linux host profile required")
        host_shape["glibc"] = supported_host["glibc"]
        selected_python = {**python, "glibc": supported_host["glibc"]}
    if supported_host != host_shape:
        raise ValueError("declared host profile differs from runtime architecture")
    if contents["schema_version"] == 1:
        # Exact legacy predicates in this hash-pinned library's installer.
        # It checks Python's major/minor and these Node versions, not npm.
        if (target != "macos-arm64" or "runtime_target" in contents
                or observed["node"]["version"] not in ("v24.19.0", "v22.19.0")):
            raise ValueError("unsupported legacy Mac library runtime contract")
    elif contents["schema_version"] == 2 and target == "linux-x86_64":
        expected = contents.get("runtime_target")
        if not isinstance(expected, dict) or set(expected) != {"python", "node"}:
            raise ValueError("complete exact library runtime_target required")
        if (not isinstance(expected["python"], dict)
                or type(expected["python"].get("pointer_bits")) is not int
                or expected["python"] != selected_python):
            raise ValueError("Python metadata or declared host profile differs from library runtime_target")
        if expected["node"] != observed["node"]:
            raise ValueError("Node/npm metadata differs from library runtime_target")
    else:
        raise ValueError("unsupported selected library runtime contract")
    return {"library_contents_sha256": hashlib.sha256(contents_raw).hexdigest(),
            "library_schema_version": contents["schema_version"], "supported_host": supported_host,
            "runtime_metadata": observed,
            "scope": "hash_bound_archive_metadata_and_declared_host_only_not_runtime_qualification"}


def materialize_library(archive, resources, target):
    """Package inert archive bytes; only the checkout's validator is loaded."""
    name = archive.name.removesuffix(".tar.gz")
    extract(archive, resources, lambda path: "library/" + path, target=target)
    release = resources / "library" / name
    owner = runpy.run_path(str(ROOT / "sdk/consumer-setup/prepare_host.py"))
    owner["release_snapshot"](str(archive), digest(archive), release, target)
    return "library/" + name


def build(destination, upstream, zstd, target="macos-arm64", *, library=None,
          library_sha256=None, library_bytes=None, starter=None, release_manifest_sha256=None,
          acquisition_node_archive=None, acquisition_node_sha256=None, acquisition_node_bytes=None,
          npm_archive=None, npm_archive_sha256=None, npm_archive_bytes=None,
          node_prebuilt_policy=None, node_prebuilt_policy_sha256=None,
          node_build_profile=None, node_build_profile_sha256=None, families=None,
          immutable_library_release=False):
    if target not in TARGETS:
        raise ValueError("unsupported packaging target")
    fields = (library, library_sha256, library_bytes, starter, release_manifest_sha256)
    explicit = any(value is not None for value in fields)
    if explicit and not all(value is not None for value in fields):
        raise ValueError("complete explicit library and starter selection required")
    if type(immutable_library_release) is not bool or immutable_library_release and not explicit:
        raise ValueError("immutable library requires explicit current starter and library selection")
    if families is not None:
        if not isinstance(families, str):
            raise ValueError("invalid library families selection")
        if not explicit:
            raise ValueError("family selection requires explicit current starter and library selection")
    acquisition = selected_npm_inputs(target, acquisition_node_archive, acquisition_node_sha256,
        acquisition_node_bytes, npm_archive, npm_archive_sha256, npm_archive_bytes)
    if acquisition is not None and not explicit:
        raise ValueError("npm resources require explicit current starter and library selection")
    node_build_raw = captured_node_build_profile(node_build_profile, node_build_profile_sha256,
                                               selected_node=acquisition is not None)
    prebuilt_policy_raw = None
    if node_prebuilt_policy is not None or node_prebuilt_policy_sha256 is not None:
        if node_prebuilt_policy is None or not node_prebuilt_policy_sha256 or acquisition is None:
            raise ValueError("Node prebuilt policy requires an exact pair and selected npm resources")
        with os.fdopen(os.open(node_prebuilt_policy, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
            metadata = os.fstat(source.fileno())
            if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= 65536:
                raise ValueError("invalid Node prebuilt policy input")
            prebuilt_policy_raw = source.read(65537)
        if hashlib.sha256(prebuilt_policy_raw).hexdigest() != node_prebuilt_policy_sha256:
            raise ValueError("Node prebuilt policy pin mismatch")
    selected = TARGETS[target]
    python, node = (selected[key] for key in ("python", "node"))
    selected_library, selected_starter = library, starter
    library = selected["library"]
    destination = destination.resolve()
    if destination.exists():
        raise ValueError("fresh destination required")
    if target == "linux-x86_64":
        require_case_sensitive(destination.parent)
    for name, sha, size in (python, node, PIP):
        pin(upstream / name, sha, size)
    input_selection = None
    if explicit:
        archive, library, starter, starter_files, input_selection = selected_resource_inputs(
            target, selected_library, library_sha256, library_bytes, selected_starter, release_manifest_sha256)
    else:
        archive = ROOT / "dist" / selected["library_dir"] / library[0]
        pin(archive, library[1], library[2])
        starter = ROOT / "dist" / selected["starter"]
        pins = STARTERS[selected["starter"]]
        if {p.name for p in starter.iterdir()} != set(STARTER_FILES):
            raise ValueError("unexpected starter closure")
        for name in STARTER_FILES:
            pin(starter / name, pins[name])
        starter_files = {name: {"sha256": pins[name]} for name in STARTER_FILES}
    pip_license = PREPARE.parent / "notices/pip-LICENSE.txt"
    pin(pip_license, PIP_LICENSE)
    source_prepare_sha = digest(PREPARE)
    source_packager_sha = digest(Path(__file__)) if explicit else None
    helpers = runpy.run_path(str(PREPARE))
    destination.mkdir()
    package = destination / ("meerkat-connect-" + target)
    resources = package / "libexec/meerkat-connect"
    resources.mkdir(parents=True)
    (package / "bin").mkdir()
    with tempfile.TemporaryDirectory(prefix="connect-resource-unpack-") as scratch:
        unpacked = Path(scratch) / "python.tar"
        with captured_archive(upstream / python[0], python[1], python[2]) as selected_python, \
                unpacked.open("xb") as python_tar:
            subprocess.run([str(zstd), "-d", "-c", str(selected_python)], stdout=python_tar,
                           stderr=subprocess.PIPE, check=True, timeout=60)
        extract(unpacked, resources, python_mapping, target=target)
    prefix = node[0].removesuffix(".tar.gz") + "/"
    with captured_archive(upstream / node[0], node[1], node[2]) as selected_node:
        extract(selected_node, resources,
                lambda name: "runtime/node/" + name.removeprefix(prefix) if name.startswith(prefix) else None,
                target=target)
    if acquisition is not None:
        # Extraction reads only the captured archive, never its mutable input
        # pathname. The library pair remains in runtime/node unchanged.
        with tempfile.TemporaryDirectory(prefix="connect-acquisition-unpack-") as scratch:
            selected_archive = Path(scratch) / acquisition_node_archive.name
            selected_archive.write_bytes(acquisition["node_raw"])
            prefix = acquisition_node_archive.name.removesuffix(".tar.gz") + "/"
            extract(selected_archive, resources,
                lambda name: "runtime/acquisition-node/" + name.removeprefix(prefix)
                if name.startswith(prefix) else None, target=target)
        observed = node_metadata(resources / "runtime/acquisition-node", target, helpers["decode"])
        if not acquisition["owner"]["_runtime_matches"](observed, target):
            raise ValueError("acquisition Node metadata is unsupported by the npm owner")
        (resources / "runtime/npm-acquisition.tgz").write_bytes(acquisition["npm_raw"])
        acquisition["evidence"]["runtime_metadata"] = observed
        acquisition["evidence"]["scope"] = "captured_archive_metadata_only_not_runtime_qualification"
    if prebuilt_policy_raw is not None:
        policy = helpers["decode"](prebuilt_policy_raw)
        if (not isinstance(policy, dict) or set(policy) != {"schema_version", "scope", "target", "allowed_origins", "allowed_redirect_origins"}
                or type(policy["schema_version"]) is not int or policy["schema_version"] != 1
                or policy["scope"] != "trusted_public_node_prebuilt_resources"
                or not isinstance(policy["target"], dict)
                or policy["target"].get("node_sha256") != digest(resources / "runtime/acquisition-node/bin/node")
                or (policy["target"].get("platform"), policy["target"].get("arch")) !=
                    {"linux-x86_64": ("linux", "x64"), "macos-arm64": ("darwin", "arm64")}[target]):
            raise ValueError("Node prebuilt policy does not bind the selected target")
        (resources / "node-prebuilt-policy.json").write_bytes(prebuilt_policy_raw)
    if node_build_raw is not None:
        (resources / "node-build-profile.json").write_bytes(node_build_raw)
    library_release = None
    # Both selection modes inspect and package the same captured library bytes.
    with captured_archive(archive, library[1], library[2]) as selected_archive:
        compatibility = runtime_compatibility(
            selected_archive, resources, target, SUPPORTED_HOSTS[target], helpers["decode"])
        (resources / "archives").mkdir()
        shutil.copyfile(selected_archive, resources / "archives" / library[0])
        pin(resources / "archives" / library[0], library[1], library[2])
        if immutable_library_release:
            library_release = materialize_library(selected_archive, resources, target)
    shutil.copyfile(upstream / PIP[0], resources / "runtime/pip.pyz")
    pin(resources / "runtime/pip.pyz", PIP[1], PIP[2])
    shutil.copyfile(pip_license, resources / "notices/pip-LICENSE.txt")
    pin(resources / "notices/pip-LICENSE.txt", PIP_LICENSE)
    (resources / "notices/runtime-compatibility.json").write_text(json.dumps(compatibility, indent=2) + "\n")
    (resources / "starter").mkdir()
    for name in STARTER_FILES:
        shutil.copyfile(starter / name, resources / "starter" / name)
        (resources / "starter" / name).chmod(0o644)
        pin(resources / "starter" / name, starter_files[name]["sha256"], starter_files[name].get("bytes"))
    shutil.copyfile(PREPARE, resources / "prepare.py")
    if digest(resources / "prepare.py") != source_prepare_sha:
        raise ValueError("preparation source changed during packaging")
    sources = [
        {"name": "pip-26.2.1.pyz", "sha256": PIP[1], "size": PIP[2],
         "url": "https://github.com/pypa/pip"},
        {"name": python[0], "sha256": python[1], "size": python[2],
         "url": "https://github.com/astral-sh/python-build-standalone/releases/download/20260901/" + python[0]},
        {"name": node[0], "sha256": node[1], "size": node[2],
         "url": "https://nodejs.org/download/release/" + compatibility["runtime_metadata"]["node"]["version"] + "/" + node[0]},
    ]
    # sources records actual upstream downloads, not the entire composition.
    # A locally produced library, selected or default, has no invented hosted
    # URL; its entries/archive and build.json selection retain its exact identity.
    if not explicit and "library_url" in selected:
        sources.append({"name": library[0], "sha256": library[1], "size": library[2],
                        "url": selected["library_url"]})
    # The accepted installer invokes isolated Python for some venv operations.
    # -I ignores PYTHONDONTWRITEBYTECODE. A read-only installation lets CPython
    # skip cache writes without adding mutable exceptions to the resource pin.
    # This is an operational layout, not a same-user security boundary.
    for directory, subdirs, files in os.walk(resources, followlinks=False):
        for name in files:
            path = Path(directory) / name
            if not path.is_symlink():
                path.chmod(0o555 if path.stat().st_mode & 0o111 else 0o444)
        if Path(directory) != resources:
            Path(directory).chmod(0o555)
    manifest = {"schema_version": 1, "kind": "meerkat_connect_resources", "target": target,
        "prepare": "prepare.py", "python": "runtime/python/bin/python3.11", "node": "runtime/node/bin/node",
        "npm_cli": "runtime/node/lib/node_modules/npm/bin/npm-cli.js", "starter": "starter/start.py",
        "archive": "archives/" + library[0], "sources": sources, "entries": helpers["tree_entries"](resources)}
    if library_release is not None:
        manifest["library_release"] = library_release
    if families is not None:
        manifest["families"] = families
    if acquisition is not None:
        manifest["npm_acquisition"] = {"node": "runtime/acquisition-node/bin/node",
            "npm_cli": "runtime/acquisition-node/lib/node_modules/npm/bin/npm-cli.js",
            "npm_archive": "runtime/npm-acquisition.tgz"}
    raw = (json.dumps(manifest, indent=2) + "\n").encode()
    (resources / "resources.json").write_bytes(raw)
    (resources / "resources.json").chmod(0o444)
    resources.chmod(0o555)
    helpers["verify_resources"](resources)
    output = destination / ("meerkat-connect-resources-" + target + ".tar.gz")
    member_count = content_bytes = largest_member_bytes = 0
    with output.open("xb") as stream, gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as tar:
            for path in [package, *sorted(package.rglob("*"))]:
                member = tar.gettarinfo(str(path), arcname=path.relative_to(destination).as_posix())
                member.uid = member.gid = 0
                member.uname = member.gname = ""
                member.mtime = 0
                member_count += 1
                content_bytes += member.size
                largest_member_bytes = max(largest_member_bytes, member.size)
                if path.is_file() and not path.is_symlink():
                    with path.open("rb") as payload:
                        tar.addfile(member, payload)
                else:
                    tar.addfile(member)
    result = {"scope": target.replace("-", "_") + "_product_resources_only_native_cli_supplied_separately",
        "resources": str(resources), "resources_sha256": hashlib.sha256(raw).hexdigest(),
        "entries": len(manifest["entries"]), "archive": str(output), "archive_sha256": digest(output),
        "archive_bytes": output.stat().st_size, "prepare_source_sha256": source_prepare_sha,
        "runtime_compatibility": compatibility,
        "resource_volume": {"manifest_bytes": len(raw),
            "archive_members": member_count, "archive_member_content_bytes": content_bytes,
            "largest_archive_member_bytes": largest_member_bytes,
            "resource_entries_including_manifest": len(manifest["entries"]) + 1}}
    if library_release is not None:
        result["library_release"] = library_release
    if explicit:
        if digest(Path(__file__)) != source_packager_sha or digest(PREPARE) != source_prepare_sha:
            raise ValueError("packaging source changed during packaging")
        result.update({"schema_version": 1, "target": target,
                       "packager_source_sha256": source_packager_sha, "input_selection": input_selection})
    if acquisition is not None:
        result["npm_acquisition_inputs"] = acquisition["evidence"]
    if prebuilt_policy_raw is not None:
        result["node_prebuilt_policy_input"] = {"path": str(node_prebuilt_policy),
            "sha256": node_prebuilt_policy_sha256, "bytes": len(prebuilt_policy_raw),
            "scope": "explicit_target_and_origins_host_binds_selected_python_during_preparation"}
    if node_build_raw is not None:
        result["node_build_profile_input"] = {"path": str(node_build_profile),
            "sha256": node_build_profile_sha256, "bytes": len(node_build_raw),
            "scope": "exact_profile_bytes_external_toolchain_paths_remain_host_owned"}
    (destination / "build.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--upstream", type=Path, required=True)
    parser.add_argument("--zstd", type=Path, required=True)
    parser.add_argument("--target", choices=tuple(TARGETS), default="macos-arm64")
    parser.add_argument("--library", type=Path)
    parser.add_argument("--library-sha256")
    parser.add_argument("--library-bytes", type=int)
    parser.add_argument("--starter", type=Path)
    parser.add_argument("--release-manifest-sha256")
    parser.add_argument("--immutable-library-release", action="store_true",
        help="package the complete verified readonly library in resources; requires explicit current inputs")
    parser.add_argument("--families", help="explicit comma-separated setup families, pinned in resources.json; requires explicit library and starter inputs")
    parser.add_argument("--acquisition-node-archive", type=Path)
    parser.add_argument("--acquisition-node-sha256")
    parser.add_argument("--acquisition-node-bytes", type=int)
    parser.add_argument("--npm-archive", type=Path)
    parser.add_argument("--npm-archive-sha256")
    parser.add_argument("--npm-archive-bytes", type=int)
    parser.add_argument("--node-prebuilt-policy", type=Path)
    parser.add_argument("--node-prebuilt-policy-sha256")
    parser.add_argument("--node-build-profile", type=Path)
    parser.add_argument("--node-build-profile-sha256")
    args = parser.parse_args()
    selected = {key: getattr(args, key) for key in (
        "library", "library_sha256", "library_bytes", "starter", "release_manifest_sha256")}
    if not any(value is not None for value in selected.values()):
        selected = {}
    if args.immutable_library_release:
        selected["immutable_library_release"] = True
    if args.families is not None:
        selected["families"] = args.families
    acquisition = {key: getattr(args, key) for key in (
        "acquisition_node_archive", "acquisition_node_sha256", "acquisition_node_bytes",
        "npm_archive", "npm_archive_sha256", "npm_archive_bytes")}
    if any(value is not None for value in acquisition.values()):
        selected.update(acquisition)
    if args.node_prebuilt_policy is not None or args.node_prebuilt_policy_sha256 is not None:
        selected.update(node_prebuilt_policy=args.node_prebuilt_policy,
                        node_prebuilt_policy_sha256=args.node_prebuilt_policy_sha256)
    if args.node_build_profile is not None or args.node_build_profile_sha256 is not None:
        selected.update(node_build_profile=args.node_build_profile,
                        node_build_profile_sha256=args.node_build_profile_sha256)
    print(json.dumps(build(args.destination, args.upstream, args.zstd, target=args.target, **selected), sort_keys=True))


if __name__ == "__main__":
    main()
