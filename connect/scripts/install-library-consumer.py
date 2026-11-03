#!/usr/bin/env python3
"""Install an explicitly selected offline consumer, or reconcile an existing one.

Uses ordinary pip/npm and an operator-selected CONTENTS.json digest. This is
software installation, not account authentication, agent setup or activation.
--verify-existing never installs or repairs. Library readback is inert; selected
readback invokes the trusted metadata validator on private temporary copies.
"""
from __future__ import annotations

import argparse
import base64
import configparser
import csv
import email.parser
import gzip
import hashlib
import io
import importlib.machinery
import json
import os
from pathlib import Path, PurePosixPath
import re
import selectors
import signal
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import zipfile
import zlib

MAX_FILE = 64 * 1024 * 1024
MAX_CONFIGURATION = 64 * 1024
MAX_TOTAL = 512 * 1024 * 1024
MAX_NPM_MEMBERS = 20000
MAX_NPM_EXPANDED = 256 * 1024 * 1024
DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
FAMILIES = {
    "agno": ("meerkat_toolkit_agno", "meerkat_toolkit_agno.agno_mcp"),
    "services": ("meerkat_toolkit_services", "meerkat_toolkit_services.adapters.services_mcp"),
}
PYTHON_GRAPH = "import importlib.metadata as m,json,sysconfig; print(json.dumps({'distributions':[[d.metadata['Name'],d.version] for d in m.distributions()],'site':sysconfig.get_path('purelib')}))"
SELECTED_PYTHON_TARGETS = {"macos-arm64": ("darwin", "arm64"), "linux-x86_64": ("linux", "x86_64")}
NODE_NATIVE_MANIFEST = "closure/native-payloads"
NODE_NATIVE_SOURCE = "closure/native-source-receipt"
NODE_NATIVE_TARGET_EXPRESSION = (
    "JSON.stringify({node_version:process.version,node_abi:process.versions.modules,"
    "platform:process.platform,arch:process.arch,libc:process.platform==='linux'?"
    "{family:'glibc',version:process.report.getReport().header.glibcVersionRuntime}:null})"
)


class InstallError(ValueError):
    pass


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encoded(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode()


def name(value: str) -> str:
    return re.sub(r"[-_.]+", "-", value).lower()


def relative(value: str) -> tuple[str, ...]:
    path = PurePosixPath(value)
    # Wheel data and the trusted venv bootstrap include spaces and parentheses.
    if (not value or not path.parts or path.is_absolute() or str(path) != value or ".." in path.parts
            or not re.fullmatch(r"[A-Za-z0-9_./@+ ()-]+", value)):
        raise InstallError("invalid relative path")
    return path.parts


def directory(path: Path) -> int:
    handle = os.open(path.anchor, DIR_FLAGS)
    try:
        for part in path.parts[1:]:
            child = os.open(part, DIR_FLAGS, dir_fd=handle)
            os.close(handle)
            handle = child
        return handle
    except BaseException:
        os.close(handle)
        raise


def read(root: Path, path: str, limit: int = MAX_FILE) -> bytes:
    parts = relative(path)
    handle = directory(root)
    try:
        for part in parts[:-1]:
            child = os.open(part, DIR_FLAGS, dir_fd=handle)
            os.close(handle)
            handle = child
        fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=handle)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
                raise InstallError("input must be a bounded regular file")
            data = stream.read(limit + 1)
            if len(data) > limit:
                raise InstallError("input too large")
            return data
    finally:
        os.close(handle)


def library_runtime_target(contents: dict) -> dict | None:
    """Schema 1 keeps its historical target; schema 2 binds an exact Linux tuple."""
    if type(contents["schema_version"]) is not int:
        raise InstallError("CONTENTS schema must be an integer")
    if contents["schema_version"] == 1:
        if "runtime_target" in contents:
            raise InstallError("legacy contents cannot override its runtime target")
        return None
    if contents["schema_version"] != 2:
        raise InstallError("unsupported CONTENTS schema")
    target = contents.get("runtime_target")
    if not isinstance(target, dict) or set(target) != {"python", "node"}:
        raise InstallError("complete exact runtime target required")
    python, node = target["python"], target["node"]
    if (not isinstance(python, dict)
            or set(python) != {"implementation", "version", "platform", "machine", "pointer_bits", "glibc"}
            or python["implementation"] != "cpython" or python["platform"] != "linux"
            or python["machine"] != "x86_64" or type(python["pointer_bits"]) is not int
            or python["pointer_bits"] != 64
            or not isinstance(python["version"], str) or not re.fullmatch(r"3\.11\.[0-9]+", python["version"])
            or not isinstance(python["glibc"], str) or not re.fullmatch(r"glibc [0-9]+\.[0-9]+", python["glibc"])):
        raise InstallError("unsupported exact Python runtime target")
    if (not isinstance(node, dict) or set(node) != {"version", "platform", "arch", "npm_version"}
            or node["platform"] != "linux" or node["arch"] != "x64"
            or not isinstance(node["version"], str) or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", node["version"])
            or not isinstance(node["npm_version"], str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", node["npm_version"])):
        raise InstallError("unsupported exact Node runtime target")
    return target


def inputs(bundle: Path, digest: str, family: str) -> tuple[dict[str, bytes], dict | None]:
    if not re.fullmatch(r"[a-f0-9]{64}", digest):
        raise InstallError("expected CONTENTS digest must be SHA-256")
    raw = read(bundle, "CONTENTS.json", 1024 * 1024)
    if sha(raw) != digest:
        raise InstallError("CONTENTS differs from operator-selected digest")
    contents = unique_json(raw)
    target = library_runtime_target(contents)
    files = {}
    total = 0
    for path, record in contents["files"].items():
        parts = relative(path)
        if parts[0] != family:
            continue
        if len(parts) != 2 or len(files) >= 256:
            raise InstallError("unsupported family layout")
        data = read(bundle, path)
        total += len(data)
        if total > MAX_TOTAL or sha(data) != record["sha256"]:
            raise InstallError("family input differs from selected contents")
        files[parts[1]] = data
    if not files:
        raise InstallError("family missing from selected bundle")
    return files, target


def python_closure(files: dict[str, bytes], family: str) -> dict[str, str]:
    requirements = {}
    for line in files["install-offline.txt"].decode("ascii").splitlines():
        match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9_.+!-]+) --hash=sha256:([a-f0-9]{64})", line)
        if not match or name(match[1]) in requirements:
            raise InstallError("offline requirements must be unique exact pins and hashes")
        requirements[name(match[1])] = (match[2], match[3])
    wheels = {}
    for filename, data in files.items():
        if not filename.endswith(".whl"):
            continue
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            metadata = [item for item in archive.infolist() if item.filename.endswith(".dist-info/METADATA")]
            if len(metadata) != 1 or metadata[0].file_size > 1024 * 1024:
                raise InstallError("invalid wheel metadata")
            info = email.parser.BytesParser().parsebytes(archive.read(metadata[0]))
            package = name(info["Name"])
            if package in wheels or any("@" in value or "://" in value for value in info.get_all("Requires-Dist", [])):
                raise InstallError("duplicate wheel or nonlocal dependency reference")
            wheels[package] = (info["Version"], sha(data))
    if not requirements or wheels != requirements or name(FAMILIES[family][0]) not in wheels:
        raise InstallError("wheel closure must exactly match hash requirements")
    return {key: value[0] for key, value in requirements.items()}


def npm_closure(files: dict[str, bytes]) -> list[dict]:
    lock = json.loads(files["package-lock.json"])
    cache = json.loads(files["offline-cache.json"])
    if lock["lockfileVersion"] != 3 or cache["schema_version"] != 1:
        raise InstallError("unsupported npm lock/cache format")
    rows = cache["tarballs"]
    component = "@rkat/toolkit-activepieces"
    archive = "rkat-toolkit-activepieces-0.1.0.tgz"
    package = json.loads(files["package.json"])
    dependencies = {component: "file:./" + archive}
    packages = lock["packages"]
    if (package.get("dependencies") != dependencies or packages[""].get("dependencies") != dependencies
            or any(package.get(key) for key in ("devDependencies", "optionalDependencies", "workspaces"))
            or packages["node_modules/" + component]["resolved"] != "file:" + archive
            or archive not in files):
        raise InstallError("npm project must select only the retained local component")
    expected = {}
    for path, row in packages.items():
        if not path or path == "node_modules/" + component:
            continue
        dependency = path.removeprefix("node_modules/")
        if (path != "node_modules/" + dependency or "/node_modules/" in path
                or not re.fullmatch(r"(?:@[a-z0-9_.-]+/)?[a-z0-9_.-]+", dependency)
                or not row["resolved"].startswith("https://") or row.get("link")):
            raise InstallError("npm lock contains an unsupported dependency location")
        expected[dependency] = row
    if (len(rows) != len(expected) or {row["name"] for row in rows} != set(expected)
            or len({row["path"] for row in rows}) != len(rows)):
        raise InstallError("npm cache does not cover lock")
    for row in rows:
        if len(relative(row["path"])) != 1:
            raise InstallError("npm tarball must be local")
        bound = expected[row["name"]]
        if (row["version"] != bound["version"] or row["integrity"] != bound["integrity"]
                or sha(files[row["path"]]) != row["sha256"]):
            raise InstallError("npm tarball differs from selected lock")
    return rows


def run(argv: list[str], cwd: Path, env: dict[str, str], label: str, timeout: int = 900,
        *, discard_output: bool = False) -> bytes:
    """Own and join subprocesses; do not inherit shell settings or credentials."""
    # Configuration validators may print account or endpoint values. Suppress
    # both streams at the child boundary, without a temporary raw log or buffer.
    out_path = Path(os.devnull) if discard_output else cwd / (label + ".stdout")
    err_path = Path(os.devnull) if discard_output else cwd / (label + ".stderr")
    mode = "wb" if discard_output else "xb"
    with out_path.open(mode) as out, err_path.open(mode) as err:
        child, interrupted = None, False
        caller_handlers = {signum: signal.getsignal(signum)
                           for signum in (signal.SIGINT, signal.SIGTERM)}
        def defer_interrupt(signum, frame):
            nonlocal interrupted
            interrupted = True
        try:
            # Keep flag-only handlers until this direct child is joined. They
            # defer signals across Popen handle publication and coalesce later
            # signals during cleanup, without another handler transition or a
            # blocked signal mask inherited by the child's exec.
            for signum in caller_handlers:
                signal.signal(signum, defer_interrupt)
            child = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err,
                                     start_new_session=True)
            deadline = time.monotonic() + timeout
            while True:
                if interrupted:
                    raise KeyboardInterrupt
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise subprocess.TimeoutExpired(argv, timeout)
                try:
                    code = child.wait(timeout=min(0.1, remaining))
                    break
                except subprocess.TimeoutExpired:
                    continue
            # Cancellation concurrent with exit must not enable the next step.
            if interrupted:
                raise KeyboardInterrupt
            if code:
                raise InstallError("installer step failed: " + label)
        except BaseException:
            if child is None:
                raise
            try:
                if child.returncode is None:
                    try:
                        os.killpg(child.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        child.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        pass
                    if child.returncode is None:
                        try:
                            os.killpg(child.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
            finally:
                # Signal failure cannot release a still-live direct child.
                # A returned wait is direct-child evidence only. Never signal
                # a PID/group after its leader has been reaped.
                child.wait()
            raise
        finally:
            try:
                signal.signal(signal.SIGINT, caller_handlers[signal.SIGINT])
            finally:
                signal.signal(signal.SIGTERM, caller_handlers[signal.SIGTERM])
    # A signal can be coalesced while the original pair is being restored
    # after a successful wait. Do not lose it before the next installer step.
    if interrupted:
        raise KeyboardInterrupt
    return b"" if discard_output else out_path.read_bytes()


def selected_configuration(args: argparse.Namespace) -> bytes | None:
    """Freeze an explicit byte selection; adapter validators own its meaning."""
    selected = tuple(getattr(args, key, None) for key in ("config", "config_sha256", "server_name"))
    if all(value is None for value in selected):
        return None
    path_value, digest, server = selected
    if (any(value is None for value in selected)
            or not re.fullmatch(r"[a-f0-9]{64}", digest)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", server)):
        raise InstallError("complete explicit configuration selection required")
    path = Path(path_value)
    if not path.is_absolute() or str(path) != path_value or any(part in (".", "..") for part in path_value.split("/")):
        raise InstallError("configuration must be an absolute canonical file path")
    raw = read(path.parent, path.name, MAX_CONFIGURATION)
    if sha(raw) != digest:
        raise InstallError("configuration differs from selected bytes")
    return raw


def new_destination(value: str, bundle: Path) -> Path:
    path = Path(value)
    if not path.is_absolute() or any(part in (".", "..") for part in value.split("/")) or path == path.parent:
        raise InstallError("destination must be an absolute new directory")
    result = path.parent.resolve(strict=True) / path.name
    if result == bundle or bundle in result.parents or os.path.lexists(result):
        raise InstallError("destination exists or lies inside the bundle")
    return result


def executable(value: str | None) -> str:
    if not value or not Path(value).is_absolute():
        raise InstallError("supply an absolute selected runtime path")
    return str(Path(value).resolve(strict=True))


def python_graph(observed: dict, venv: Path) -> tuple[dict[str, str], Path]:
    """Keep duplicate distributions visible and bind reads to this fresh venv."""
    site = Path(observed["site"])
    if site != venv / "lib/python3.11/site-packages":
        raise InstallError("installed Python site differs from selected environment")
    graph = {}
    for package, version in observed["distributions"]:
        normalized = name(package)
        if normalized in graph:
            raise InstallError("duplicate installed Python distribution")
        graph[normalized] = version
    return graph, site


def verify_wheel_payload(wheel: Path, installed: Path, venv: Path) -> dict:
    """Compare retained wheel bytes, allowing only pip's recorded relocations."""
    count = 0
    script_projections = []
    with zipfile.ZipFile(wheel) as archive:
        members = archive.infolist()
        if len(members) != len({item.filename for item in members}):
            raise InstallError("duplicate wheel member")
        for item in members:
            if item.is_dir():
                continue
            relative(item.filename)
            if item.file_size > MAX_FILE:
                raise InstallError("wheel member exceeds byte limit")
            # pip rewrites RECORD with generated scripts and installer metadata.
            # It is not an independent source of expected installed bytes.
            if item.filename.endswith(".dist-info/RECORD"):
                continue
            expected = archive.read(item)
            if ".data/" in item.filename:
                _, relocated = item.filename.split(".data/", 1)
                if not relocated.startswith("scripts/"):
                    raise InstallError("unsupported wheel relocation")
                script = relocated.removeprefix("scripts/")
                relative(script)
                actual = read(venv, "bin/" + script)
                source_line, separator, body = expected.partition(b"\n")
                if source_line in (b"#!python", b"#!pythonw") and separator:
                    # The existing verifier permits only the installer-selected
                    # interpreter line to differ, never the script body.
                    target_line, target_separator, target_body = actual.partition(b"\n")
                    if target_line != b"#!" + str(venv / "bin/python").encode() or not target_separator or target_body != body:
                        raise InstallError("installed script differs beyond interpreter line")
                    script_projections.append(item.filename)
                elif actual != expected:
                    raise InstallError("installed script differs from wheel")
            elif read(installed, item.filename) != expected:
                raise InstallError("installed wheel payload differs from retained archive")
            count += 1
    if not count:
        raise InstallError("wheel payload missing")
    return {"payloads": count, "interpreter_line_projections": script_projections}


def verify_npm_payload(archive_path: Path, installed: Path) -> int:
    count = 0
    with tarfile.open(archive_path) as archive:
        for item in archive.getmembers():
            if item.isdir():
                continue
            if not item.isfile() or not item.name.startswith("package/") or item.size > MAX_FILE:
                raise InstallError("unsupported npm archive member")
            path = item.name[len("package/"):]
            relative(path)
            with archive.extractfile(item) as source:
                if read(installed, path) != source.read(MAX_FILE + 1):
                    raise InstallError("installed npm payload differs from archive")
            count += 1
    return count


def npm_environment(destination: Path, runtime: str, env: dict[str, str]) -> dict[str, str]:
    """Pin npm configuration to the new operator-owned root."""
    for filename in ("user.npmrc", "global.npmrc"):
        (destination / filename).write_text("")
    return {**env, "PATH": str(Path(runtime).parent) + ":/usr/bin:/bin",
            "npm_config_userconfig": str(destination / "user.npmrc"),
            "npm_config_globalconfig": str(destination / "global.npmrc"),
            "npm_config_cache": str(destination / "npm-cache"), "npm_config_ignore_scripts": "true",
            "npm_config_offline": "true", "npm_config_update_notifier": "false"}


def unique_json(raw: bytes) -> dict:
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise InstallError("duplicate JSON key")
            result[key] = value
        return result
    value = json.loads(raw, object_pairs_hook=pairs)
    if not isinstance(value, dict):
        raise InstallError("JSON object required")
    return value


def npm_archive(raw: bytes) -> dict:
    """Inspect bounded archive bytes before npm is allowed to extract them."""
    with gzip.GzipFile(fileobj=io.BytesIO(raw)) as compressed:
        expanded = compressed.read(MAX_NPM_EXPANDED + 1)
    if len(expanded) > MAX_NPM_EXPANDED:
        raise InstallError("npm archive expansion exceeds limit")
    files, paths, total, wrapper = {}, set(), 0, None
    with tarfile.open(fileobj=io.BytesIO(expanded), mode="r:") as archive:
        for index, member in enumerate(archive):
            if index >= MAX_NPM_MEMBERS:
                raise InstallError("too many npm archive members")
            clean = member.name.rstrip("/") if member.isdir() else member.name
            parts = relative(clean)
            if wrapper is None:
                wrapper = parts[0]
            if parts[0] != wrapper or (not member.isdir() and len(parts) < 2):
                raise InstallError("npm archive must stay inside one wrapper directory")
            if (not member.isdir() and not member.isfile()) or member.issparse():
                raise InstallError("npm archive links and special files are unsupported")
            if clean.casefold() in paths or "node_modules" in parts[1:]:
                raise InstallError("duplicate, aliased or bundled npm archive path")
            paths.add(clean.casefold())
            if member.isdir():
                continue
            total += member.size
            if member.size > MAX_FILE or total > MAX_NPM_EXPANDED:
                raise InstallError("npm archive payload exceeds limit")
            with archive.extractfile(member) as source:
                data = source.read(MAX_FILE + 1)
            if len(data) != member.size:
                raise InstallError("truncated npm archive member")
            # npm removes one archive wrapper; package.json and the exact lock
            # own package identity independently of that directory's name.
            files["/".join(parts[1:])] = data
    metadata = unique_json(files["package.json"])
    if not re.fullmatch(r"(?:@[a-z0-9_.-]+/)?[a-z0-9_.-]+", metadata.get("name", "")):
        raise InstallError("unsupported npm package name")
    if not isinstance(metadata.get("version"), str) or not metadata["version"]:
        raise InstallError("npm version required")
    if metadata.get("bundleDependencies") or metadata.get("bundledDependencies"):
        raise InstallError("bundled npm dependencies are unsupported")
    return {"name": metadata["name"], "version": metadata["version"], "files": files,
            "integrity": "sha512-" + base64.b64encode(hashlib.sha512(raw).digest()).decode(),
            "expanded_bytes": len(expanded)}


def npm_location(value: str) -> str:
    """A lock row is an exact installed path, including nested versions."""
    parts = relative(value)
    offset, package = 0, ""
    while offset < len(parts):
        if parts[offset] != "node_modules" or offset + 1 >= len(parts):
            raise InstallError("unsupported npm lock location")
        offset += 1
        if parts[offset].startswith("@"):
            if offset + 1 >= len(parts):
                raise InstallError("invalid scoped npm location")
            package = parts[offset] + "/" + parts[offset + 1]
            offset += 2
        else:
            package = parts[offset]
            offset += 1
        if not re.fullmatch(r"(?:@[a-z0-9_.-]+/)?[a-z0-9_.-]+", package):
            raise InstallError("unsupported npm package path")
    return package


def native_keys(value: object, keys: set[str]) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise InstallError("native payload object has unexpected fields")
    return value


def native_hash(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{64}", value):
        raise InstallError("native payload SHA-256 required")
    return value


def checked_node_native_target(value: object) -> dict:
    """Validate a declared exact target; this does not establish module readiness."""
    target = native_keys(value, {"node_sha256", "node_version", "node_abi", "platform", "arch", "libc"})
    native_hash(target["node_sha256"])
    if (not isinstance(target["node_version"], str)
            or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", target["node_version"])
            or not isinstance(target["node_abi"], str) or not re.fullmatch(r"[1-9][0-9]{0,5}", target["node_abi"])
            or (target["platform"], target["arch"]) not in (("linux", "x64"), ("darwin", "arm64"))):
        raise InstallError("unsupported exact native Node target")
    if target["platform"] == "linux":
        libc = native_keys(target["libc"], {"family", "version"})
        if (libc["family"] != "glibc" or not isinstance(libc["version"], str)
                or not re.fullmatch(r"[0-9]+\.[0-9]+", libc["version"])):
            raise InstallError("native Linux payload requires an explicit glibc target")
    elif target["libc"] is not None:
        raise InstallError("Darwin native target must not declare Linux libc")
    return target


def node_native_archive(raw: bytes) -> dict[str, bytes]:
    """Read a bounded prebuilt tar.gz without extracting or executing any member."""
    if len(raw) > MAX_FILE:
        raise InstallError("native archive exceeds input limit")
    with gzip.GzipFile(fileobj=io.BytesIO(raw)) as compressed:
        expanded = compressed.read(MAX_NPM_EXPANDED + 1)
    if len(expanded) > MAX_NPM_EXPANDED:
        raise InstallError("native archive expansion exceeds limit")
    files, seen, spellings, total = {}, set(), {}, 0
    with tarfile.open(fileobj=io.BytesIO(expanded), mode="r:") as archive:
        for index, member in enumerate(archive):
            if index >= MAX_NPM_MEMBERS:
                raise InstallError("too many native archive members")
            clean = member.name.rstrip("/") if member.isdir() else member.name
            relative(clean)
            if ((not member.isdir() and not member.isfile()) or member.issparse()
                    or clean.casefold() in seen):
                raise InstallError("native archive links, special files and aliases are unsupported")
            seen.add(clean.casefold())
            for prefix in [PurePosixPath(clean), *PurePosixPath(clean).parents]:
                value = str(prefix)
                if spellings.setdefault(value.casefold(), value) != value:
                    raise InstallError("native archive has case-aliased directories")
            if member.isdir():
                continue
            total += member.size
            if member.size < 0 or member.size > MAX_FILE or total > MAX_NPM_EXPANDED:
                raise InstallError("native archive payload exceeds limit")
            with archive.extractfile(member) as source:
                data = source.read(MAX_FILE + 1)
            if len(data) != member.size:
                raise InstallError("truncated native archive member")
            files[clean] = data
    checked_paths(list(files))
    file_paths = {path.casefold() for path in files}
    if any(str(parent) in file_paths for path in seen for parent in PurePosixPath(path).parents):
        raise InstallError("native archive file and directory collide")
    return files


def node_native_inputs(files: dict[str, bytes]) -> tuple[dict, dict | None]:
    """Separate explicit supplemental inputs from the unchanged original npm graph."""
    reserved = {key for key in files if key in (NODE_NATIVE_MANIFEST, NODE_NATIVE_SOURCE)
                or key.startswith("closure/native-asset-")}
    if not reserved:
        return files, None
    if NODE_NATIVE_MANIFEST not in files or NODE_NATIVE_SOURCE not in files:
        raise InstallError("complete native payload manifest and source receipt required")
    if len(files[NODE_NATIVE_MANIFEST]) > MAX_CONFIGURATION or len(files[NODE_NATIVE_SOURCE]) > 4 * 1024 * 1024:
        raise InstallError("native payload provenance exceeds document limit")
    manifest = native_keys(unique_json(files[NODE_NATIVE_MANIFEST]),
                           {"schema_version", "scope", "source_receipt_sha256", "target", "payloads"})
    if (type(manifest["schema_version"]) is not int or manifest["schema_version"] != 1
            or manifest["scope"] != "selected_node_native_payloads"
            or native_hash(manifest["source_receipt_sha256"]) != sha(files[NODE_NATIVE_SOURCE])):
        raise InstallError("native payload source identity differs")
    checked_node_native_target(manifest["target"])
    payloads = manifest["payloads"]
    if not isinstance(payloads, list) or not 1 <= len(payloads) <= 16:
        raise InstallError("native payload count exceeds limit")
    assets = set()
    for row in payloads:
        native_keys(row, {"package_location", "package_archive_sha256", "asset_id", "url", "files"})
        if (not isinstance(row["asset_id"], str)
                or not re.fullmatch(r"native-asset-[a-f0-9]{64}", row["asset_id"])):
            raise InstallError("native asset ID must bind its digest")
        key = "closure/" + row["asset_id"]
        if key in assets or key not in files or sha(files[key]) != row["asset_id"][len("native-asset-"):]:
            raise InstallError("native asset missing, duplicated or changed")
        assets.add(key)
        native_hash(row["package_archive_sha256"])
        if not isinstance(row["package_location"], str):
            raise InstallError("native payload requires an exact npm lock location")
        npm_location(row["package_location"])
        from urllib.parse import urlsplit
        url = row["url"]
        if not isinstance(url, str) or len(url) > 4096 or not url.isascii() or any(ord(c) < 33 for c in url):
            raise InstallError("native payload requires a public source URL")
        parsed = urlsplit(url)
        if (parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password
                or parsed.query or parsed.fragment):
            raise InstallError("native payload requires an exact HTTPS source URL")
    if reserved != {NODE_NATIVE_MANIFEST, NODE_NATIVE_SOURCE, *assets}:
        raise InstallError("native supplemental inputs differ from manifest")
    base = {key: value for key, value in files.items() if key not in reserved}
    source = unique_json(files[NODE_NATIVE_SOURCE])
    expected = {key: sha(raw) for key, raw in base.items() if key == "artifact.tgz" or key.startswith("closure/")}
    if (type(source.get("schema_version")) is not int or source["schema_version"] != 1
            or source.get("scope") != "retained_node_package_inputs" or source.get("files") != expected):
        raise InstallError("native source receipt differs from original retained npm inputs")
    return base, {"manifest": manifest, "source": source, "assets": {key: files[key] for key in assets},
                  "manifest_sha256": sha(files[NODE_NATIVE_MANIFEST])}


def compose_node_native_payloads(closure: dict, files: dict, native: dict) -> dict:
    primary = closure["rows"][closure["component_path"]][1]
    source = native["source"].get("primary")
    if (not isinstance(source, dict) or source.get("sha256") != sha(files["artifact.tgz"])
            or source.get("name") != primary["name"] or source.get("version") != primary["version"]):
        raise InstallError("native source receipt primary differs from retained component")
    additions, per_package, expanded = {}, {}, 0
    original_bytes = sum(len(raw) for _, archive in closure["rows"].values() for raw in archive["files"].values())
    for row in native["manifest"]["payloads"]:
        package = row["package_location"]
        if package not in closure["rows"] or sha(files[closure["rows"][package][0]]) != row["package_archive_sha256"]:
            raise InstallError("native payload package differs from exact lock and archive")
        archive = node_native_archive(native["assets"]["closure/" + row["asset_id"]])
        mappings = row["files"]
        if not isinstance(mappings, list) or not 1 <= len(mappings) <= 64:
            raise InstallError("native payload mapping count exceeds limit")
        used, has_addon = set(), False
        for item in mappings:
            native_keys(item, {"member", "destination", "sha256"})
            if not isinstance(item["member"], str) or not isinstance(item["destination"], str):
                raise InstallError("native payload paths must be literal text")
            relative(item["member"])
            parts = relative(item["destination"])
            if ("node_modules" in (part.casefold() for part in parts)
                    or item["member"] in used or item["member"] not in archive):
                raise InstallError("native mapping must select one exact retained member")
            raw = archive[item["member"]]
            if native_hash(item["sha256"]) != sha(raw):
                raise InstallError("native member differs from selected digest")
            path = package + "/" + item["destination"]
            if path in additions or path in closure["paths"]:
                raise InstallError("native payload cannot overwrite an npm or supplemental file")
            if (len(closure["paths"]) + len(additions) + 1 > MAX_NPM_MEMBERS
                    or original_bytes + expanded + len(raw) > MAX_TOTAL):
                raise InstallError("combined native npm graph exceeds limits")
            additions[path] = raw
            per_package.setdefault(package, {})[item["destination"]] = sha(raw)
            expanded += len(raw)
            has_addon |= item["destination"].endswith(".node")
            used.add(item["member"])
        if used != set(archive) or not has_addon:
            raise InstallError("native mapping must consume every regular member and include a Node addon")
    combined = list(closure["paths"]) + list(additions)
    checked_paths(combined)
    summary = {"manifest_sha256": native["manifest_sha256"],
               "source_receipt_sha256": native["manifest"]["source_receipt_sha256"],
               "target": native["manifest"]["target"],
               "files": {path: sha(raw) for path, raw in sorted(additions.items())}}
    return {**closure, "npm_paths": closure["paths"], "paths": set(combined),
            "native": {"files": additions, "per_package": per_package, "summary": summary}}


NPM_TARGET_INPUT = "closure/npm-target"


def npm_conditional(lock: dict) -> bool:
    return any(isinstance(row, dict) and (row.get("optional") or row.get("optionalDependencies")
               or any(key in row for key in ("os", "cpu", "libc")))
               for row in lock.get("packages", {}).values())


def npm_platform_list(value: object) -> list[str]:
    if value is None:
        return []
    values = [value] if isinstance(value, str) else value
    if (not isinstance(values, list) or not 1 <= len(values) <= 128
            or any(not isinstance(item, str) or not re.fullmatch(r"!?[a-zA-Z0-9_.-]{1,64}", item)
                   for item in values)):
        raise InstallError("unsupported npm platform restriction")
    return values


def npm_platform_matches(value: str | None, allowed: list[str]) -> bool:
    if not allowed:
        return True
    if value is None:
        return False
    if allowed == ["any"]:
        return True
    return ("!" + value not in allowed
            and (value in allowed or all(item.startswith("!") for item in allowed)))


def npm_target_graph(closure: dict, lock: dict, target: dict) -> dict:
    """Select a bounded production graph; npm still owns version resolution.

    Preserve and validate every archive, including platform-excluded optional
    payloads. No download, engine, script or extraction failure permits omission.
    Peer/workspace/shrinkwrap graphs remain outside this conditional slice.
    """
    checked_node_native_target(target)
    rows, edges = closure["rows"], {}
    incoming = {path: [] for path in rows}
    incompatible, edge_count = set(), 0
    actual_platform = {"os": target["platform"], "cpu": target["arch"],
                       "libc": target["libc"]["family"] if target["libc"] else None}

    def resolve(parent, name):
        if not isinstance(name, str) or npm_location("node_modules/" + name) != name:
            raise InstallError("unsupported npm dependency name")
        parts = list(PurePosixPath(parent).parts)
        while parts:
            if parts[-1] != "node_modules":
                candidate = "/".join([*parts, "node_modules", name])
                if candidate in rows:
                    return candidate
            parts.pop()
        candidate = "node_modules/" + name
        if candidate in rows:
            return candidate
        raise InstallError("npm dependency missing from exact lock graph")

    for path, (_, archive) in rows.items():
        row = lock["packages"][path]
        metadata = unique_json(archive["files"]["package.json"])
        if (any(row.get(key) or metadata.get(key) for key in
                ("peerDependencies", "peerDependenciesMeta", "workspaces"))
                or "npm-shrinkwrap.json" in archive["files"]
                or any(type(row.get(flag, False)) is not bool for flag in ("optional", "dev", "devOptional"))
                or row.get("devOptional")):
            raise InstallError("unsupported conditional npm graph")
        dependencies = {}
        for field, optional in (("optionalDependencies", True), ("dependencies", False)):
            declared, locked = metadata.get(field, {}), row.get(field, {})
            if not isinstance(declared, dict) or not isinstance(locked, dict):
                raise InstallError("npm dependency metadata must be an object")
            # npm may normalize away the overwritten required entry. Compare
            # effective declarations; the optional spec owns a same-name edge.
            if not optional:
                declared = {name: spec for name, spec in declared.items() if name not in dependencies}
                locked = {name: spec for name, spec in locked.items() if name not in dependencies}
            if (declared != locked or len(declared) > MAX_NPM_MEMBERS
                    or any(not isinstance(spec, str) or not spec for spec in declared.values())):
                raise InstallError("npm dependency metadata differs from exact lock")
            dependencies.update({name: optional for name in declared})
        edge_count += len(dependencies)
        if edge_count > MAX_NPM_MEMBERS:
            raise InstallError("conditional npm edge count exceeds limit")
        edges[path] = [(resolve(path, name), optional) for name, optional in dependencies.items()]
        for child, optional in edges[path]:
            incoming[child].append((path, optional))
        for field, actual in actual_platform.items():
            declared = npm_platform_list(metadata.get(field))
            if declared != npm_platform_list(row.get(field)):
                raise InstallError("npm platform metadata differs from exact lock")
            if not npm_platform_matches(actual, declared):
                incompatible.add(path)

    def reachable(excluded=frozenset(), required_only=False):
        seen, pending = set(), [closure["component_path"]]
        while pending:
            path = pending.pop()
            if path in seen or path in excluded:
                continue
            seen.add(path)
            pending.extend(child for child, optional in edges[path] if not required_only or not optional)
        return seen

    required = reachable(required_only=True)
    if reachable() != set(rows):
        raise InstallError("npm lock contains unreachable dependencies")
    if any(bool(lock["packages"][path].get("optional")) != (path not in required) for path in rows):
        raise InstallError("npm optional flag differs from required dependency reachability")
    if incompatible & required:
        raise InstallError("required npm dependency is incompatible with selected target")
    excluded = {path: "platform_mismatch" for path in incompatible}
    pending = list(excluded)
    while pending:
        for parent, optional in incoming[pending.pop()]:
            if not optional and parent not in excluded:
                excluded[parent] = "required_dependency_platform_mismatch"
                pending.append(parent)
    if set(excluded) & required:
        raise InstallError("platform exclusion would remove a required npm dependency")
    selected = reachable(excluded)
    for path in rows.keys() - selected - excluded.keys():
        excluded[path] = "only_reachable_from_excluded_optional_branch"
    return {**closure, "rows": {path: row for path, row in rows.items() if path in selected},
            "paths": {path + "/" + name for path in selected for name in rows[path][1]["files"]},
            "npm_target": target, "excluded_optional_packages": dict(sorted(excluded.items()))}


NODE_BUILD_PLAN = "closure/node-build-plan"
NODE_BUILD_PROFILE_LIMIT = 65536
NODE_BUILD_PLAN_LIMIT = 32768


def node_build_text(value, limit=4096):
    if not isinstance(value, str) or not 1 <= len(value.encode("utf-8")) <= limit or "\0" in value:
        raise InstallError("bounded literal build text required")
    return value


def node_build_id(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}", value):
        raise InstallError("invalid build tool or input ID")
    return value


def node_build_plan(raw, closure):
    """Validate declared inputs/outputs against original archives, without execution."""
    if len(raw) > NODE_BUILD_PLAN_LIMIT:
        raise InstallError("build plan exceeds document limit")
    value = native_keys(unique_json(raw), {"schema_version", "target", "inputs", "steps", "outputs"})
    if type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise InstallError("unsupported build plan")
    checked_node_native_target(value["target"])
    if ((closure.get("npm_target") and closure["npm_target"] != value["target"])
            or closure.get("native") and closure["native"]["summary"]["target"] != value["target"]):
        raise InstallError("build and npm targets differ")
    originals = {root + "/" + name: data for root, (_, archive) in closure["rows"].items()
                 for name, data in archive["files"].items()}
    inputs, steps, outputs = value["inputs"], value["steps"], value["outputs"]
    if (not isinstance(inputs, list) or not 1 <= len(inputs) <= 256
            or not isinstance(steps, list) or not 1 <= len(steps) <= 16
            or not isinstance(outputs, list) or not 1 <= len(outputs) <= 64):
        raise InstallError("build input, command or output count exceeds limit")
    for row in inputs:
        native_keys(row, {"source", "path"})
        if relative(row["path"])[0] == "tmp":
            raise InstallError("build input uses reserved temporary directory")
        source = row["source"]
        if not isinstance(source, dict) or len(source) != 1:
            raise InstallError("one exact build input source required")
        if "npm" in source:
            if source["npm"] not in originals:
                raise InstallError("build input is not an original retained npm file")
        elif "toolchain" in source:
            node_build_id(source["toolchain"])
        else:
            raise InstallError("unsupported build input source")
    checked_paths([row["path"] for row in inputs])
    for step in steps:
        native_keys(step, {"executable", "arguments"})
        node_build_id(step["executable"])
        arguments = step["arguments"]
        if not isinstance(arguments, list) or len(arguments) > 256:
            raise InstallError("build argument count exceeds limit")
        for argument in arguments:
            if not isinstance(argument, str) or len(argument.encode("utf-8")) > 4096 or "\0" in argument:
                raise InstallError("bounded literal build argument required")
    if len(encoded(steps)) > NODE_BUILD_PLAN_LIMIT:
        raise InstallError("build commands exceed limit")
    destinations, additions = [], []
    for row in outputs:
        native_keys(row, {"path", "destination", "replaces_sha256"})
        if relative(row["path"])[0] == "tmp":
            raise InstallError("build output uses reserved temporary directory")
        relative(row["destination"])
        destination = row["destination"]
        if not any(destination.startswith(root + "/") for root in closure["rows"]):
            raise InstallError("generated output must belong to a selected npm package")
        original = originals.get(destination)
        if row["replaces_sha256"] is None:
            if destination in closure["paths"]:
                raise InstallError("generated output would overwrite retained payload")
            additions.append(destination)
        elif original is None or native_hash(row["replaces_sha256"]) != sha(original):
            raise InstallError("replacement requires exact original archive file hash")
        destinations.append(destination)
    checked_paths([row["path"] for row in outputs])
    checked_paths(destinations)
    checked_paths([*closure["paths"], *additions])
    # An input is never implicitly replaced inside the compiler workspace.
    checked_paths([row["path"] for row in inputs] + [row["path"] for row in outputs])
    return {"plan": value, "sha256": sha(raw), "originals": originals}


def node_build_profile(args):
    """Observe host-selected file identities; do not execute or infer tool closure."""
    name, expected = getattr(args, "node_build_profile", None), getattr(args, "node_build_profile_sha256", None)
    if name is None or expected is None:
        raise InstallError("selected Node build requires an explicit host profile")
    path = Path(name)
    if not path.is_absolute() or str(path) != name:
        raise InstallError("build profile requires an absolute file")
    raw = read(path.parent, path.name, NODE_BUILD_PROFILE_LIMIT)
    if sha(raw) != native_hash(expected):
        raise InstallError("build profile changed")
    value = native_keys(unique_json(raw), {"schema_version", "scope", "target", "executables", "files", "environment", "limits"})
    if type(value["schema_version"]) is not int or value["schema_version"] != 1 or value["scope"] != "selected_node_build":
        raise InstallError("unsupported build host profile")
    checked_node_native_target(value["target"])
    observed, payloads, total = {}, {}, 0
    for key, maximum in (("executables", 16), ("files", 256)):
        rows = value[key]
        if not isinstance(rows, dict) or not (1 if key == "executables" else 0) <= len(rows) <= maximum:
            raise InstallError("build toolchain inventory exceeds limit")
        observed[key], payloads[key] = {}, {}
        for identifier, row in rows.items():
            node_build_id(identifier)
            native_keys(row, {"path", "sha256"})
            path = Path(node_build_text(row["path"]))
            if not path.is_absolute() or str(path) != row["path"]:
                raise InstallError("toolchain requires an absolute path")
            data = read(path.parent, path.name, MAX_TOTAL if key == "executables" else MAX_FILE)
            total += len(data)
            if total > MAX_TOTAL or sha(data) != native_hash(row["sha256"]):
                raise InstallError("toolchain file changed or exceeds limit")
            if key == "executables" and (not os.access(path, os.X_OK) or executable(str(path)) != str(path)):
                raise InstallError("toolchain executable must be an exact canonical executable")
            observed[key][identifier] = {**row, "bytes": len(data)}
            payloads[key][identifier] = data
    environment = value["environment"]
    if (not isinstance(environment, dict) or len(environment) > 32
            or any(not isinstance(key, str) or not re.fullmatch(r"[A-Z_][A-Z0-9_]{0,63}", key)
                   or key == "TMPDIR" for key in environment)):
        raise InstallError("invalid fixed build environment")
    for item in environment.values():
        if not isinstance(item, str) or len(item.encode("utf-8")) > 4096 or "\0" in item:
            raise InstallError("invalid fixed build environment value")
    limits = native_keys(value["limits"], {"timeout_seconds", "file_bytes", "total_output_bytes"})
    for key, maximum in (("timeout_seconds", 3600), ("file_bytes", MAX_FILE), ("total_output_bytes", MAX_TOTAL)):
        if type(limits[key]) is not int or not 1 <= limits[key] <= maximum:
            raise InstallError("invalid build execution limit")
    if limits["file_bytes"] > limits["total_output_bytes"]:
        raise InstallError("build file limit exceeds total output limit")
    return {"value": value, "sha256": sha(raw), "observed": observed, "payloads": payloads}


def review_node_build(closure, args):
    """The same parser serves review, execution and inert existing-file readback."""
    build = closure.get("build")
    if build is None:
        raise InstallError("explicit build plan required")
    profile = node_build_profile(args)
    value, plan = profile["value"], build["plan"]
    if value["target"] != plan["target"] or node_runtime_hash(executable(args.node)) != plan["target"]["node_sha256"]:
        raise InstallError("build target differs from selected host/runtime")
    inputs = []
    for row in plan["inputs"]:
        source = row["source"]
        if "npm" in source:
            data = build["originals"][source["npm"]]
        else:
            if source["toolchain"] not in profile["payloads"]["files"]:
                raise InstallError("build input is not selected by the host")
            data = profile["payloads"]["files"][source["toolchain"]]
        inputs.append({**row, "sha256": sha(data), "bytes": len(data)})
    commands = []
    for step in plan["steps"]:
        tool = profile["observed"]["executables"].get(step["executable"])
        if tool is None:
            raise InstallError("build executable is not selected by the host")
        commands.append({"executable": step["executable"], "argv": [tool["path"], *step["arguments"]],
                         "executable_sha256": tool["sha256"]})
    return {"schema_version": 1, "scope": "selected_node_build", "plan_sha256": build["sha256"],
            "profile_sha256": profile["sha256"], "target": plan["target"],
            "tools": profile["observed"], "inputs": inputs, "commands": commands,
            "outputs": plan["outputs"], "environment": value["environment"], "limits": value["limits"],
            "working_directory": "separate_installation_build_work", "temporary_directory": "build_work/tmp",
            "custody": "Trusted local software effect; not sandboxed or a complete dynamic toolchain closure."}


def describe_node_build(args):
    profile = node_build_profile(args)
    if node_runtime_hash(executable(args.node)) != profile["value"]["target"]["node_sha256"]:
        raise InstallError("build profile differs from selected Node")
    return {"schema_version": 1, "scope": "selected_node_build_planning", "profile_sha256": profile["sha256"],
            "target": profile["value"]["target"], "limits": profile["value"]["limits"],
            "executables": {key: {"sha256": row["sha256"], "name": Path(row["path"]).name}
                            for key, row in profile["observed"]["executables"].items()},
            "files": {key: {"sha256": row["sha256"], "bytes": row["bytes"], "name": Path(row["path"]).name}
                      for key, row in profile["observed"]["files"].items()},
            "environment_keys": sorted(profile["value"]["environment"]),
            "notice": "Host-selected inputs for an explicit reviewed build plan; no build or compatibility proof."}


def node_build_review_bundle(args):
    """Read only the already checked snapshot; the caller owns canonical validation."""
    bundle = Path(args.review_node_build).resolve(strict=True)
    plan = unique_json(read(bundle, "plan.json", MAX_CONFIGURATION * 16))
    files = {"artifact.tgz": read(bundle, "artifact.tgz")}
    for key, expected in plan["closure"].items():
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", key):
            raise InstallError("invalid closure ID")
        raw = read(bundle, "closure/" + key)
        if sha(raw) != native_hash(expected):
            raise InstallError("reviewed closure changed")
        files["closure/" + key] = raw
    if sha(files["artifact.tgz"]) != plan["artifact_digest"]:
        raise InstallError("reviewed artifact changed")
    return review_node_build(selected_npm_closure(files, plan), args)


def node_build_write(root, path, raw, *, replace=False):
    parts = relative(path)
    parent = root
    for part in parts[:-1]:
        parent = parent / part
        if not parent.exists():
            parent.mkdir(mode=0o700)
        fd = directory(parent)
        os.close(fd)
    flags = os.O_WRONLY | os.O_NOFOLLOW | (os.O_TRUNC if replace else os.O_CREAT | os.O_EXCL)
    fd = os.open(parent / parts[-1], flags, 0o600)
    with os.fdopen(fd, "wb") as output:
        output.write(raw)


def execute_node_build(destination, closure, args, review, deadline):
    """Retain original files, execute literal argv, then publish declared additions."""
    if review_node_build(closure, args) != review:
        raise InstallError("reviewed build inputs or tools changed")
    profile, build = node_build_profile(args), closure["build"]
    work = destination / "build-work"
    work.mkdir(mode=0o700)
    (work / "tmp").mkdir(mode=0o700)
    for row in build["plan"]["inputs"]:
        source = row["source"]
        data = (build["originals"][source["npm"]] if "npm" in source
                else profile["payloads"]["files"][source["toolchain"]])
        node_build_write(work, row["path"], data)
    environment = {**review["environment"], "TMPDIR": str(work / "tmp")}
    steps = []
    for index, command in enumerate(review["commands"]):
        if review_node_build(closure, args) != review:
            raise InstallError("selected toolchain changed before build command")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise InstallError("selected build deadline expired")
        # Compiler streams do not enter the model response or an unbounded log.
        # run returns only after this exact direct child has exited successfully.
        run(command["argv"], work, environment, "build-" + str(index), timeout=remaining, discard_output=True)
        steps.append({"index": index, "exit_code": 0})
    if review_node_build(closure, args) != review:
        raise InstallError("selected toolchain changed during build")
    for row in review["inputs"]:
        if sha(read(work, row["path"])) != row["sha256"]:
            raise InstallError("build modified an original working input")
    # A compiler is trusted local code, not a sandbox. Detect changes to owned
    # original npm bytes before publishing any declared generated output.
    verify_selected_npm(destination, closure)
    outputs, total = {}, 0
    for row in review["outputs"]:
        raw = read(work, row["path"], review["limits"]["file_bytes"])
        total += len(raw)
        if total > review["limits"]["total_output_bytes"]:
            raise InstallError("generated output exceeds selected limit")
        outputs[row["destination"]] = {"sha256": sha(raw), "bytes": len(raw)}
    for row in review["outputs"]:
        raw = read(work, row["path"], review["limits"]["file_bytes"])
        if {"sha256": sha(raw), "bytes": len(raw)} != outputs[row["destination"]]:
            raise InstallError("generated output changed during publication")
        if row["replaces_sha256"] is not None and sha(read(destination, row["destination"])) != row["replaces_sha256"]:
            raise InstallError("original replacement payload changed")
        node_build_write(destination, row["destination"], raw, replace=row["replaces_sha256"] is not None)
    return {"review": review, "steps": steps, "outputs": outputs}


def checked_node_build_receipt(closure, args, value):
    native_keys(value, {"review", "steps", "outputs"})
    review = review_node_build(closure, args)
    if (value["review"] != review or value["steps"] != [
            {"index": index, "exit_code": 0} for index in range(len(review["commands"]))]
            or not isinstance(value["outputs"], dict)
            or set(value["outputs"]) != {row["destination"] for row in review["outputs"]}):
        raise InstallError("generated output receipt differs from original build selection")
    total = 0
    for row in value["outputs"].values():
        native_keys(row, {"sha256", "bytes"})
        native_hash(row["sha256"])
        if type(row["bytes"]) is not int or not 0 <= row["bytes"] <= review["limits"]["file_bytes"]:
            raise InstallError("generated output size exceeds limit")
        total += row["bytes"]
    if total > review["limits"]["total_output_bytes"]:
        raise InstallError("generated output total exceeds limit")
    return value


def npm_archive_closure(files: dict[str, bytes]) -> dict:
    """Bind npm's concrete lock to retained archives without choosing an entrypoint."""
    build_raw = files.get(NODE_BUILD_PLAN)
    # The authored plan is not part of the acquisition/native source receipt.
    # Remove this exact reserved input before checking that unchanged graph.
    files = {key: raw for key, raw in files.items() if key != NODE_BUILD_PLAN}
    files, native = node_native_inputs(files)
    project = unique_json(files["closure/package-json"])
    lock = unique_json(files["closure/package-lock"])
    target_raw = files.get(NPM_TARGET_INPUT)
    if target_raw is not None and len(target_raw) > MAX_CONFIGURATION:
        raise InstallError("npm target exceeds document limit")
    target = checked_node_native_target(unique_json(target_raw)) if target_raw is not None else None
    component = npm_archive(files["artifact.tgz"])
    selected = component["name"]
    expected = {selected: "file:./artifact.tgz"}
    if (set(project) - {"name", "version", "private", "dependencies"}
            or not isinstance(project.get("name"), str) or not project["name"]
            or not isinstance(project.get("version"), str) or not project["version"]
            or project.get("private") is not True or project.get("dependencies") != expected
            or lock.get("lockfileVersion") != 3 or not isinstance(lock.get("packages"), dict)
            or lock.get("name") != project.get("name") or lock.get("version") != project.get("version")
            or not isinstance(lock["packages"].get(""), dict)
            or lock["packages"].get("", {}).get("dependencies") != expected):
        raise InstallError("npm project must select only the retained local artifact")
    archives = {component["integrity"]: ("artifact.tgz", component)}
    total = component["expanded_bytes"]
    members = len(component["files"])
    for path, raw in files.items():
        if not path.startswith("closure/") or path in ("closure/package-json", "closure/package-lock", NPM_TARGET_INPUT):
            continue
        archive = npm_archive(raw)
        total += archive["expanded_bytes"]
        members += len(archive["files"])
        if total > MAX_TOTAL or members > MAX_NPM_MEMBERS or archive["integrity"] in archives:
            raise InstallError("duplicate archive or excessive npm closure")
        archives[archive["integrity"]] = (path, archive)
    rows, used, aliases = {}, set(), set()
    realized_members, realized_bytes = 0, 0
    component_path = "node_modules/" + selected
    for path, row in lock["packages"].items():
        if not isinstance(row, dict):
            raise InstallError("npm lock row must be an object")
        if not path:
            if any(row.get(key) for key in ("devDependencies", "optionalDependencies", "workspaces", "link")):
                raise InstallError("unsupported npm root dependencies")
            continue
        package = npm_location(path)
        if path.casefold() in aliases or any(row.get(key) for key in ("link", "inBundle", "dev")):
            raise InstallError("unsupported npm dependency or path alias")
        aliases.add(path.casefold())
        resolved = row.get("resolved", "")
        if not isinstance(resolved, str):
            raise InstallError("npm resolved location must be text")
        if path == component_path:
            if resolved != "file:artifact.tgz":
                raise InstallError("component lock must use retained artifact")
            integrity = component["integrity"]
            if row.get("integrity") != integrity:
                raise InstallError("component lock integrity mismatch")
        else:
            from urllib.parse import urlsplit
            url = urlsplit(resolved)
            if url.scheme != "https" or not url.hostname or url.username or url.password or url.query or url.fragment:
                raise InstallError("only pinned HTTPS registry dependency rows are supported")
            integrity = row.get("integrity", "")
        if integrity not in archives:
            raise InstallError("npm lock dependency missing from retained closure")
        archive_path, archive = archives[integrity]
        if archive["name"] != package or archive["version"] != row.get("version"):
            raise InstallError("npm archive name/version differs from exact lock location")
        realized_members += len(archive["files"])
        realized_bytes += archive["expanded_bytes"]
        if realized_members > MAX_NPM_MEMBERS or realized_bytes > MAX_TOTAL:
            raise InstallError("realized npm file graph exceeds limits")
        rows[path] = (archive_path, archive)
        used.add(integrity)
    if component_path not in rows or used != set(archives):
        raise InstallError("npm closure differs from selected lock")
    # Catch case aliases and file/directory conflicts before npm touches disk.
    all_paths = {path + "/" + file for path, (_, archive) in rows.items() for file in archive["files"]}
    folded = {path.casefold() for path in all_paths}
    if len(folded) != len(all_paths) or any(str(parent).casefold() in folded
            for path in all_paths for parent in PurePosixPath(path).parents):
        raise InstallError("npm closure has colliding file paths")
    spellings = {}
    for path in all_paths:
        for prefix in [PurePosixPath(path), *PurePosixPath(path).parents]:
            value = str(prefix)
            if spellings.setdefault(value.casefold(), value) != value:
                raise InstallError("npm closure has case-aliased directories")
    closure = {"rows": rows, "archives": archives, "component_path": component_path, "paths": all_paths}
    conditional = npm_conditional(lock) or any(npm_conditional({"packages": {"":
        unique_json(archive["files"]["package.json"])}}) for _, archive in archives.values())
    if conditional and target is None:
        raise InstallError("conditional npm graph requires a bound target")
    if target is not None:
        closure = npm_target_graph(closure, lock, target)
        if native and native["manifest"]["target"] != target:
            raise InstallError("npm and native payload targets differ")
    closure = compose_node_native_payloads(closure, files, native) if native else closure
    if build_raw is not None:
        closure["build"] = node_build_plan(build_raw, closure)
    return closure


def selected_npm_closure(files: dict[str, bytes], plan: dict) -> dict:
    """Validate the agent-selected entrypoint against the canonical archive graph."""
    closure = npm_archive_closure(files)
    component = closure["rows"][closure["component_path"]][1]
    entry = plan["entrypoint"]["executable"]
    relative(entry)
    generated = {row["destination"] for row in closure.get("build", {}).get("plan", {}).get("outputs", [])}
    if (not entry.endswith((".js", ".mjs", ".cjs"))
            or entry not in component["files"] and closure["component_path"] + "/" + entry not in generated):
        raise InstallError("selected Node entrypoint must be retained or explicitly generated JavaScript")
    return {**closure, "entrypoint": entry}


def verify_selected_npm(destination: Path, closure: dict, generated: dict | None = None) -> dict:
    """Require exact payload bytes and no extra executable files or symlinks."""
    graph = {}
    generated = generated or {}
    for path, (_, archive) in closure["rows"].items():
        for filename, raw in archive["files"].items():
            if path + "/" + filename not in generated and read(destination, path + "/" + filename) != raw:
                raise InstallError("installed npm payload differs from retained archive")
        graph[path] = {"name": archive["name"], "version": archive["version"],
                       "integrity": archive["integrity"], "files": len(archive["files"])}
    if native := closure.get("native"):
        for path, raw in native["files"].items():
            if read(destination, path) != raw:
                raise InstallError("installed native payload differs from selected asset")
        for package, hashes in native["per_package"].items():
            graph[package]["native_payloads"] = hashes
            graph[package]["files"] += len(hashes)
    for path, expected in generated.items():
        raw = read(destination, path)
        if {"sha256": sha(raw), "bytes": len(raw)} != expected:
            raise InstallError("installed generated output differs from its receipt")
        package = max((root for root in graph if path.startswith(root + "/")), key=len)
        graph[package].setdefault("generated_outputs", {})[path] = expected
        graph[package]["files"] += path not in closure["paths"]
    actual = set()
    for root, dirs, filenames in os.walk(destination / "node_modules", followlinks=False):
        for child in dirs + filenames:
            if (Path(root) / child).is_symlink():
                raise InstallError("unexpected installed npm symlink")
        actual.update(str((Path(root) / file).relative_to(destination)) for file in filenames)
    if actual - {"node_modules/.package-lock.json"} != closure["paths"] | set(generated):
        raise InstallError("installed npm file graph differs from closure")
    return graph


def node_runtime_hash(runtime: str) -> str:
    path = Path(runtime)
    return sha(read(path.parent, path.name, MAX_TOTAL))


def observe_node_native_target(runtime: str, destination: Path, env: dict) -> dict:
    before = node_runtime_hash(runtime)
    raw = run([runtime, "-p", NODE_NATIVE_TARGET_EXPRESSION], destination, env, "node-native-target", timeout=30)
    if len(raw) > 65536:
        raise InstallError("native target observation exceeds output limit")
    value = native_keys(unique_json(raw), {"node_version", "node_abi", "platform", "arch", "libc"})
    target = checked_node_native_target({**value, "node_sha256": before})
    if node_runtime_hash(runtime) != before:
        raise InstallError("selected native Node executable changed during observation")
    return target


def install_node_native_payloads(destination: Path, native: dict) -> None:
    """Create only reviewed additions; no overwrite, scripts, imports or repair."""
    for path, raw in native["files"].items():
        handle = directory(destination)
        try:
            parts = relative(path)
            for part in parts[:-1]:
                try:
                    os.mkdir(part, 0o700, dir_fd=handle)
                except FileExistsError:
                    pass
                child = os.open(part, DIR_FLAGS, dir_fd=handle)
                os.close(handle)
                handle = child
            fd = os.open(parts[-1], os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=handle)
            with os.fdopen(fd, "wb") as output:
                output.write(raw)
        finally:
            os.close(handle)


LOCAL_SOFTWARE_DOCUMENT = "local_software_declaration"
SELECTED_IDENTITY_FIELDS = {"document_kind", "declaration_digest", "manifest_digest"}


def selected_document(args):
    kind = getattr(args, "document_kind", None)
    if kind is None:
        return "--selection", "activation.json", "manifest_digest"
    if kind != LOCAL_SOFTWARE_DOCUMENT or not (getattr(args, "selected_python", None) or getattr(args, "selected_node", None)):
        raise InstallError("document kind requires a selected Python or Node software declaration")
    return "--declaration", "declaration.json", "declaration_digest"


def selected_identity(value, args):
    """Bind the canonical plan and receipt to the explicitly selected form."""
    _, _, pin = selected_document(args)
    expected = {pin: value.get(pin)} if isinstance(value, dict) else {}
    if pin == "declaration_digest":
        expected["document_kind"] = LOCAL_SOFTWARE_DOCUMENT
    if (not isinstance(value, dict) or not isinstance(value.get(pin), str)
            or not re.fullmatch(r"[a-f0-9]{64}", value[pin])
            or {key: value[key] for key in SELECTED_IDENTITY_FIELDS if key in value} != expected):
        raise InstallError("selected document identity differs from its explicit form")
    return expected


def install_selected_node(args: argparse.Namespace) -> dict:
    """An explicit trusted-local effect; this is not a native agent grant."""
    bundle = Path(args.selected_node).resolve(strict=True)
    destination = new_destination(args.destination, bundle)
    if not re.fullmatch(r"[a-f0-9]{64}", args.selection_sha256):
        raise InstallError("expected raw selection SHA-256 required")
    document_flag, document_name, _ = selected_document(args)
    selection = read(bundle, document_name, 1024 * 1024)
    if sha(selection) != args.selection_sha256:
        raise InstallError("selected document differs from operator-selected bytes")
    package = read(bundle, "package.json", 1024 * 1024)
    runtime, npm, validator = executable(args.node), executable(args.npm_cli), executable(args.validator)
    parent = directory(destination.parent)
    try:
        os.mkdir(destination.name, 0o700, dir_fd=parent)
    finally:
        os.close(parent)
    frozen = destination / "inputs"
    frozen.mkdir(mode=0o700)
    (frozen / "closure").mkdir(mode=0o700)
    files = {"package.json": package, document_name: selection}
    for filename, raw in files.items():
        (frozen / filename).write_bytes(raw)
    (destination / "tmp").mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(destination / "tmp")}
    # The canonical executable validates this explicit form's exact frozen
    # documents. Never deserialize a claim that a prior plan was validated.
    plan = unique_json(run([validator, "--package", str(frozen / "package.json"),
                           document_flag, str(frozen / document_name), "--component", args.component],
                          destination, env, "validate-selection"))
    if plan["schema_version"] != 1 or plan["component"] != args.component:
        raise InstallError("unsupported validator projection")
    identity = selected_identity(plan, args)
    expected = {"artifact.tgz": plan["artifact_digest"],
                **{"closure/" + key: digest for key, digest in plan["closure"].items()}}
    total = 0
    for filename, digest in expected.items():
        raw = read(bundle, filename)
        total += len(raw)
        if total > MAX_TOTAL or sha(raw) != digest:
            raise InstallError("selected artifact closure differs from retained bytes")
        files[filename] = raw
        (frozen / filename).write_bytes(raw)
    closure = selected_npm_closure(files, plan)
    build_review = review_node_build(closure, args) if closure.get("build") else None
    build_deadline = time.monotonic() + build_review["limits"]["timeout_seconds"] if build_review else None
    if build_review:
        if unique_json(read(bundle, "build-review.json", MAX_CONFIGURATION * 32)) != build_review:
            raise InstallError("build differs from the reviewed software effect")
    observed = json.loads(run([runtime, "-p", "JSON.stringify([process.version,process.platform,process.arch])"],
                              destination, env, "node-runtime"))
    native = closure.get("native")
    if native or closure.get("npm_target") or build_review:
        bound_target = (build_review["target"] if build_review else
                        native["summary"]["target"] if native else closure["npm_target"])
        if (observe_node_native_target(runtime, destination, env) != bound_target
                or observed != [bound_target["node_version"], bound_target["platform"], bound_target["arch"]]):
            raise InstallError("native payload target differs from selected Node runtime")
    env = npm_environment(destination, runtime, env)
    env.update({"npm_config_engine_strict": "true", "npm_config_bin_links": "false"})
    for target, source in (("package.json", "closure/package-json"), ("package-lock.json", "closure/package-lock"),
                           ("artifact.tgz", "artifact.tgz")):
        (destination / target).write_bytes(files[source])
    command = [runtime, npm, "--prefix", str(destination)]
    npm_inputs = destination / "npm-tarballs"
    npm_inputs.mkdir(mode=0o700)
    for index, (path, _) in enumerate(closure["archives"].values()):
        # npm's local spec parser requires a tarball suffix. Opaque manifest
        # IDs remain unchanged; these are verified byte-for-byte working copies.
        tarball = npm_inputs / (sha(files[path]) + ".tgz")
        tarball.write_bytes(files[path])
        run(command + ["cache", "add", str(tarball), "--offline"], destination, env, "npm-cache-" + str(index))
    run(command + ["ci", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", "--bin-links=false"],
        destination, env, "npm-install")
    if native:
        original = {key: value for key, value in closure.items() if key != "native"}
        original["paths"] = closure["npm_paths"]
        verify_selected_npm(destination, original)
        install_node_native_payloads(destination, native)
    graph = verify_selected_npm(destination, closure)
    build_receipt = None
    if build_review:
        build_receipt = execute_node_build(destination, closure, args, build_review, build_deadline)
        graph = verify_selected_npm(destination, closure, build_receipt["outputs"])
        for name, original in files.items():
            if read(frozen, name) != original:
                raise InstallError("build modified retained original selection")
        for name, original in (("package.json", files["closure/package-json"]),
                               ("package-lock.json", files["closure/package-lock"]), ("artifact.tgz", files["artifact.tgz"])):
            if read(destination, name) != original:
                raise InstallError("build modified original npm project")
    if (native or closure.get("npm_target") or build_review) and node_runtime_hash(runtime) != bound_target["node_sha256"]:
        raise InstallError("selected native Node executable changed during installation")
    launch = [runtime, str(destination / closure["component_path"] / closure["entrypoint"]),
              *plan["entrypoint"]["arguments"]]
    configuration = {"mcpServers": {plan["mcp_name"]: {"command": launch[0], "args": launch[1:]}}}
    receipt = {"schema_version": 1, "scope": "observed_software_installation_only",
               "selection_sha256": args.selection_sha256, **identity,
               "inputs": {key: sha(raw) for key, raw in sorted(files.items())}, "runtime": observed,
               "installed_graph": graph, "command": launch, "destination": str(destination),
               "mcp_configuration": configuration,
               "launch_environment": "Owned by the consuming host; this configuration does not clear ambient environment or filter tools."}
    if closure.get("npm_target"):
        receipt.update({key: closure[key] for key in ("npm_target", "excluded_optional_packages")})
    if native:
        receipt["native_payloads"] = native["summary"]
    if build_receipt:
        receipt["node_build"] = build_receipt
    # No guessed command is executed against an unknown server. A successful
    # install is not protocol readiness, authorization or business completion.
    ready = destination / "pending-ready"
    ready.mkdir(mode=0o700)
    with (ready / "installed.json").open("xb") as output:
        output.write(encoded(receipt))
    with (ready / "mcp.json").open("xb") as output:
        output.write(encoded(configuration))
    ready.rename(destination / "ready")
    return receipt


def checked_paths(paths: list[str]) -> None:
    """Reject file collisions, including aliases on case-insensitive hosts."""
    folded, spellings = set(), {}
    for path in paths:
        relative(path)
        if path.casefold() in folded:
            raise InstallError("duplicate or aliased wheel path")
        folded.add(path.casefold())
        for prefix in [PurePosixPath(path), *PurePosixPath(path).parents]:
            value = str(prefix)
            if spellings.setdefault(value.casefold(), value) != value:
                raise InstallError("case-aliased wheel directory")
    if any(str(parent).casefold() in folded for path in paths for parent in PurePosixPath(path).parents):
        raise InstallError("wheel file and directory collide")


# Fixed code only. Wheel metadata and caller arguments are separate literal argv.
PYTHON_CONSOLE_LAUNCH = """import importlib, sys
module, attributes, invoked = sys.argv[1:4]
sys.argv = [invoked, *sys.argv[4:]]
target = importlib.import_module(module)
for attribute in attributes.split('.'):
    target = getattr(target, attribute)
sys.exit(target())
"""


def python_script_reference(value: str) -> tuple[str, str, bool]:
    """Parse metadata only; no import, attribute lookup or optional dependency resolution."""
    reference, bracket, extras = value.partition("[")
    if bracket and not re.fullmatch(r"\s*[A-Za-z0-9][A-Za-z0-9_.-]*(?:\s*,\s*[A-Za-z0-9][A-Za-z0-9_.-]*)*\s*\]\s*", extras):
        raise InstallError("unsupported wheel script extras")
    module, colon, attributes = reference.partition(":")
    module, attributes = module.strip(), attributes.strip()
    if (not colon or any(not part.isidentifier() for part in module.split("."))
            or any(not part.isidentifier() for part in attributes.split("."))):
        raise InstallError("unsupported wheel script entry point")
    return module, attributes, bool(bracket)


def selected_wheel(filename: str, raw: bytes, stdlib: set[str]) -> dict:
    """Inspect inert wheel bytes before pip or an installed interpreter runs."""
    pieces = filename.removesuffix(".whl").split("-")
    if (not filename.endswith(".whl") or len(pieces) not in (5, 6)
            or not re.fullmatch(r"[A-Za-z0-9_.+-]+", filename)
            or (len(pieces) == 6 and not re.fullmatch(r"[0-9][A-Za-z0-9_]*", pieces[2]))):
        raise InstallError("original wheel filename required")
    files, total, members = {}, 0, []
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        if len(archive.infolist()) > MAX_NPM_MEMBERS:
            raise InstallError("too many wheel members")
        for item in archive.infolist():
            path = item.filename.rstrip("/") if item.is_dir() else item.filename
            parts = relative(path)
            mode = stat.S_IFMT(item.external_attr >> 16)
            if (item.orig_filename != item.filename or item.flag_bits & 1
                    or item.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)
                    or mode not in (0, stat.S_IFREG, stat.S_IFDIR)
                    or (mode == stat.S_IFDIR) != item.is_dir() and mode != 0):
                raise InstallError("wheel links, encryption and special files are unsupported")
            members.append(path)
            if any(part.endswith(".data") for part in parts):
                raise InstallError("selected wheel profile does not relocate .data payloads")
            if item.is_dir():
                continue
            total += item.file_size
            if item.file_size > MAX_FILE or total > MAX_NPM_EXPANDED:
                raise InstallError("wheel expansion exceeds limit")
            if (path.casefold().endswith((".pth", ".pyc", ".pyo")) or "__pycache__" in [part.casefold() for part in parts]
                    or any(part.split(".")[0].casefold() in ("sitecustomize", "usercustomize") for part in parts)
                    or parts[0].split(".")[0].casefold() in {value.casefold() for value in stdlib} | {"pip", "setuptools", "pkg_resources", "_distutils_hack"}):
                raise InstallError("wheel could replace interpreter or installer startup code")
            try:
                data = archive.read(item)
            except zlib.error as error:
                raise InstallError("invalid wheel compression") from error
            if len(data) != item.file_size:
                raise InstallError("truncated wheel payload")
            files[path] = data
    # Directories may be explicitly represented, so use them only for alias
    # checks; file/directory conflicts are checked against payload paths.
    if len(members) != len(set(path.casefold() for path in members)):
        raise InstallError("duplicate or aliased wheel member")
    spellings = {}
    for path in members:
        for prefix in [PurePosixPath(path), *PurePosixPath(path).parents]:
            value = str(prefix)
            if spellings.setdefault(value.casefold(), value) != value:
                raise InstallError("case-aliased wheel member directory")
    checked_paths(list(files))
    metadata = [path for path in files if path.endswith(".dist-info/METADATA") and path.count("/") == 1]
    if len(metadata) != 1:
        raise InstallError("one wheel distribution required")
    info = email.parser.BytesParser().parsebytes(files[metadata[0]])
    if len(info.get_all("Name", [])) != 1 or len(info.get_all("Version", [])) != 1:
        raise InstallError("unambiguous wheel identity required")
    package, version = name(info["Name"]), info["Version"]
    dist = metadata[0].split("/")[0]
    if (not re.fullmatch(r"[a-z0-9][a-z0-9-]*", package)
            or name(pieces[0]) != package or pieces[1] != version
            or dist != pieces[0] + "-" + pieces[1] + ".dist-info"
            or any("@" in value or "://" in value for value in info.get_all("Requires-Dist", []))
            or any(part.endswith(".dist-info") and part != dist for path in files for part in relative(path))):
        raise InstallError("wheel identity or dependency location differs from supported profile")
    wheel = email.parser.BytesParser().parsebytes(files[dist + "/WHEEL"])
    expected_parts = [set(part.split(".")) for part in pieces[-3:]]
    expected_tags = {(a, b, c) for a in expected_parts[0]
                     for b in expected_parts[1] for c in expected_parts[2]}
    purelib = wheel.get_all("Root-Is-Purelib", [])
    actual_tags = set()
    for tag in wheel.get_all("Tag", []):
        parts = tag.split("-")
        if (len(parts) != 3 or any(not re.fullmatch(r"[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)*", part)
                                   for part in parts)):
            raise InstallError("invalid wheel compatibility tag")
        fields = [set(part.split(".")) for part in parts]
        # Expand compressed tags only within the filename's exact tag set.
        if any(not field <= expected for field, expected in zip(fields, expected_parts)):
            raise InstallError("wheel format or tags differ from filename")
        actual_tags.update((a, b, c) for a in fields[0] for b in fields[1] for c in fields[2])
    if (wheel.get_all("Wheel-Version") != ["1.0"]
            or len(purelib) != 1 or purelib[0].lower() not in ("true", "false")
            or actual_tags != expected_tags):
        raise InstallError("wheel format or tags differ from filename")
    record = dist + "/RECORD"
    rows = {}
    for row in csv.reader(io.StringIO(files[record].decode("utf-8"), newline="")):
        if len(row) != 3 or row[0] in rows:
            raise InstallError("invalid wheel RECORD")
        rows[row[0]] = row[1:]
    if set(rows) != set(files) or rows[record] != ["", ""]:
        raise InstallError("wheel RECORD does not cover exact payload")
    for path, (digest, size) in rows.items():
        if path == record:
            continue
        algorithm, separator, expected = digest.partition("=")
        if algorithm not in ("sha256", "sha384", "sha512") or not separator:
            raise InstallError("wheel RECORD requires strong content hashes")
        actual = base64.urlsafe_b64encode(hashlib.new(algorithm, files[path]).digest()).rstrip(b"=").decode()
        if expected != actual or size != str(len(files[path])):
            raise InstallError("wheel RECORD payload mismatch")
    if any(path in files for path in (dist + "/INSTALLER", dist + "/REQUESTED", dist + "/direct_url.json")):
        raise InstallError("wheel supplies installer-owned metadata")
    scripts, console_scripts = {}, {}
    if dist + "/entry_points.txt" in files:
        entries = configparser.ConfigParser(interpolation=None, strict=True, delimiters=("=",))
        entries.optionxform = str
        entries.read_string(files[dist + "/entry_points.txt"].decode("utf-8"))
        if entries.defaults():
            raise InstallError("entry point defaults are unsupported")
        for group in ("console_scripts", "gui_scripts"):
            if not entries.has_section(group):
                continue
            for script, target in entries.items(group):
                if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", script) or script in scripts:
                    raise InstallError("unsupported wheel script entry point")
                reference = python_script_reference(target)
                scripts[script] = target
                if group == "console_scripts":
                    console_scripts[script] = reference
    return {"name": package, "version": version, "filename": filename, "sha256": sha(raw),
            "files": files, "dist_info": dist, "scripts": scripts, "console_scripts": console_scripts, "expanded_bytes": total}


def selected_python_closure(files: dict[str, bytes], plan: dict, stdlib: set[str], extensions: list[str]) -> dict:
    pins = {}
    for line in files["closure/requirements"].decode("ascii").splitlines():
        match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9_.+!-]+) --hash=sha256:([a-f0-9]{64})", line)
        if not match or name(match[1]) in pins:
            raise InstallError("wheel requirements must be unique exact pins and hashes")
        pins[name(match[1])] = (match[2], match[3])
    wheels, members, expanded = [], 0, 0
    sources = [(plan["artifact_filename"], files["artifact.whl"])]
    sources.extend((path.removeprefix("closure/"), data) for path, data in files.items()
                   if path.startswith("closure/") and path != "closure/requirements")
    for filename, data in sources:
        wheel = selected_wheel(filename, data, stdlib)
        members += len(wheel["files"])
        expanded += wheel["expanded_bytes"]
        if members > MAX_NPM_MEMBERS or expanded > MAX_TOTAL:
            raise InstallError("wheel closure expansion exceeds limit")
        wheels.append(wheel)
    identities = {row["name"]: (row["version"], row["sha256"]) for row in wheels}
    if not pins or len(identities) != len(wheels) or identities != pins:
        raise InstallError("selected wheel closure differs from exact requirements")
    checked_paths([path for wheel in wheels for path in wheel["files"]])
    checked_paths([script for wheel in wheels for script in wheel["scripts"]])
    primary = wheels[0]
    entry = plan["entrypoint"]["executable"]
    parts = relative(entry)
    console_name = plan["entrypoint"].get("python_console_script")
    if console_name is not None and (not isinstance(console_name, str)
            or not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", console_name)):
        raise InstallError("invalid explicit Python console script name")
    package_entry = console_name is not None and parts[-1] == "__init__.py"
    module_parts = parts[:-1] if package_entry else [*parts[:-1], parts[-1].removesuffix(".py")]
    if (not entry.endswith(".py") or not module_parts
            or parts[-1] == "__init__.py" and not package_entry or entry not in primary["files"]
            or any(not re.fullmatch(r"[A-Za-z_]\w*", part, re.ASCII) for part in module_parts)
            or plan["module"] != ".".join(module_parts)):
        raise InstallError("entrypoint must be a Python module owned by the primary wheel")
    namespace = module_parts[0]
    for index in range(1, len(parts)):
        parent = "/".join(parts[:index])
        if parent + "/__init__.py" not in primary["files"] or parent + ".py" in primary["files"]:
            raise InstallError("module parents require unambiguous primary-owned packages")
        if any(parent + suffix in primary["files"] or parent + "/__init__" + suffix in primary["files"]
               for suffix in extensions):
            raise InstallError("extension module shadows selected package source")
    stem = entry.removesuffix(".py")
    if (any(stem + suffix in primary["files"] for suffix in extensions)
            or not package_entry and any(stem + "/__init__" + suffix in primary["files"] for suffix in [".py", *extensions])):
        raise InstallError("package or extension shadows selected module source")
    if any(path.split("/")[0].split(".")[0] == namespace for wheel in wheels[1:] for path in wheel["files"]):
        raise InstallError("selected module namespace has multiple wheel owners")
    console = None
    if console_name is not None:
        reference = primary["console_scripts"].get(console_name)
        if reference is None:
            raise InstallError("console script must be declared by the primary wheel in console_scripts")
        module, attributes, extras = reference
        if extras:
            raise InstallError("selected console script extras are unsupported; no requirements are ignored")
        if module != plan["module"]:
            raise InstallError("console script module differs from the selected primary source")
        console = {"name": console_name, "module": module, "attributes": attributes}
    return {"wheels": wheels, "versions": {key: value[0] for key, value in identities.items()},
            "module": plan["module"], "console_script": console}


def selected_python_launch(python: str, plan: dict, closure: dict) -> list[str]:
    console = closure["console_script"]
    if console is None:
        return [python, "-I", "-B", "-m", closure["module"], *plan["entrypoint"]["arguments"]]
    return [python, "-I", "-B", "-c", PYTHON_CONSOLE_LAUNCH,
            console["module"], console["attributes"], str(Path(python).parent / console["name"]),
            *plan["entrypoint"]["arguments"]]


def snapshot_tree(root: Path) -> dict[str, tuple[str, str]]:
    result = {}
    for directory_path, dirs, filenames in os.walk(root, followlinks=False):
        for child in dirs + filenames:
            path = Path(directory_path) / child
            key = str(path.relative_to(root))
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                result[key] = ("symlink", os.readlink(path))
            elif stat.S_ISREG(mode):
                result[key] = ("file", sha(read(root, key)))
            elif not stat.S_ISDIR(mode):
                raise InstallError("unexpected environment special file")
    return result


def verify_selected_python(venv: Path, baseline: dict, closure: dict) -> dict:
    """Check retained bytes and the exact file delta, excluding pip-owned bodies."""
    site = "lib/python3.11/site-packages/"
    expected, generated, graph = {}, set(), {}
    for wheel in closure["wheels"]:
        dist = site + wheel["dist_info"]
        for path, raw in wheel["files"].items():
            if path == wheel["dist_info"] + "/RECORD":
                continue
            expected[site + path] = ("file", sha(raw))
        generated.update((dist + "/RECORD", dist + "/INSTALLER", dist + "/REQUESTED"))
        generated.update("bin/" + script for script in wheel["scripts"])
        if read(venv, dist + "/INSTALLER") != b"pip\n" or read(venv, dist + "/REQUESTED") != b"":
            raise InstallError("unexpected pip metadata")
        graph[wheel["name"]] = {"version": wheel["version"], "wheel_sha256": wheel["sha256"],
                               "payloads": len(wheel["files"]) - 1}
    actual = snapshot_tree(venv)
    if set(actual) != set(baseline) | set(expected) | generated:
        raise InstallError("installed Python file graph differs from selected closure")
    if any(actual[path] != value for path, value in {**baseline, **expected}.items()):
        raise InstallError("installed Python or bootstrap bytes differ")
    if any(actual[path][0] != "file" for path in generated):
        raise InstallError("unexpected installer-generated link")
    return {"wheels": graph, "installer_generated": {path: actual[path][1] for path in sorted(generated)},
            "verified_payload_files": len(expected), "preserved_bootstrap_files": len(baseline)}


def selected_python_target(value: str | None) -> str:
    target = "macos-arm64" if value is None else value
    if target not in SELECTED_PYTHON_TARGETS:
        raise InstallError("unsupported selected Python target")
    return target


def install_selected_python(args: argparse.Namespace) -> dict:
    """Realize one concrete selection; native authority and launch stay outside."""
    target = selected_python_target(getattr(args, "python_target", None))
    bundle = Path(args.selected_python).resolve(strict=True)
    destination = new_destination(args.destination, bundle)
    if not re.fullmatch(r"[a-f0-9]{64}", args.selection_sha256):
        raise InstallError("expected raw selection SHA-256 required")
    document_flag, document_name, _ = selected_document(args)
    selection = read(bundle, document_name, 1024 * 1024)
    if sha(selection) != args.selection_sha256:
        raise InstallError("selected document differs from operator-selected bytes")
    package = read(bundle, "package.json", 1024 * 1024)
    runtime, validator = executable(args.python), executable(args.validator)
    parent = directory(destination.parent)
    try:
        os.mkdir(destination.name, 0o700, dir_fd=parent)
    finally:
        os.close(parent)
    frozen = destination / "inputs"
    frozen.mkdir(mode=0o700)
    (frozen / "closure").mkdir(mode=0o700)
    (destination / "tmp").mkdir(mode=0o700)
    files = {"package.json": package, document_name: selection}
    for filename, raw in files.items():
        (frozen / filename).write_bytes(raw)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(destination / "tmp"),
           "PIP_CONFIG_FILE": os.devnull, "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1",
           "PIP_NO_INDEX": "1", "PIP_NO_CACHE_DIR": "1"}
    plan = unique_json(run([validator, "--package", str(frozen / "package.json"),
                           document_flag, str(frozen / document_name), "--component", args.component],
                          destination, env, "validate-selection"))
    if plan["schema_version"] != 1 or plan["component"] != args.component:
        raise InstallError("unsupported validator projection")
    identity = selected_identity(plan, args)
    expected = {"artifact.whl": plan["artifact_digest"],
                **{"closure/" + key: digest for key, digest in plan["closure"].items()}}
    total = 0
    for filename, digest in expected.items():
        raw = read(bundle, filename)
        total += len(raw)
        if total > MAX_TOTAL or sha(raw) != digest:
            raise InstallError("selected wheel closure differs from retained bytes")
        files[filename] = raw
        (frozen / filename).write_bytes(raw)
    probe = "import importlib.machinery,json,platform,struct,sys; print(json.dumps({'runtime':[sys.version,sys.version_info[:2],sys.platform,platform.machine(),sys.implementation.name],'pointer_bits':8*struct.calcsize('P'),'stdlib':sorted(sys.stdlib_module_names),'extensions':importlib.machinery.EXTENSION_SUFFIXES}))"
    observed = unique_json(run([runtime, "-I", "-S", "-B", "-c", probe], destination, env, "python-runtime"))
    if (observed["runtime"][1:] != [[3, 11], *SELECTED_PYTHON_TARGETS[target], "cpython"]
            or target == "linux-x86_64" and observed.get("pointer_bits") != 64):
        raise InstallError("selected wheel profile requires native CPython 3.11 on " + target)
    closure = selected_python_closure(files, plan, set(observed["stdlib"]), observed["extensions"])
    wheelhouse = destination / "wheelhouse"
    wheelhouse.mkdir(mode=0o700)
    for index, wheel in enumerate(closure["wheels"]):
        source = "artifact.whl" if index == 0 else "closure/" + wheel["filename"]
        (wheelhouse / wheel["filename"]).write_bytes(files[source])
    venv = destination / "runtime"
    run([runtime, "-I", "-B", "-m", "venv", str(venv)], destination, env, "create-venv")
    python = str(venv / "bin/python")
    bootstrap, _ = python_graph(unique_json(run([python, "-I", "-B", "-c", PYTHON_GRAPH],
                                 destination, env, "bootstrap-python")), venv)
    if "pip" not in bootstrap or not set(bootstrap) <= {"pip", "setuptools"} or set(bootstrap) & set(closure["versions"]):
        raise InstallError("wheel closure would replace fresh venv bootstrap")
    baseline = snapshot_tree(venv)
    if target == "linux-x86_64" and "lib64" in baseline and baseline["lib64"] != ("symlink", "lib"):
        raise InstallError("Linux venv lib64 must link to lib")
    paths = ["lib/python3.11/site-packages/" + path for wheel in closure["wheels"] for path in wheel["files"]]
    paths += ["bin/" + script for wheel in closure["wheels"] for script in wheel["scripts"]]
    checked_paths([*baseline, *paths])
    run([python, "-I", "-B", "-m", "pip", "install", "--no-index", "--no-cache-dir", "--only-binary=:all:",
         "--no-compile", "--require-hashes", "--find-links", str(wheelhouse), "-r", str(frozen / "closure/requirements")],
        destination, env, "pip-install")
    payloads = verify_selected_python(venv, baseline, closure)
    run([python, "-I", "-B", "-m", "pip", "check"], destination, env, "pip-check")
    graph, _ = python_graph(unique_json(run([python, "-I", "-B", "-c", PYTHON_GRAPH],
                               destination, env, "installed-python")), venv)
    if graph != {**bootstrap, **closure["versions"]}:
        raise InstallError("installed Python distributions differ from selected closure")
    launch = selected_python_launch(python, plan, closure)
    configuration = {"mcpServers": {plan["mcp_name"]: {"command": launch[0], "args": launch[1:]}}}
    receipt = {"schema_version": 1, "scope": "observed_software_installation_only",
               "selection_sha256": args.selection_sha256, **identity,
               "inputs": {key: sha(raw) for key, raw in sorted(files.items())}, "runtime": observed["runtime"],
               "installed_graph": graph, "bootstrap_distributions": bootstrap, "verification": payloads,
               "command": launch, "destination": str(destination), "mcp_configuration": configuration,
               "launch_environment": "Owned by the consuming host; this configuration does not clear ambient environment, supply external executables or filter tools.",
               "installer_generated_files": "Recorded hashes, not retained wheel payloads; canonical launch bypasses generated wrappers; selected console-script extras are refused and runtime usability is not asserted."}
    if target != "macos-arm64":
        receipt["python_target"] = target
    ready = destination / "pending-ready"
    ready.mkdir(mode=0o700)
    (ready / "installed.json").write_bytes(encoded(receipt))
    (ready / "mcp.json").write_bytes(encoded(configuration))
    ready.rename(destination / "ready")
    return receipt


def install(args: argparse.Namespace) -> dict:
    bundle = Path(args.bundle).resolve(strict=True)
    destination = new_destination(args.destination, bundle)
    configuration = selected_configuration(args)
    files, target = inputs(bundle, args.contents_sha256, args.family)
    versions = python_closure(files, args.family) if args.family in FAMILIES else None
    tarballs = npm_closure(files) if args.family == "activepieces" else None
    runtime = executable(args.python if versions else args.node)
    npm = executable(args.npm_cli) if tarballs else None
    parent = directory(destination.parent)
    try:
        os.mkdir(destination.name, 0o700, dir_fd=parent)
    finally:
        os.close(parent)
    # Parent and destination must remain controlled by the invoking operator.
    # Freeze verified bytes; later source changes cannot alter installer inputs.
    frozen = destination / "inputs"
    frozen.mkdir(mode=0o700)
    for filename, data in files.items():
        with (frozen / filename).open("xb") as output:
            output.write(data)
    if configuration is not None:
        (destination / "configuration").mkdir(mode=0o700)
        config_path = destination / "configuration/selected.json"
        with config_path.open("xb") as output:
            output.write(configuration)
    (destination / "tmp").mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(destination / "tmp"),
           "PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1"}
    identity = {}
    if versions:
        probe = "import json,platform,sys; print(json.dumps([sys.version,sys.version_info[:2],sys.platform,platform.machine(),sys.implementation.name]))"
        if target is not None:
            probe = ("import json,os,platform,struct,sys; print(json.dumps({"
                     "'version':platform.python_version(),'implementation':sys.implementation.name,"
                     "'platform':sys.platform,'machine':platform.machine(),'pointer_bits':8*struct.calcsize('P'),"
                     "'glibc':os.confstr('CS_GNU_LIBC_VERSION')}))")
        observed = json.loads(run([runtime, "-I", "-c", probe], destination, env, "python-runtime"))
        if target is not None and observed != target["python"]:
            raise InstallError("selected Python differs from the bundle runtime target")
        if target is None and observed[1:] != [[3, 11], "darwin", "arm64", "cpython"]:
            raise InstallError("this wheel closure requires CPython 3.11 on macOS arm64")
        env.update({"PIP_CONFIG_FILE": os.devnull, "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1",
                    "PIP_NO_INDEX": "1", "PIP_NO_CACHE_DIR": "1"})
        venv = destination / "runtime"
        run([runtime, "-I", "-m", "venv", str(venv)], destination, env, "create-venv")
        python = str(venv / "bin/python")
        bootstrap, _ = python_graph(json.loads(run([python, "-I", "-c", PYTHON_GRAPH],
            destination, env, "bootstrap-python")), venv)
        if "pip" not in bootstrap or not set(bootstrap) <= {"pip", "setuptools"}:
            raise InstallError("unexpected fresh venv bootstrap distributions")
        run([python, "-I", "-m", "pip", "install", "--no-index", "--no-cache-dir", "--only-binary=:all:",
             "--require-hashes", "--find-links", str(frozen), "-r", str(frozen / "install-offline.txt")], destination, env, "pip-install")
        run([python, "-I", "-m", "pip", "check"], destination, env, "pip-check")
        actual, installed = python_graph(json.loads(run([python, "-I", "-c", PYTHON_GRAPH],
            destination, env, "installed-python")), venv)
        if actual != {**bootstrap, **versions}:
            raise InstallError("installed versions differ from pinned closure")
        namespace, module = FAMILIES[args.family]
        wheels = {filename: verify_wheel_payload(frozen / filename, installed, venv)
                  for filename in sorted(files) if filename.endswith(".whl")}
        component = next(filename for filename in wheels if filename.startswith(namespace + "-"))
        payloads = wheels[component]["payloads"]
        identity = {"bootstrap_distributions": bootstrap, "installed_graph": actual,
                    "wheels": wheels,
                    "excluded_installer_metadata": "Wheel RECORD is rewritten by pip; generated metadata and console scripts are not retained archive payloads."}
        command = [python, "-I", "-m", module]
    else:
        observed = json.loads(run([runtime, "-p", "JSON.stringify([process.version,process.platform,process.arch])"], destination, env, "node-runtime"))
        if target is not None and observed != [target["node"][key] for key in ("version", "platform", "arch")]:
            raise InstallError("selected Node differs from the bundle runtime target")
        if target is None and (observed[0] not in ("v24.19.0", "v22.19.0") or observed[1:] != ["darwin", "arm64"]):
            raise InstallError("this consumer profile is checked on Node 24.19.0/22.19.0 macOS arm64")
        env = npm_environment(destination, runtime, env)
        # Establish the selected project before npm runs, so ancestor project
        # configuration cannot become the consumer's local configuration.
        for filename in ("package.json", "package-lock.json", "rkat-toolkit-activepieces-0.1.0.tgz"):
            (destination / filename).write_bytes(files[filename])
        npm_command = [runtime, npm, "--prefix", str(destination)]
        if target is not None:
            npm_version = run(npm_command + ["--version"], destination, env, "npm-runtime").decode("ascii").strip()
            if npm_version != target["node"]["npm_version"]:
                raise InstallError("selected npm differs from the bundle runtime target")
        for index, row in enumerate(tarballs):
            run(npm_command + ["cache", "add", str(frozen / row["path"]), "--offline"], destination, env, "npm-cache-" + str(index))
        # npm owns dependency realization. ci operates only in the new root.
        run(npm_command + ["ci", "--offline", "--ignore-scripts", "--no-audit", "--no-fund"], destination, env, "npm-install")
        lock = json.loads(files["package-lock.json"])
        versions = {}
        for path, row in lock["packages"].items():
            if not path:
                continue
            relative(path)
            installed = json.loads(read(destination, path + "/package.json"))
            if installed["version"] != row["version"]:
                raise InstallError("installed npm versions differ from lock")
            versions[installed["name"]] = installed["version"]
        payloads = verify_npm_payload(frozen / "rkat-toolkit-activepieces-0.1.0.tgz", destination / "node_modules/@rkat/toolkit-activepieces")
        for row in tarballs:
            verify_npm_payload(frozen / row["path"], destination / "node_modules" / row["name"])
        command = [runtime, str(destination / "node_modules/@rkat/toolkit-activepieces/dist/mcp-cli.js")]
    profiles = json.loads(run(command + ["profiles"], destination, env, "profiles"))
    if not profiles:
        raise InstallError("installed profile listing empty")
    receipt = {"schema_version": 1, "scope": "observed_software_installation_only",
               "family": args.family, "contents_sha256": args.contents_sha256,
               "inputs": {key: sha(value) for key, value in sorted(files.items())},
               "runtime": observed, "versions": versions, "matching_component_payloads": payloads,
               "installed_identity": identity,
               "command": command, "destination": str(destination)}
    if target is not None:
        receipt["runtime_target"] = target
    if configuration is not None:
        run(command + ["check-config", "--config", str(config_path)], destination, env,
            "check-config", discard_output=True)
        if read(destination, "configuration/selected.json", MAX_CONFIGURATION) != configuration:
            raise InstallError("frozen configuration changed during validation")
        launch = command + ["serve", "--config", str(config_path)]
        mcp = {"mcpServers": {args.server_name: {"command": launch[0], "args": launch[1:]}}}
        receipt.update({"command": launch, "mcp_configuration": mcp,
                        "configuration": {"sha256": sha(configuration), "path": str(config_path),
                                          "server_name": args.server_name, "validation": "configuration_only"},
                        "launch_environment": "Owned by the consuming host; this configuration does not clear ambient environment, supply credentials or attest account access."})
        pending = destination / "pending-ready"
        pending.mkdir(mode=0o700)
        (pending / "mcp.json").write_bytes(encoded(mcp))
        (pending / "installed.json").write_bytes(encoded(receipt))
        pending.rename(destination / "ready")
        return receipt
    with (destination / "installed.json").open("xb") as output:
        output.write(encoded(receipt))
    return receipt


def existing_tree(root: Path, maximum: int = 100000) -> dict[str, str]:
    """Inspect bounded file kinds without following links or reading their targets."""
    result = {}
    def walk(handle: int, prefix: str, depth: int) -> None:
        if depth > 64:
            raise InstallError("existing tree exceeds depth limit")
        with os.scandir(handle) as entries:
            for item in entries:
                key = prefix + item.name
                relative(key)
                if len(result) >= maximum:
                    raise InstallError("existing tree exceeds entry limit")
                mode = item.stat(follow_symlinks=False).st_mode
                if stat.S_ISDIR(mode):
                    result[key] = "directory"
                    child = os.open(item.name, DIR_FLAGS, dir_fd=handle)
                    try:
                        walk(child, key + "/", depth + 1)
                    finally:
                        os.close(child)
                elif stat.S_ISREG(mode):
                    result[key] = "file"
                elif stat.S_ISLNK(mode):
                    result[key] = "link"
                else:
                    raise InstallError("existing tree contains a special file")
    handle = directory(root)
    try:
        walk(handle, "", 0)
    finally:
        os.close(handle)
    return result


def existing_destination(value: str, bundle: Path) -> Path:
    path = Path(value)
    if (not path.is_absolute() or str(path) != value or path == path.parent
            or any(part in (".", "..") for part in value.split("/"))):
        raise InstallError("destination must be an absolute canonical directory")
    # Unlike new_destination, do not resolve a substituted existing directory.
    handle = directory(path)
    os.close(handle)
    if path == bundle or bundle in path.parents:
        raise InstallError("destination lies inside the bundle")
    return path


def existing_resolved(path: Path) -> str:
    try:
        return str(path.resolve(strict=True))
    except RuntimeError as error:
        raise InstallError("existing path cannot be resolved") from error


def readback_validator(argv: list[str], cwd: Path, *, timeout: float = 30,
                       limit: int = MAX_FILE) -> bytes:
    """Run only the host's trusted metadata owner, with bounded in-memory output."""
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(cwd)}
    child, interrupted = None, False
    caller_handlers = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
    def defer_interrupt(signum, frame):
        nonlocal interrupted
        interrupted = True
    try:
        # A normal cancellation must not land between process creation and
        # assigning its handle. Record it until cleanup owns that handle.
        # Python signal dispositions are reset by exec; no blocked signal mask
        # is inherited by the validator.
        try:
            for signum in caller_handlers:
                signal.signal(signum, defer_interrupt)
            child = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     start_new_session=True)
        finally:
            for signum, handler in caller_handlers.items():
                signal.signal(signum, handler)
        if interrupted:
            raise KeyboardInterrupt
        deadline, output = time.monotonic() + timeout, bytearray()
        with selectors.DefaultSelector() as selector:
            selector.register(child.stdout, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise InstallError("metadata validation timed out")
                for key, _ in selector.select(remaining):
                    chunk = os.read(key.fileobj.fileno(), min(8192, limit + 1 - len(output)))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        output.extend(chunk)
                        if len(output) > limit:
                            raise InstallError("metadata validation output exceeds limit")
        remaining = deadline - time.monotonic()
        if remaining <= 0 or child.wait(timeout=remaining):
            raise InstallError("metadata validation failed")
        return bytes(output)
    except BaseException:
        if child is None:
            raise
        handlers = {signum: signal.signal(signum, signal.SIG_IGN)
                    for signum in (signal.SIGINT, signal.SIGTERM)}
        try:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                pass
            # Kill the group even if its leader exited while descendants held
            # stdout open. Waiting only for the leader is not enough.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
        finally:
            for signum, handler in handlers.items():
                signal.signal(signum, handler)
        raise
    finally:
        try:
            if child is not None:
                child.stdout.close()
        finally:
            # Also restore the original pair if cancellation interrupted the
            # initial handoff while only one disposition had been restored.
            try:
                signal.signal(signal.SIGINT, caller_handlers[signal.SIGINT])
            finally:
                signal.signal(signal.SIGTERM, caller_handlers[signal.SIGTERM])


def selected_readback_inputs(args: argparse.Namespace, bundle: Path,
                             destination: Path, python: bool) -> tuple[dict, dict, str]:
    """Freeze independent intent for the canonical owner, outside the observation."""
    if not re.fullmatch(r"[a-f0-9]{64}", args.selection_sha256):
        raise InstallError("expected raw selection SHA-256 required")
    document_flag, document_name, _ = selected_document(args)
    selection = read(bundle, document_name, 1024 * 1024)
    if sha(selection) != args.selection_sha256:
        raise InstallError("selected document differs from operator-selected bytes")
    files = {"package.json": read(bundle, "package.json", 1024 * 1024),
             document_name: selection}
    selected_paths = (args.python if python else args.node, args.validator)
    if any(not value or not Path(value).is_absolute() for value in selected_paths):
        raise InstallError("supply absolute selected executable paths")
    runtime, validator = (existing_resolved(Path(value)) for value in selected_paths)
    for path in (runtime, validator):
        if not stat.S_ISREG(Path(path).stat().st_mode) or not os.access(path, os.X_OK):
            raise InstallError("selected executable must be an executable regular file")
    # Honor the host's permitted scratch location, but validate it before any
    # temporary-file probe or creation could write into the observed trees.
    temporary_value = os.environ.get("TMPDIR", "/tmp")
    if not temporary_value or not Path(temporary_value).is_absolute():
        raise InstallError("metadata scratch must be an absolute directory")
    temporary_root = Path(existing_resolved(Path(temporary_value)))
    if any(temporary_root == root or root in temporary_root.parents for root in (bundle, destination)):
        raise InstallError("metadata scratch must be outside observed inputs and destination")
    temporary_handle = directory(temporary_root)
    os.close(temporary_handle)
    with tempfile.TemporaryDirectory(prefix="toolkit-selected-readback-", dir=temporary_root) as scratch:
        frozen = Path(scratch)
        for filename, raw in files.items():
            (frozen / filename).write_bytes(raw)
        plan = unique_json(readback_validator([validator, "--package", str(frozen / "package.json"),
                           document_flag, str(frozen / document_name), "--component", args.component], frozen))
    if (type(plan.get("schema_version")) is not int or plan["schema_version"] != 1 or plan["component"] != args.component
            or not isinstance(plan.get("closure"), dict) or not isinstance(plan.get("entrypoint"), dict)):
        raise InstallError("unsupported validator projection")
    selected_identity(plan, args)
    artifact = "artifact.whl" if python else "artifact.tgz"
    expected = {artifact: plan["artifact_digest"],
                **{"closure/" + key: digest for key, digest in plan["closure"].items()}}
    total = 0
    for filename, digest in expected.items():
        raw = read(bundle, filename)
        total += len(raw)
        if total > MAX_TOTAL or sha(raw) != digest:
            raise InstallError("selected artifact closure differs from retained bytes")
        files[filename] = raw
    frozen_tree = {key: "file" for key in files}
    frozen_tree["closure"] = "directory"
    if existing_tree(destination / "inputs", 260) != frozen_tree:
        raise InstallError("existing frozen input set differs")
    for filename, raw in files.items():
        if read(destination / "inputs", filename) != raw:
            raise InstallError("existing frozen input differs from selection")
    return files, plan, runtime


def selected_python_readback(destination: Path, runtime: str, files: dict, plan: dict,
                             target: str = "macos-arm64") -> tuple[dict, list[str]]:
    """Check selected wheel bytes without importing the observed environment."""
    target = selected_python_target(target)
    if sys.implementation.name != "cpython" or sys.version_info[:2] != (3, 11):
        raise InstallError("selected Python readback requires a CPython 3.11 verifier")
    # This is the verifier's own stdlib metadata, not evidence of the selected
    # runtime's compatibility. Include the selected target's suffixes when
    # inspecting elsewhere; pip still owns compatibility at installation.
    extensions = sorted(set(importlib.machinery.EXTENSION_SUFFIXES)
                        | {".abi3.so", ".so"}
                        | ({".cpython-311-darwin.so"} if target == "macos-arm64" else
                           {".cpython-311-x86_64-linux-gnu.so", ".cpython-311-x86_64-linux-musl.so"}))
    closure = selected_python_closure(files, plan, set(sys.stdlib_module_names), extensions)
    venv, site_prefix = destination / "runtime", "lib/python3.11/site-packages/"
    cfg = {}
    for line in read(venv, "pyvenv.cfg", 16384).decode("utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or key.strip() in cfg:
            raise InstallError("invalid existing venv configuration")
        cfg[key.strip()] = value.strip()
    if (cfg.get("include-system-site-packages") != "false" or cfg.get("executable") != runtime
            or not re.fullmatch(r"3\.11\.[0-9]+", cfg.get("version", ""))
            or not Path(cfg.get("home", "")).is_absolute()
            or existing_resolved(Path(cfg["home"]) / "python3.11") != runtime):
        raise InstallError("existing venv differs from selected runtime path")
    observed = existing_tree(venv)
    expected, generated, graph = {}, set(), {}
    for wheel in closure["wheels"]:
        dist = site_prefix + wheel["dist_info"]
        for path, raw in wheel["files"].items():
            if path != wheel["dist_info"] + "/RECORD":
                expected[site_prefix + path] = raw
        generated.update((dist + "/RECORD", dist + "/INSTALLER", dist + "/REQUESTED"))
        generated.update("bin/" + script for script in wheel["scripts"])
        graph[wheel["name"]] = {"version": wheel["version"], "wheel_sha256": wheel["sha256"],
                               "payloads": len(wheel["files"]) - 1}
    if any(observed.get(path) != "file" or read(venv, path) != raw for path, raw in expected.items()):
        raise InstallError("existing Python payload differs from retained wheel")
    if any(observed.get(path) != "file" for path in generated):
        raise InstallError("existing generated file set differs")
    bootstrap_files = 0
    fixed_dirs = {"bin", "include", "include/python3.11", "lib", "lib/python3.11", site_prefix.rstrip("/")}
    bootstrap_bins = {"pip", "pip3", "pip3.11", "activate", "activate.csh", "activate.fish", "Activate.ps1"}
    for path, kind in observed.items():
        if path in expected or path in generated:
            continue
        if path.startswith(site_prefix):
            top = path.removeprefix(site_prefix).split("/", 1)[0]
            bootstrap = (top in {"pip", "setuptools", "pkg_resources", "_distutils_hack", "distutils-precedence.pth"}
                         or re.fullmatch(r"(?:pip|setuptools)-[A-Za-z0-9_.+!-]+\.dist-info", top))
            if kind == "link":
                raise InstallError("unexpected existing Python symlink")
            if bootstrap:
                bootstrap_files += kind == "file"
            elif kind != "directory":
                raise InstallError("unselected existing Python file")
        elif path in {"bin/python", "bin/python3", "bin/python3.11"}:
            if kind != "link" or existing_resolved(venv / path) != runtime:
                raise InstallError("existing venv interpreter path differs")
        elif path == "lib64" and target == "linux-x86_64":
            if kind != "link" or os.readlink(venv / path) != "lib":
                raise InstallError("existing Linux venv lib64 must link to lib")
        elif path == "pyvenv.cfg" or path.removeprefix("bin/") in bootstrap_bins and path.startswith("bin/"):
            if kind != "file":
                raise InstallError("unexpected existing Python bootstrap path")
        elif path not in fixed_dirs or kind != "directory":
            raise InstallError("unselected existing Python path")
    if any(observed.get("bin/" + leaf) != "link" for leaf in ("python", "python3", "python3.11")):
        raise InstallError("existing venv interpreter links missing")
    wheel_files = {row["filename"]: files["artifact.whl"] if index == 0 else files["closure/" + row["filename"]]
                   for index, row in enumerate(closure["wheels"])}
    if existing_tree(destination / "wheelhouse", 257) != {key: "file" for key in wheel_files}:
        raise InstallError("existing wheelhouse set differs")
    if any(read(destination / "wheelhouse", key) != raw for key, raw in wheel_files.items()):
        raise InstallError("existing wheelhouse differs")
    return {"wheels": graph, "versions": closure["versions"], "verified_payload_files": len(expected),
            "generated_file_hashes": {path: sha(read(venv, path)) for path in sorted(generated)},
            "excluded_bootstrap_files": bootstrap_files}, selected_python_launch(str(venv / "bin/python"), plan, closure)


def verify_existing_selected(args: argparse.Namespace) -> dict:
    """Compare independent intent and current payloads; never install or repair."""
    python = bool(getattr(args, "selected_python", None))
    bundle = Path(existing_resolved(Path(args.selected_python if python else args.selected_node)))
    destination = existing_destination(args.destination, bundle)
    if any(os.path.lexists(destination / key) for key in ("pending-ready", "installed.json", "configuration")):
        raise InstallError("incomplete or crossed publication remains")
    if existing_tree(destination / "ready", 3) != {"installed.json": "file", "mcp.json": "file"}:
        raise InstallError("incomplete selected publication")
    files, plan, runtime = selected_readback_inputs(args, bundle, destination, python)
    # Selected installation emits these projections without a separate 64 KiB
    # configuration limit. Escaped Unicode and duplicated launch fields may
    # expand beyond the original 1 MiB metadata document bounds.
    receipt_raw = read(destination, "ready/installed.json")
    receipt = unique_json(receipt_raw)
    if type(receipt.get("schema_version")) is not int:
        raise InstallError("receipt schema must be an integer")
    if python:
        target = selected_python_target(getattr(args, "python_target", None))
        if (receipt.get("python_target", "macos-arm64") != target
                or target == "macos-arm64" and "python_target" in receipt):
            raise InstallError("receipt differs from selected Python target")
        proof, launch = selected_python_readback(destination, runtime, files, plan, target)
        recorded = receipt.get("verification", {})
        if (not isinstance(recorded, dict)
                or any(recorded.get(key) != proof[key] for key in ("wheels", "verified_payload_files"))):
            raise InstallError("receipt payload observations differ")
        if recorded.get("installer_generated") != proof["generated_file_hashes"]:
            raise InstallError("generated file hashes differ from historical receipt")
        recorded_graph = receipt.get("installed_graph", {})
        if (not isinstance(recorded_graph, dict) or set(recorded_graph) - set(proof["versions"]) - {"pip", "setuptools"}
                or any(recorded_graph.get(key) != value for key, value in proof["versions"].items())):
            raise InstallError("receipt selected distributions differ")
    else:
        closure = selected_npm_closure(files, plan)
        tree = existing_tree(destination / "node_modules", MAX_NPM_MEMBERS * 2)
        if any(kind == "link" for kind in tree.values()):
            raise InstallError("unexpected existing npm symlink")
        build_receipt = None
        if closure.get("build"):
            build_receipt = checked_node_build_receipt(closure, args, receipt.get("node_build"))
            if unique_json(read(bundle, "build-review.json", MAX_CONFIGURATION * 32)) != build_receipt["review"]:
                raise InstallError("existing build differs from reviewed software effect")
        elif "node_build" in receipt:
            raise InstallError("unexpected generated build receipt")
        graph = verify_selected_npm(destination, closure, build_receipt["outputs"] if build_receipt else None)
        for target, source in (("package.json", "closure/package-json"), ("package-lock.json", "closure/package-lock"), ("artifact.tgz", "artifact.tgz")):
            if read(destination, target) != files[source]:
                raise InstallError("existing npm project differs from selection")
        proof = {"installed_graph": graph, "verified_payload_files": sum(row["files"] for row in graph.values())}
        if build_receipt:
            proof["node_build"] = build_receipt
        if receipt.get("installed_graph") != graph:
            raise InstallError("receipt npm graph differs")
        if target := closure.get("npm_target"):
            if (node_runtime_hash(runtime) != target["node_sha256"]
                    or any(receipt.get(key) != closure[key] for key in ("npm_target", "excluded_optional_packages"))):
                raise InstallError("receipt conditional npm graph or runtime differs")
            proof.update({key: closure[key] for key in ("npm_target", "excluded_optional_packages")})
        elif any(key in receipt for key in ("npm_target", "excluded_optional_packages")):
            raise InstallError("unexpected conditional npm receipt")
        native = closure.get("native")
        if native:
            if (receipt.get("native_payloads") != native["summary"]
                    or node_runtime_hash(runtime) != native["summary"]["target"]["node_sha256"]):
                raise InstallError("native payload receipt or selected Node executable differs")
            proof["native_payloads"] = native["summary"]
        elif "native_payloads" in receipt:
            raise InstallError("unexpected native payload receipt")
        launch = [runtime, str(destination / closure["component_path"] / closure["entrypoint"]), *plan["entrypoint"]["arguments"]]
    configuration = {"mcpServers": {plan["mcp_name"]: {"command": launch[0], "args": launch[1:]}}}
    identity = selected_identity(plan, args)
    if selected_identity(receipt, args) != identity:
        raise InstallError("receipt differs from selected document identity")
    expected = {"schema_version": 1, "scope": "observed_software_installation_only",
                "selection_sha256": args.selection_sha256, **identity,
                "inputs": {key: sha(raw) for key, raw in sorted(files.items())},
                "command": launch, "destination": str(destination), "mcp_configuration": configuration}
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise InstallError("receipt differs from original selection or current retained payloads")
    # Installation writes encoded(configuration). Its exact projected length
    # includes Unicode escaping and every literal argument, unlike the smaller
    # library configuration limit. Keep independently supplied metadata bounded.
    if unique_json(read(destination, "ready/mcp.json", len(encoded(configuration)))) != configuration:
        raise InstallError("published MCP configuration differs from original selection")
    return {"schema_version": 1, "scope": "retained_payload_and_launch_binding_only", "verified": True,
            "selection_kind": "python" if python else "node", "destination": str(destination),
            **({"python_target": target} if python and target != "macos-arm64" else {}),
            "selection_sha256": args.selection_sha256, "component": args.component,
            **identity, "command": launch,
            "selected_runtime_path": runtime, "retained_payloads": proof,
            "historical_receipt": {"sha256": sha(receipt_raw), "recorded_scope": receipt["scope"]},
            "excluded": ["Only explicitly selected native-payload or conditional-graph Node executable bytes are checked when present; runtime behavior, current OS/libc state, compatibility and historical probe claims are not verified.",
                         "Python bootstrap bytes and historical preservation claims are not verified. Generated file hashes establish only consistency with the mutable receipt, not provenance or safety.",
                         "Generated build outputs establish consistency with the original plan, selected tools and mutable receipt. Cold readback does not establish historical build provenance. npm-generated hidden lock metadata, caches and step logs are not verified.",
                         "No observed runtime, npm, pip, donor code or MCP server was executed. Authentication, account access, grants, readiness and native activation are not established."],
            "effects": "Only the host-selected canonical metadata validator executes, with bounded output and temporary copies outside the observed destination; no installation or repair.",
            "custody": "Point-in-time observation requires exclusive host custody of original inputs, executables and destination; not an atomic filesystem snapshot or a claim the environment is safe to execute."}


def verify_existing_python(files: dict[str, bytes], family: str, destination: Path,
                           runtime: str) -> dict:
    """Retained wheel payload proof only; never import the observed environment."""
    venv = destination / "runtime"
    site = venv / "lib/python3.11/site-packages"
    cfg = {}
    for line in read(venv, "pyvenv.cfg", 16384).decode("utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or key.strip() in cfg:
            raise InstallError("invalid existing venv configuration")
        cfg[key.strip()] = value.strip()
    if (cfg.get("include-system-site-packages") != "false" or cfg.get("executable") != runtime
            or not re.fullmatch(r"3\.11\.[0-9]+", cfg.get("version", ""))
            or not Path(cfg.get("home", "")).is_absolute()
            or existing_resolved(Path(cfg.get("home", "")) / "python3.11") != runtime
            or existing_resolved(venv / "bin/python") != runtime):
        raise InstallError("existing venv differs from selected runtime path")
    expected, generated, wrappers, retained_scripts, wheels = set(), set(), set(), set(), {}
    for filename, raw in files.items():
        if not filename.endswith(".whl"):
            continue
        wheels[filename] = verify_wheel_payload(io.BytesIO(raw), site, venv)
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            for item in archive.infolist():
                if item.is_dir():
                    continue
                path = item.filename
                if ".data/" in path:
                    script = path.split(".data/scripts/", 1)[1]
                    wrappers.add(script)
                    retained_scripts.add(script)
                elif path.endswith(".dist-info/RECORD"):
                    generated.add(path)
                else:
                    expected.add(path)
                if path.endswith(".dist-info/METADATA"):
                    dist = path.rsplit("/", 1)[0]
                    generated.update(dist + "/" + suffix for suffix in ("RECORD", "INSTALLER", "REQUESTED", "direct_url.json"))
                if path.endswith(".dist-info/entry_points.txt"):
                    parser = configparser.ConfigParser(interpolation=None)
                    parser.optionxform = str
                    parser.read_string(archive.read(item).decode("utf-8"))
                    for group in ("console_scripts", "gui_scripts"):
                        for script in parser[group] if parser.has_section(group) else ():
                            if len(relative(script)) != 1:
                                raise InstallError("unsupported generated script name")
                            wrappers.add(script)
    observed = existing_tree(site)
    excluded = {"bootstrap": 0, "generated_metadata": 0, "source_caches": 0}
    for path, kind in observed.items():
        if kind == "link":
            raise InstallError("unexpected existing Python symlink")
        top = path.split("/", 1)[0]
        bootstrap = (top in {"pip", "setuptools", "pkg_resources", "_distutils_hack", "distutils-precedence.pth"}
                     or re.fullmatch(r"(?:pip|setuptools)-[A-Za-z0-9_.+!-]+\.dist-info", top))
        if kind == "directory":
            continue
        if path in expected:
            continue
        if bootstrap:
            excluded["bootstrap"] += 1
        elif path in generated:
            excluded["generated_metadata"] += 1
        else:
            parent, _, leaf = path.rpartition("/")
            match = re.fullmatch(r"(.+)\.cpython-311(?:\.opt-[0-9]+)?\.pyc", leaf)
            source_parent = parent.removesuffix("/__pycache__") if parent.endswith("/__pycache__") else "" if parent == "__pycache__" else None
            source = ((source_parent + "/") if source_parent else "") + match[1] + ".py" if match and source_parent is not None else None
            if source not in expected:
                raise InstallError("unselected existing Python file")
            excluded["source_caches"] += 1
    bins = existing_tree(venv / "bin", 4096)
    wrappers.update({"pip", "pip3", "pip3.11", "activate", "activate.csh", "activate.fish", "Activate.ps1"})
    for path, kind in bins.items():
        if path == "__pycache__" and kind == "directory":
            continue
        cache = re.fullmatch(r"__pycache__/(.+)\.cpython-311(?:\.opt-[0-9]+)?\.pyc", path)
        if kind == "file" and cache and cache[1] + ".py" in retained_scripts:
            # pip may compile a relocated retained script. Its bytes are excluded,
            # while verify_wheel_payload above checks the actual source body.
            excluded["source_caches"] += 1
        elif path in {"python", "python3", "python3.11"}:
            if kind != "link" or existing_resolved(venv / "bin" / path) != runtime:
                raise InstallError("existing venv interpreter path differs")
        elif kind != "file" or path not in wrappers:
            raise InstallError("unselected existing Python bin file")
    namespace, _ = FAMILIES[family]
    component = next(filename for filename in wheels if filename.startswith(namespace + "-"))
    return {"wheels": wheels, "matching_component_payloads": wheels[component]["payloads"],
            "excluded_file_counts": excluded}


def verify_existing_npm(files: dict[str, bytes], destination: Path, rows: list[dict]) -> dict:
    archives = [("@rkat/toolkit-activepieces", "rkat-toolkit-activepieces-0.1.0.tgz")]
    archives.extend((row["name"], row["path"]) for row in rows)
    expected, links, versions, payloads = set(), {}, {}, {}
    lock = unique_json(files["package-lock.json"])
    for package, filename in archives:
        archive = npm_archive(files[filename])
        row = lock["packages"]["node_modules/" + package]
        if archive["name"] != package or archive["version"] != row["version"]:
            raise InstallError("npm archive identity differs from selected lock")
        if package != "@rkat/toolkit-activepieces" and archive["integrity"] != row["integrity"]:
            raise InstallError("npm archive integrity differs from selected lock")
        versions[package] = archive["version"]
        payloads[package] = len(archive["files"])
        for path, raw in archive["files"].items():
            key = package + "/" + path
            if read(destination / "node_modules", key) != raw:
                raise InstallError("existing npm payload differs from selected archive")
            expected.add(key)
        metadata = unique_json(archive["files"]["package.json"])
        bins = metadata.get("bin", {})
        if isinstance(bins, str):
            bins = {package.split("/")[-1]: bins}
        if not isinstance(bins, dict):
            raise InstallError("unsupported npm bin declaration")
        for executable_name, target in bins.items():
            if isinstance(target, str):
                target = target.removeprefix("./")
            if len(relative(executable_name)) != 1 or target not in archive["files"]:
                raise InstallError("unsupported npm bin target")
            key = ".bin/" + executable_name
            if key in links:
                raise InstallError("conflicting npm bin declaration")
            links[key] = "../" + package + "/" + target
    for path, kind in existing_tree(destination / "node_modules").items():
        if kind == "directory":
            continue
        if path in links:
            if kind != "link" or os.readlink(destination / "node_modules" / path) != links[path]:
                raise InstallError("existing npm bin link differs")
        elif kind != "file" or (path not in expected and path != ".package-lock.json"):
            raise InstallError("unselected existing npm file")
    for filename in ("package.json", "package-lock.json", "rkat-toolkit-activepieces-0.1.0.tgz"):
        if read(destination, filename) != files[filename]:
            raise InstallError("existing npm project differs from selection")
    return {"versions": versions, "payloads": payloads,
            "matching_component_payloads": payloads["@rkat/toolkit-activepieces"]}


def native_connector_observation(files: dict[str, bytes], family: str,
                                 profile: str | None = None) -> dict | None:
    """Observe an optional publisher contract from already verified wheel bytes.

    Old or unsupported declarations leave ordinary installation unchanged. This
    is software evidence only, not native execution, account binding or access.
    """
    if family not in FAMILIES or (family == "agno" and profile != "salesforce"):
        return None
    namespace, module = FAMILIES[family]
    wheels = [name for name in files if name.startswith(namespace + "-") and name.endswith(".whl")]
    if len(wheels) != 1:
        return None
    filename = wheels[0]
    member = module.replace(".", "/") + ".py"
    manifest_member = namespace + "/PACKAGE-SOURCES.json"
    with zipfile.ZipFile(io.BytesIO(files[filename])) as archive:
        if manifest_member not in archive.namelist():
            return None
        item = archive.getinfo(manifest_member)
        if item.file_size > 1024 * 1024:
            return None
        raw = archive.read(item)
        try:
            manifest = unique_json(raw)
        except (ValueError, UnicodeError):
            return None
        declaration = manifest.get("native_connector")
        expected = {"schema_version": 1, "module": module, "mode": "serve-native",
                    "configuration_argument": "--config", "member": member}
        if family == "agno":
            expected["profiles"] = ["salesforce"]
        mapping = manifest.get("source_mapping")
        source = mapping.get("src/" + member) if isinstance(mapping, dict) else None
        if (not isinstance(declaration, dict) or set(declaration) != set(expected) | {"member_sha256"}
                or type(declaration.get("schema_version")) is not int
                or any(declaration.get(key) != value for key, value in expected.items())
                or manifest.get("distribution") != "meerkat-toolkit-" + family
                or manifest.get("namespace") != namespace or not isinstance(source, dict)
                or source.get("canonical_source") != "sdk/python/src/meerkat_toolkit/adapters/" + family + "_mcp.py"
                or member not in archive.namelist()):
            return None
        member_digest = sha(archive.read(member))
        if declaration.get("member_sha256") != member_digest or source.get("sha256") != member_digest:
            return None
    return {**declaration, "wheel": {"filename": filename, "sha256": sha(files[filename])},
            "package_sources_sha256": sha(raw)}


def verify_existing(args: argparse.Namespace) -> dict:
    """Reconcile independent original intent without execution, writes or repair.

    The operator retains custody of these directories during the observation.
    Excluded bootstrap/generated bytes are not made trustworthy by a receipt.
    """
    bundle = Path(existing_resolved(Path(args.bundle)))
    destination = existing_destination(args.destination, bundle)
    configuration = selected_configuration(args)
    files, target = inputs(bundle, args.contents_sha256, args.family)
    versions = python_closure(files, args.family) if args.family in FAMILIES else None
    tarballs = npm_closure(files) if args.family == "activepieces" else None
    if (versions and args.node) or (tarballs and args.python):
        raise InstallError("runtime argument does not match selected library family")
    runtime_value = args.python if versions else args.node
    if not runtime_value or not Path(runtime_value).is_absolute():
        raise InstallError("supply an absolute selected runtime path")
    runtime = existing_resolved(Path(runtime_value))
    if not stat.S_ISREG(Path(runtime).stat().st_mode) or not os.access(runtime, os.X_OK):
        raise InstallError("selected runtime must be an executable regular file")
    frozen = destination / "inputs"
    if existing_tree(frozen, 257) != {key: "file" for key in files}:
        raise InstallError("existing frozen input set differs")
    for filename, raw in files.items():
        if read(frozen, filename) != raw:
            raise InstallError("existing frozen input differs from selection")
    if os.path.lexists(destination / "pending-ready"):
        raise InstallError("incomplete publication remains")
    if configuration is None:
        if os.path.lexists(destination / "ready") or os.path.lexists(destination / "configuration"):
            raise InstallError("configured publication requires original configuration")
        receipt_raw = read(destination, "installed.json", 1024 * 1024)
    else:
        if os.path.lexists(destination / "installed.json"):
            raise InstallError("crossed publication form")
        if existing_tree(destination / "ready", 3) != {"installed.json": "file", "mcp.json": "file"}:
            raise InstallError("incomplete configured publication")
        if existing_tree(destination / "configuration", 2) != {"selected.json": "file"}:
            raise InstallError("configuration publication differs")
        if read(destination, "configuration/selected.json", MAX_CONFIGURATION) != configuration:
            raise InstallError("frozen configuration differs from original selection")
        receipt_raw = read(destination, "ready/installed.json", 1024 * 1024)
    receipt = unique_json(receipt_raw)
    if type(receipt.get("schema_version")) is not int:
        raise InstallError("receipt schema must be an integer")
    if versions:
        proof = verify_existing_python(files, args.family, destination, runtime)
        command = [str(destination / "runtime/bin/python"), "-I", "-m", FAMILIES[args.family][1]]
    else:
        proof = verify_existing_npm(files, destination, tarballs)
        versions = proof["versions"]
        command = [runtime, str(destination / "node_modules/@rkat/toolkit-activepieces/dist/mcp-cli.js")]
    expected = {"schema_version": 1, "scope": "observed_software_installation_only", "family": args.family,
                "contents_sha256": args.contents_sha256, "inputs": {key: sha(raw) for key, raw in sorted(files.items())},
                "versions": versions, "matching_component_payloads": proof["matching_component_payloads"],
                "command": command, "destination": str(destination)}
    if target is not None:
        expected["runtime_target"] = target
    elif "runtime_target" in receipt:
        raise InstallError("unexpected receipt runtime target")
    if configuration is not None:
        path = str(destination / "configuration/selected.json")
        command = command + ["serve", "--config", path]
        mcp = {"mcpServers": {args.server_name: {"command": command[0], "args": command[1:]}}}
        expected.update({"command": command, "mcp_configuration": mcp,
                         "configuration": {"sha256": sha(configuration), "path": path,
                                           "server_name": args.server_name, "validation": "configuration_only"}})
        if unique_json(read(destination, "ready/mcp.json", 65536)) != mcp:
            raise InstallError("published MCP configuration differs from original selection")
    elif any(key in receipt for key in ("configuration", "mcp_configuration", "launch_environment")):
        raise InstallError("unexpected configured receipt fields")
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise InstallError("receipt differs from original selection or current retained payloads")
    # The original selected configuration has just been matched to the retained
    # installation. A family name alone cannot enable a profile-specific entry.
    profile = None
    if args.family == "agno" and configuration is not None:
        try:
            selected = unique_json(configuration)
        except (ValueError, UnicodeError):
            selected = None
        profile = selected.get("profile") if isinstance(selected, dict) else None
    native_connector = native_connector_observation(files, args.family, profile)
    return {"schema_version": 1, "scope": "retained_payload_and_launch_binding_only", "verified": True,
            **({"native_connector": native_connector} if native_connector is not None else {}),
            "family": args.family, "destination": str(destination), "contents_sha256": args.contents_sha256,
            "historical_receipt": {"sha256": sha(receipt_raw), "recorded_scope": receipt["scope"]},
            "configuration_sha256": sha(configuration) if configuration is not None else None,
            "command": command, "selected_runtime_path": runtime, "retained_payloads": proof,
            "excluded": ["Runtime executable bytes, compatibility and prior probe claims are not verified.",
                         "Python pip/setuptools bootstrap trees, their known startup file, generated metadata/console wrappers and source-associated pyc bytes are not verified.",
                         "npm-generated hidden lock metadata is not verified.",
                         "No parser, MCP server, authentication, account access, grant or native activation was executed or established."],
            "custody": "Point-in-time observation of operator-controlled directories; not a filesystem sandbox or a claim the environment is safe to execute."}


# One fixed official route. No package-selected executable is invoked here.
OFFICIAL_FAMILY = "official-github"
OFFICIAL_PROFILE = "selected-reads"
OFFICIAL_TOOLS = ["get_me", "list_commits", "get_file_contents"]
OFFICIAL_ENV = "GITHUB_PERSONAL_ACCESS_TOKEN"
OFFICIAL_ARGS = ["stdio", "--read-only", "--gh-host", "https://github.com", "--tools", ",".join(OFFICIAL_TOOLS)]


def official_target(component):
    # Historical descriptors name no machine target and retain their original
    # macOS-only contract. Human-readable runtime_profile never selects one.
    target = component.get("target", "macos-arm64")
    if target not in ("macos-arm64", "linux-x86_64"):
        raise InstallError("unsupported official target")
    return target


def check_official_executable(binary, target):
    if target == "macos-arm64":
        if binary[:16] != bytes.fromhex("cffaedfe0c0000010000000002000000"):
            raise InstallError("unsupported official executable")
        return
    # Fixed static ELF64 little-endian x86_64 executable profile. Inspect bytes
    # only: neither metadata inspection nor installation launches the binary.
    if len(binary) < 64 or binary[:16] != bytes.fromhex("7f454c46020101000000000000000000"):
        raise InstallError("unsupported official executable")
    kind, machine, version, entry, phoff, _, flags, size, phsize, count, _, _, _ = struct.unpack_from("<HHIQQQIHHHHHH", binary, 16)
    if ((kind, machine, version, flags, size, phsize) != (2, 62, 1, 0, 64, 56)
            or not 1 <= count <= 128 or phoff < 64 or phoff + count * phsize > len(binary)):
        raise InstallError("unsupported official executable")
    executable_entry = False
    for index in range(count):
        segment, permissions, offset, address, _, filesz, memsz, _ = struct.unpack_from("<IIQQQQQQ", binary, phoff + index * phsize)
        if segment in (2, 3) or offset + filesz > len(binary):
            raise InstallError("unsupported official executable")
        if segment == 1:
            if filesz > memsz:
                raise InstallError("unsupported official executable")
            executable_entry |= bool(permissions & 1 and address <= entry < address + filesz)
    if not executable_entry:
        raise InstallError("unsupported official executable")


def official_inputs(bundle: Path, expected: str, component: str):
    if component != "github" or not isinstance(expected, str) or not re.fullmatch(r"[a-f0-9]{64}", expected):
        raise InstallError("unsupported official selection")
    raw = read(bundle, "CONTENTS.json", 1024 * 1024)
    if sha(raw) != expected:
        raise InstallError("official contents differ")
    manifest = unique_json(raw)
    if set(manifest) != {"format", "files"} or type(manifest["format"]) is not int or manifest["format"] != 1:
        raise InstallError("unsupported official contents")
    entries = manifest["files"]
    if not isinstance(entries, dict) or not 1 <= len(entries) <= 512:
        raise InstallError("invalid official inventory")
    files, total = {}, 0
    for path, item in entries.items():
        parts = relative(path)
        if not isinstance(item, dict) or set(item) != {"sha256", "bytes"} or type(item["bytes"]) is not int or not 0 <= item["bytes"] <= MAX_FILE:
            raise InstallError("invalid official file record")
        if path != "delivery.json" and parts[0] != "github":
            continue
        data = read(bundle, path)
        total += len(data)
        if total > MAX_TOTAL or len(data) != item["bytes"] or sha(data) != item["sha256"]:
            raise InstallError("official payload differs")
        files[path] = data
    delivery = unique_json(files["delivery.json"])
    if delivery.get("format") != "meerkat-toolkit.official-consumers.v1" or delivery.get("role") != "service_connector":
        raise InstallError("unsupported official delivery")
    components = delivery["components"]
    if not isinstance(components, list) or any(not isinstance(row, dict) for row in components):
        raise InstallError("invalid official components")
    selected = [row for row in components if row.get("catalog_id") == component]
    if len(selected) != 1:
        raise InstallError("missing or ambiguous official component")
    selected = selected[0]
    if (selected.get("artifact") != "github/github-mcp-server" or selected.get("configuration_template") != "github/mcp.template.json"
            or selected.get("notices") != "github/notices" or selected.get("selected_operations") != OFFICIAL_TOOLS
            or selected.get("required_secret_environment") != [OFFICIAL_ENV]
            or not isinstance(selected.get("source_revision"), str) or not re.fullmatch(r"[0-9a-f]{40}", selected["source_revision"])):
        raise InstallError("unsupported official profile")
    expected_template = {"mcpServers": {"github-selected-reads": {
        "command": "ABS_GITHUB_INSTALL/github-mcp-server", "args": OFFICIAL_ARGS,
        "cwd": "ABS_GITHUB_INSTALL", "env": {}}}}
    if unique_json(files["github/mcp.template.json"]) != expected_template:
        raise InstallError("unsupported official template")
    binary = files["github/github-mcp-server"]
    check_official_executable(binary, official_target(selected))
    if sha(binary) != selected["artifact_sha256"]:
        raise InstallError("unsupported official executable")
    notices = [path for path in files if path.startswith("github/notices/")]
    if len(notices) != 78 or "github/notices/upstream-LICENSE" not in notices:
        raise InstallError("incomplete official notices")
    expected_files = {"delivery.json", "github/github-mcp-server", "github/mcp.template.json", "github/UPSTREAM-PROFILE.md", *notices}
    if set(files) != expected_files:
        raise InstallError("unsupported official payload layout")
    return files, selected


def official_help(files, component):
    target = official_target(component)
    platform_help = ("Installation and readback support macOS arm64 only. Metadata and configuration checks do not establish host compatibility or callability."
                     if target == "macos-arm64" else
                     "Installation and readback require Linux x86_64. Consult the selected bundle guide for its runtime evidence and limits. Metadata and configuration checks do not establish host compatibility or callability.")
    return {"profile": OFFICIAL_PROFILE, "operations": OFFICIAL_TOOLS, "effect": "read", "selectable_operations": False,
        "fields": [{"name": "token_env", "kind": "string", "required": True, "credential_reference": True}],
        "prerequisites": [{"kind": "host_platform", "instruction": platform_help},
            {"kind": "host_credential_injection", "instruction": "Enter only GITHUB_PERSONAL_ACCESS_TOKEN as token_env. The host must supply its existing secret to that child environment variable. No credential is read or created here."},
            {"kind": "fixed_route", "instruction": "Select all three listed read operations. This route fixes GitHub.com and read-only mode; custom hosts, tools and environment values are unsupported."}],
        "unresolved_details": ["Account permissions, live access and native attachment are not checked.", "The host owns the whole child environment; env:{} does not clear inherited variables."],
        "scope": "configuration_only", "target": target, "guide_text": files["github/UPSTREAM-PROFILE.md"].decode("utf-8"),
        "source_revision": component["source_revision"], "artifact_sha256": component["artifact_sha256"]}


def official_configuration(raw):
    value = unique_json(raw)
    if (set(value) != {"profile", "operations", "config"} or value["profile"] != OFFICIAL_PROFILE
            or value["operations"] != OFFICIAL_TOOLS or value["config"] != {"token_env": OFFICIAL_ENV}):
        raise InstallError("unsupported official configuration")
    return {"profile": OFFICIAL_PROFILE, "valid": True, "scope": "configuration_only",
        "credential_environment_references": [{"field_path": ["config", "token_env"], "environment_variable": OFFICIAL_ENV}]}


def official_launch(destination, server):
    return {"mcpServers": {server: {"command": str(destination / "github-mcp-server"),
                                   "args": OFFICIAL_ARGS, "cwd": str(destination), "env": {}}}}


def official_consumer(args):
    if args.component == "playwright":
        return playwright_consumer(args)
    bundle = Path(existing_resolved(Path(args.official_bundle)))
    files, component = official_inputs(bundle, args.contents_sha256, args.component)
    target = official_target(component)
    if args.official_describe:
        return {"family": OFFICIAL_FAMILY, "profiles": [{"profile": OFFICIAL_PROFILE, "operations": OFFICIAL_TOOLS, "effect": "read"}],
                "help": official_help(files, component), "contents_sha256": args.contents_sha256, "target": target, "scope": "official_metadata_only"}
    raw = selected_configuration(args)
    if raw is None:
        raise InstallError("official route requires explicit configuration")
    projection = official_configuration(raw)
    if target == "linux-x86_64":
        projection["target"] = target
    if args.check_config_only:
        return projection
    import platform
    if target == "macos-arm64" and (sys.platform != "darwin" or platform.machine() != "arm64"):
        raise InstallError("official setup supports macOS arm64 only")
    if target == "linux-x86_64" and (sys.platform != "linux" or platform.machine() != "x86_64"):
        raise InstallError("official setup requires Linux x86_64")
    destination = existing_destination(args.destination, bundle) if args.verify_existing else new_destination(args.destination, bundle)
    launch = official_launch(destination, args.server_name)
    command = [str(destination / "github-mcp-server"), *OFFICIAL_ARGS]
    payloads = {path.removeprefix("github/"): data for path, data in files.items() if path.startswith("github/")}
    expected = {"schema_version": 1, "scope": "observed_software_installation_only", "family": OFFICIAL_FAMILY,
        "component": "github", "profile": OFFICIAL_PROFILE, "contents_sha256": args.contents_sha256,
        "inputs": {name: sha(data) for name, data in sorted(files.items())},
        "artifact_sha256": component["artifact_sha256"], "source_revision": component["source_revision"],
        "target": target, "command": command, "destination": str(destination), "mcp_configuration": launch,
        "configuration": {"path": str(destination / "configuration/selected.json"), "sha256": sha(raw),
                          "server_name": args.server_name, "validation": "configuration_only"}}
    if args.verify_existing:
        expected_tree = {name: "file" for name in payloads}
        expected_tree.update({"inputs/" + name: "file" for name in files})
        expected_tree.update({"configuration/selected.json": "file", "ready/mcp.json": "file", "ready/installed.json": "file"})
        for name in list(expected_tree):
            for parent in PurePosixPath(name).parents:
                if str(parent) != ".": expected_tree[str(parent)] = "directory"
        if existing_tree(destination, 1024) != expected_tree:
            raise InstallError("official installation layout differs")
        for name, data in {**payloads, **{"inputs/" + n: d for n, d in files.items()}, "configuration/selected.json": raw}.items():
            if read(destination, name) != data:
                raise InstallError("official installed payload differs")
        if stat.S_IMODE((destination / "github-mcp-server").stat().st_mode) != 0o755:
            raise InstallError("official executable mode differs")
        receipt_raw = read(destination, "ready/installed.json", MAX_CONFIGURATION)
        receipt = unique_json(receipt_raw)
        if type(receipt.get("schema_version")) is not int or receipt != expected or unique_json(read(destination, "ready/mcp.json")) != launch:
            raise InstallError("official publication differs")
        return {"schema_version": 1, "scope": "retained_payload_and_launch_binding_only", "verified": True,
            **({"target": target} if target == "linux-x86_64" else {}),
            "family": OFFICIAL_FAMILY, "component": "github", "destination": str(destination),
            "contents_sha256": args.contents_sha256, "configuration_sha256": sha(raw), "command": command,
            "historical_receipt": {"sha256": sha(receipt_raw), "recorded_scope": expected["scope"]},
            "retained_payloads": {"files": len(payloads), "artifact_sha256": component["artifact_sha256"]},
            "excluded": ["No GitHub process, account validation, secret read, native attachment or current callability probe was performed."],
            "custody": "Point-in-time observation under exclusive host custody."}
    destination.mkdir(mode=0o700)
    for name, data in {**payloads, **{"inputs/" + n: d for n, d in files.items()}, "configuration/selected.json": raw}.items():
        path = destination / name; path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with path.open("xb") as output: output.write(data)
    (destination / "github-mcp-server").chmod(0o755)
    pending = destination / "pending-ready"; pending.mkdir(mode=0o700)
    (pending / "mcp.json").write_bytes(encoded(launch)); (pending / "installed.json").write_bytes(encoded(expected))
    pending.rename(destination / "ready")
    return expected


PLAYWRIGHT_FAMILY = "official-playwright"
PLAYWRIGHT_PROFILE = "isolated-chrome"
PLAYWRIGHT_OPERATIONS = ["browser_navigate", "browser_snapshot", "browser_fill_form", "browser_click", "browser_close"]
PLAYWRIGHT_ARGS = ["ABS_PLAYWRIGHT_INSTALL/node_modules/@playwright/mcp/cli.js", "--headless", "--isolated", "--browser", "chrome", "--executable-path", "ABS_CHROME_EXECUTABLE", "--output-dir", "ABS_PROCESS_OUTPUT", "--timeout-action", "5000", "--timeout-navigation", "10000"]


def playwright_host(args, bundle):
    path = Path(args.playwright_host_profile)
    if not path.is_absolute() or existing_resolved(path) != str(path):
        raise InstallError("canonical host profile required")
    raw = read(path.parent, path.name, MAX_CONFIGURATION)
    if sha(raw) != args.playwright_host_profile_sha256:
        raise InstallError("host profile differs")
    value = unique_json(raw)
    if (not isinstance(value, dict) or set(value) != {"schema_version", "chrome_executable", "chrome_sha256", "process_parent"}
            or type(value["schema_version"]) is not int or value["schema_version"] != 1
            or not isinstance(value["chrome_sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", value["chrome_sha256"])):
        raise InstallError("unsupported host profile")
    if any(not isinstance(value[k], str) or not value[k] for k in ("chrome_executable", "process_parent")):
        raise InstallError("invalid host path")
    chrome, parent = Path(value["chrome_executable"]), Path(value["process_parent"])
    for p in (chrome, parent):
        if not p.is_absolute() or existing_resolved(p) != str(p) or p == bundle or bundle in p.parents:
            raise InstallError("canonical external host paths required")
    # File shape is observable even when the host sandbox denies execution.
    # This does not establish launch permission or browser compatibility.
    mode = chrome.stat(follow_symlinks=False).st_mode
    if (not stat.S_ISREG(mode) or not mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
            or sha(read(chrome.parent, chrome.name)) != value["chrome_sha256"]):
        raise InstallError("selected browser differs")
    handle = directory(parent); os.close(handle)
    return raw, value


def playwright_inputs(bundle, expected):
    raw = read(bundle, "CONTENTS.json", 1024 * 1024)
    if sha(raw) != expected:
        raise InstallError("official contents differ")
    contents = unique_json(raw)
    if (not isinstance(contents, dict) or set(contents) != {"format", "files"} or type(contents["format"]) is not int or contents["format"] != 1
            or not isinstance(contents["files"], dict) or not 1 <= len(contents["files"]) <= 512):
        raise InstallError("unsupported official contents")
    files, total = {}, 0
    for name, item in contents["files"].items():
        parts = relative(name)
        if name != "delivery.json" and parts[0] != "playwright": continue
        if not isinstance(item, dict) or set(item) != {"sha256", "bytes"} or type(item["bytes"]) is not int:
            raise InstallError("invalid official file record")
        data = read(bundle, name); total += len(data)
        if total > MAX_TOTAL or len(data) != item["bytes"] or sha(data) != item["sha256"]:
            raise InstallError("official payload differs")
        files[name] = data
    delivery = unique_json(files["delivery.json"])
    if not isinstance(delivery, dict) or delivery.get("format") != "meerkat-toolkit.official-consumers.v1" or delivery.get("role") != "service_connector":
        raise InstallError("unsupported official delivery")
    if not isinstance(delivery.get("components"), list) or any(not isinstance(v, dict) for v in delivery["components"]):
        raise InstallError("invalid official components")
    rows = [v for v in delivery["components"] if v.get("catalog_id") == "playwright"]
    if len(rows) != 1: raise InstallError("ambiguous official component")
    component = rows[0]
    if (not isinstance(component.get("source_revision"), str) or not re.fullmatch(r"[a-f0-9]{40}", component["source_revision"])
            or component.get("selected_operations") != PLAYWRIGHT_OPERATIONS or component.get("whole_endpoint_exposed") is not True
            or component.get("browser_distributed") is not False
            or any(component.get(k) != "playwright/" + leaf for k, leaf in (("package_json", "package.json"), ("package_lock", "package-lock.json"), ("closure", "closure.json"), ("configuration_template", "mcp.template.json")))):
        raise InstallError("unsupported official Playwright profile")
    template = {"mcpServers": {"playwright-isolated": {"command": "ABS_NODE_EXECUTABLE", "args": PLAYWRIGHT_ARGS,
        "cwd": "ABS_PROCESS_WORKSPACE", "env": {"PATH": "/usr/bin:/bin", "HOME": "ABS_PROCESS_HOME", "TMPDIR": "ABS_PROCESS_TMP", "PWTEST_SOCKETS_DIR": "ABS_SHORT_PROCESS_SOCKET_DIRECTORY"}}}}
    if unique_json(files["playwright/mcp.template.json"]) != template:
        raise InstallError("unsupported Playwright template")
    closure = unique_json(files["playwright/closure.json"])
    project = unique_json(files["playwright/package.json"]); lock = unique_json(files["playwright/package-lock.json"])
    if (not isinstance(closure, dict) or not isinstance(project, dict) or not isinstance(lock, dict)
            or not isinstance(closure.get("packages"), list)
            or not isinstance(lock.get("packages"), dict) or not isinstance(lock["packages"].get(""), dict)
            or any(not isinstance(row, dict) for row in closure["packages"])):
        raise InstallError("invalid official npm structures")
    if (set(project) != {"name", "version", "private", "dependencies"}
            or not isinstance(project.get("name"), str) or not isinstance(project.get("version"), str)
            or any(k in lock["packages"][""] for k in ("devDependencies", "optionalDependencies", "peerDependencies", "workspaces"))
            or sha(files["playwright/package.json"]) != closure["package_json_sha256"] or sha(files["playwright/package-lock.json"]) != closure["package_lock_sha256"]
            or project.get("dependencies") != {"@playwright/mcp": "0.0.83"} or project.get("private") is not True
            or type(lock.get("lockfileVersion")) is not int or lock["lockfileVersion"] != 3
            or lock.get("name") != project.get("name") or lock.get("version") != project.get("version")
            or lock["packages"][""].get("dependencies") != project["dependencies"]
            or len(closure["packages"]) != 3):
        raise InstallError("unsupported official npm project")
    versions = {"@playwright/mcp": "0.0.83", "playwright": "1.64.0-alpha-1790635538000", "playwright-core": "1.64.0-alpha-1790635538000"}
    graph, paths, archives = {}, set(), set()
    for row in closure["packages"]:
        if (not isinstance(row.get("id"), str) or not isinstance(row.get("path"), str)
                or type(row.get("bytes")) is not int or row["bytes"] <= 0
                or type(row.get("payload_files")) is not int or row["payload_files"] <= 0):
            raise InstallError("invalid official npm package record")
        package = row["id"]; loc = "node_modules/" + package
        if package not in versions or row["version"] != versions[package] or row["install_path"] != loc or loc in graph:
            raise InstallError("unsupported official npm selection")
        filename = row["path"]; parts = relative(filename)
        if len(parts) != 3 or parts[:2] != ("playwright", "archives") or filename in archives:
            raise InstallError("invalid official archive path")
        data = files[filename]; archive = npm_archive(data); meta = unique_json(archive["files"]["package.json"]); locked = lock["packages"][loc]
        if (not isinstance(meta, dict) or not isinstance(locked, dict)
                or not isinstance(meta.get("dependencies", {}), dict)
                or any(k in locked for k in ("devDependencies", "optionalDependencies", "peerDependencies"))
                or any(k in meta for k in ("optionalDependencies", "peerDependencies"))
                or archive["name"] != package or archive["version"] != row["version"] or archive["integrity"] != row["integrity"]
                or sha(data) != row["sha256"] or len(data) != row["bytes"] or len(archive["files"]) != row["payload_files"]
                or locked.get("version") != row["version"] or locked.get("integrity") != row["integrity"] or locked.get("resolved") != row["url"]
                or meta.get("dependencies", {}) != locked.get("dependencies", {})
                or any(v != versions.get(k) for k,v in meta.get("dependencies", {}).items())
                or any(locked.get(k) for k in ("link", "dev", "optional", "inBundle"))):
            raise InstallError("official npm closure differs")
        archives.add(filename);graph[loc]=(filename,archive);paths.update(loc+"/"+p for p in archive["files"])
    if set(lock["packages"]) != {"", *graph}:
        raise InstallError("extra npm lock selection")
    notices = {p for p in files if p.startswith("playwright/notices/")}
    if len(notices) != 7 or set(files) != {"delivery.json", "playwright/package.json", "playwright/package-lock.json", "playwright/closure.json", "playwright/mcp.template.json", "playwright/UPSTREAM-PROFILE.md", *archives, *notices}:
        raise InstallError("incomplete official payloads or notices")
    return files, {"rows":graph,"paths":paths}, component


def playwright_process(host, destination):
    parent = Path(host["process_parent"])
    if parent == destination or destination in parent.parents:
        raise InstallError("process parent must be outside installation")
    root = parent / ("p-" + sha(str(destination).encode())[:16])
    if root == destination or root in destination.parents:
        raise InstallError("process path overlaps installation")
    if len(str(root / "sockets/dashboard" / ("f"*16+".sock")).encode()) > 103:
        raise InstallError("process socket path too long")
    return root


def playwright_launch(host, destination, runtime, server):
    root = playwright_process(host, destination)
    replace = {"ABS_PLAYWRIGHT_INSTALL/node_modules/@playwright/mcp/cli.js":str(destination/"node_modules/@playwright/mcp/cli.js"),
               "ABS_CHROME_EXECUTABLE":host["chrome_executable"], "ABS_PROCESS_OUTPUT":str(root/"output")}
    return {"mcpServers": {server: {"command":runtime,"args":[replace.get(a,a) for a in PLAYWRIGHT_ARGS],"cwd":str(root/"workspace"),
        "env":{"PATH":"/usr/bin:/bin","HOME":str(root/"home"),"TMPDIR":str(root/"tmp"),"PWTEST_SOCKETS_DIR":str(root/"sockets")}}}}


def playwright_consumer(args):
    bundle = Path(existing_resolved(Path(args.official_bundle)))
    files, closure, component = playwright_inputs(bundle, args.contents_sha256)
    host_raw, host = playwright_host(args, bundle)
    runtime, npm = executable(args.node), executable(args.npm_cli)
    if args.official_describe:
        help_value={"profile":PLAYWRIGHT_PROFILE,"operations":PLAYWRIGHT_OPERATIONS,"effect":"unspecified","fields":[],"scope":"configuration_only","selectable_operations":False,
            "prerequisites":[{"kind":"whole_endpoint","instruction":"All Playwright endpoint tools are exposed. The five listed operations describe adopted coverage, not a tool filter. This profile accepts no custom fields."},
                {"kind":"host_runtime","instruction":"macOS arm64, selected Node 24.19.0/npm 11.12.1 and an existing host-pinned Chrome executable. Metadata does not establish browser compatibility."},
                {"kind":"host_environment","instruction":"Launch only with a host-controlled minimal environment, excluding unrelated PLAYWRIGHT_MCP_*, NODE_OPTIONS, NODE_PATH and proxy settings. The MCP env object does not clear inherited values."}],
            "unresolved_details":["The host owns browser lifetime, filesystem/network policy, snapshot-file access and cleanup after joining all children.","No logged-in state, cookies, credentials, remote browser or extension connection is imported. Isolation flags are not a security boundary."],
            "guide_text":files["playwright/UPSTREAM-PROFILE.md"].decode("utf-8")}
        return {"family":PLAYWRIGHT_FAMILY,"profiles":[{"profile":PLAYWRIGHT_PROFILE,"operations":PLAYWRIGHT_OPERATIONS,"effect":"unspecified"}],"help":help_value,
                "contents_sha256":args.contents_sha256,"target":"macos-arm64","scope":"official_metadata_only","whole_endpoint_exposed":True}
    raw = selected_configuration(args); value=unique_json(raw)
    if value != {"profile":PLAYWRIGHT_PROFILE,"operations":PLAYWRIGHT_OPERATIONS,"config":{}}:
        raise InstallError("unsupported Playwright configuration")
    if args.check_config_only:
        return {"profile":PLAYWRIGHT_PROFILE,"valid":True,"scope":"configuration_only","credential_environment_references":[]}
    import platform
    if sys.platform != "darwin" or platform.machine() != "arm64": raise InstallError("official Playwright setup requires macOS arm64")
    destination = existing_destination(args.destination,bundle) if args.verify_existing else new_destination(args.destination,bundle)
    process = playwright_process(host,destination)
    launch=playwright_launch(host,destination,runtime,args.server_name);entry=launch['mcpServers'][args.server_name]
    expected={"schema_version":1,"scope":"observed_software_installation_only","family":PLAYWRIGHT_FAMILY,"component":"playwright","profile":PLAYWRIGHT_PROFILE,
        "contents_sha256":args.contents_sha256,"inputs":{p:sha(v) for p,v in sorted(files.items())},"host_profile_sha256":sha(host_raw),"process_root":str(process),"process_destination_sha256":sha(str(destination).encode()),
        "command":[entry['command'],*entry['args']],"destination":str(destination),"mcp_configuration":launch,"whole_endpoint_exposed":True,
        "runtime_selection":{"node":runtime,"npm_cli":npm},"configuration":{"path":str(destination/'configuration/selected.json'),"sha256":sha(raw),"server_name":args.server_name,"validation":"configuration_only"}}
    fixed={"inputs/"+p:v for p,v in files.items()}
    fixed.update({"package.json":files['playwright/package.json'],"package-lock.json":files['playwright/package-lock.json'],"configuration/selected.json":raw,"configuration/host-profile.json":host_raw})
    fixed.update({p.removeprefix('playwright/'):v for p,v in files.items() if p.startswith('playwright/notices/') or p=='playwright/UPSTREAM-PROFILE.md'})
    if args.verify_existing:
        exact = set(fixed) | closure['paths'] | {'ready/installed.json','ready/mcp.json','user.npmrc','global.npmrc'}
        directories = {str(parent) for p in exact for parent in PurePosixPath(p).parents if str(parent) != '.'}
        logs = {label+suffix for label in ('node-runtime','npm-runtime','npm-cache-0','npm-cache-1','npm-cache-2','npm-install') for suffix in ('.stdout','.stderr')}
        for p,kind in existing_tree(destination, 4096).items():
            if p in ('npm-cache','tmp') or p.startswith(('npm-cache/','tmp/')):
                if kind not in ('file','directory'): raise InstallError("unexpected npm working link")
            elif p in exact or p in logs or p == 'node_modules/.package-lock.json':
                if kind != 'file': raise InstallError("unexpected Playwright file type")
            elif p not in directories or kind != 'directory':
                raise InstallError("unexpected Playwright installation path")
        for p in ('user.npmrc','global.npmrc'):
            if read(destination,p) != b'': raise InstallError("npm configuration differs")
        for p,v in fixed.items():
            if read(destination,p)!=v: raise InstallError("retained Playwright input differs")
        existing_tree(destination/'node_modules',1000)
        graph=verify_selected_npm(destination,closure)
        receipt_raw=read(destination,'ready/installed.json',MAX_CONFIGURATION);receipt=unique_json(receipt_raw)
        if not isinstance(receipt, dict) or type(receipt.get('schema_version')) is not int or receipt != expected or unique_json(read(destination,'ready/mcp.json'))!=launch:
            raise InstallError("Playwright publication differs")
        # Mutable serving files are outside this readback. Refuse a substituted
        # process directory without inspecting browser state or snapshots.
        if os.path.lexists(process):
            handle=directory(process);os.close(handle)
        return {"schema_version":1,"scope":"retained_payload_and_launch_binding_only","verified":True,"family":PLAYWRIGHT_FAMILY,"component":"playwright",
            "destination":str(destination),"contents_sha256":args.contents_sha256,"configuration_sha256":sha(raw),"command":expected['command'],
            "host_profile_sha256":sha(host_raw),"historical_receipt":{"sha256":sha(receipt_raw),"recorded_scope":expected['scope']},"retained_payloads":graph,
            "excluded":["No Node/npm/Playwright/Chrome execution or browser readiness check. Mutable process files, npm cache/logs/generated hidden lock and complete Chrome bundle integrity are excluded."],"custody":"Exclusive quiescent host custody is required."}
    # Reserve both new roots. Partial failure preserves them; never retry here.
    process.mkdir(mode=0o700)
    for name in ('workspace','output','home','tmp','sockets'): (process/name).mkdir(mode=0o700)
    destination.mkdir(mode=0o700)
    for p,v in fixed.items():
        target=destination/p;target.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
        with target.open('xb') as stream:stream.write(v)
    (destination/'tmp').mkdir(mode=0o700)
    env={'PATH':'/usr/bin:/bin','LANG':'C.UTF-8','TMPDIR':str(destination/'tmp'),'PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD':'1'}
    observed=unique_json(run([runtime,'-p','JSON.stringify({version:process.version,platform:process.platform,arch:process.arch})'],destination,env,'node-runtime'))
    if observed!={'version':'v24.19.0','platform':'darwin','arch':'arm64'}:raise InstallError("unsupported selected Node runtime")
    env=npm_environment(destination,runtime,env);env.update(npm_config_engine_strict='true',npm_config_bin_links='false')
    command=[runtime,npm,'--prefix',str(destination)]
    if run(command+['--version'],destination,env,'npm-runtime').decode('ascii').strip()!='11.12.1':raise InstallError("unsupported selected npm")
    for i,(p,_) in enumerate(closure['rows'].values()):run(command+['cache','add',str(destination/'inputs'/p),'--offline'],destination,env,'npm-cache-'+str(i))
    run(command+['ci','--offline','--ignore-scripts','--no-audit','--no-fund','--bin-links=false'],destination,env,'npm-install')
    existing_tree(destination/'node_modules',1000);verify_selected_npm(destination,closure)
    pending=destination/'pending-ready';pending.mkdir(mode=0o700)
    (pending/'installed.json').write_bytes(encoded(expected));(pending/'mcp.json').write_bytes(encoded(launch));pending.rename(destination/'ready')
    return expected


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--official-bundle", help="separately pinned official bundle with a supported target")
    selection.add_argument("--bundle", help="previously delivered library consumer bundle")
    selection.add_argument("--selected-node", help="explicitly selected artifact-backed Node MCP bundle")
    selection.add_argument("--selected-python", help="explicitly selected wheel-backed Python MCP bundle")
    selection.add_argument("--describe-node-build", action="store_true", help="inert host toolchain planning projection")
    selection.add_argument("--review-node-build", help="inert review of a checked selected Node snapshot")
    parser.add_argument("--node-build-profile")
    parser.add_argument("--node-build-profile-sha256")
    parser.add_argument("--contents-sha256", help="expected digest from the selected library or official delivery")
    parser.add_argument("--family", choices=("agno", "services", "activepieces"))
    parser.add_argument("--config", help="absolute selected nonsecret library or official configuration file")
    parser.add_argument("--config-sha256", help="expected SHA-256 of the exact configuration bytes")
    parser.add_argument("--server-name", help="explicit MCP configuration key, up to 128 ASCII letters, digits, dots, underscores or hyphens")
    parser.add_argument("--selection-sha256", help="operator-selected SHA-256 of the exact raw selected document")
    parser.add_argument("--document-kind", choices=(LOCAL_SOFTWARE_DOCUMENT,),
                        help="explicit selected Python/Node declaration.json form; omission selects activation.json")
    parser.add_argument("--component", help="exact component ID from the selected activation, or github/playwright in official mode")
    parser.add_argument("--validator", help="absolute trusted toolkit-node-plan or toolkit-python-plan executable")
    parser.add_argument("--playwright-host-profile")
    parser.add_argument("--playwright-host-profile-sha256")
    parser.add_argument("--official-describe", action="store_true", help="read fixed official route metadata only")
    parser.add_argument("--check-config-only", action="store_true", help="validate fixed official route configuration without installation")
    parser.add_argument("--destination", help="absolute new directory for installation, or existing directory for --verify-existing")
    parser.add_argument("--python", help="absolute selected Python 3.11 interpreter")
    parser.add_argument("--python-target", choices=tuple(SELECTED_PYTHON_TARGETS),
                        help="host-selected native CPython 3.11 target for --selected-python; defaults to macos-arm64")
    parser.add_argument("--node", help="absolute selected Node runtime")
    parser.add_argument("--npm-cli", help="absolute selected npm-cli.js, run with --node")
    parser.add_argument("--verify-existing", action="store_true", help="read back original selection; selected modes run only the trusted metadata validator, never install or repair")
    args = parser.parse_args()
    if args.review_node_build or args.describe_node_build:
        if (not all((args.node, args.node_build_profile, args.node_build_profile_sha256))
                or any((args.destination, args.validator, args.npm_cli, args.verify_existing, args.component,
                        args.document_kind, args.selection_sha256, args.config, args.python))):
            parser.error("build review requires only snapshot, Node and exact host profile")
        try:
            result = describe_node_build(args) if args.describe_node_build else node_build_review_bundle(args)
        except (OSError, ValueError, KeyError, TypeError, RecursionError, tarfile.TarError):
            print("selected build review unavailable", file=sys.stderr)
            return 2
        print(json.dumps(result, sort_keys=True))
        return 0
    if (args.node_build_profile is not None or args.node_build_profile_sha256 is not None) and (
            not args.selected_node or not all((args.node_build_profile, args.node_build_profile_sha256))):
        parser.error("build host profile requires selected Node mode and exact paired arguments")
    if args.document_kind is not None and not (args.selected_python or args.selected_node):
        parser.error("document-kind requires selected Python or Node mode")
    if args.python_target is not None and not args.selected_python:
        parser.error("python-target requires selected Python mode")
    configured = (args.config, args.config_sha256, args.server_name)
    if any(value is not None for value in configured):
        if not (args.bundle or args.official_bundle) or any(value is None for value in configured):
            parser.error("configuration requires library or official mode and exact config, config-sha256 and server-name together")
    if args.official_bundle:
        if (not args.contents_sha256 or args.component not in ("github", "playwright") or any((args.family, args.selection_sha256, args.validator, args.python))
                or sum((args.official_describe, args.check_config_only, args.verify_existing)) > 1
                or args.official_describe and (any(configured) or args.destination)
                or not args.official_describe and not all(configured)
                or not (args.official_describe or args.check_config_only) and not args.destination
                or args.check_config_only and args.destination):
            parser.error("official mode requires exact bundle/component and explicit action inputs")
        if args.component == "playwright":
            if not all((args.node,args.npm_cli,args.playwright_host_profile,args.playwright_host_profile_sha256)):
                parser.error("Playwright requires explicit host profile and Node/npm")
        elif any((args.node,args.npm_cli,args.playwright_host_profile,args.playwright_host_profile_sha256)):
            parser.error("GitHub does not accept browser/runtime overrides")
    elif args.playwright_host_profile or args.playwright_host_profile_sha256:
        parser.error("browser host profile requires official Playwright mode")
    elif args.official_describe or args.check_config_only or not args.destination:
        parser.error("metadata-only actions require official mode; installation requires destination")
    if args.official_bundle:
        pass
    elif args.selected_node:
        if (not all((args.selection_sha256, args.component, args.validator, args.node))
                or not args.verify_existing and not args.npm_cli
                or args.contents_sha256 or args.family or args.python):
            parser.error("selected Node mode requires selection digest, component, validator and Node; installation also requires npm")
    elif args.selected_python:
        if (not all((args.selection_sha256, args.component, args.validator, args.python))
                or args.contents_sha256 or args.family or args.node or args.npm_cli):
            parser.error("selected Python mode requires selection digest, component, validator and Python only")
    elif not args.contents_sha256 or not args.family or any((args.selection_sha256, args.component, args.validator)):
        parser.error("library mode requires contents digest and family")
    def cancelled(signum, frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, cancelled)
    try:
        if args.official_bundle:
            result = official_consumer(args)
        elif args.verify_existing and (args.selected_node or args.selected_python):
            result = verify_existing_selected(args)
        elif args.selected_node:
            result = install_selected_node(args)
        elif args.selected_python:
            result = install_selected_python(args)
        else:
            result = verify_existing(args) if args.verify_existing else install(args)
    except (OSError, EOFError, ValueError, KeyError, TypeError, StopIteration, RecursionError, subprocess.TimeoutExpired,
            zipfile.BadZipFile, tarfile.TarError, configparser.Error, csv.Error, KeyboardInterrupt):
        if args.official_describe or args.check_config_only:
            print("official metadata or configuration refused; no installation was attempted", file=sys.stderr)
        elif args.verify_existing:
            print("existing installation verification failed; no installation or repair was performed", file=sys.stderr)
        else:
            print("installation status unconfirmed; inspect the destination's ready/installed.json (or installed.json) and step logs before using or retrying it; keep incomplete installations unused and select a new directory", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
