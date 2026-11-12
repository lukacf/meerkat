#!/usr/bin/env python3
"""Install an explicitly selected offline consumer, or verify an existing library.

Uses ordinary pip/npm and an operator-selected CONTENTS.json digest. This is
software installation, not account authentication, agent setup or activation.
--verify-existing is inert, library-only readback without repair or execution.
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
import json
import os
from pathlib import Path, PurePosixPath
import re
import signal
import stat
import subprocess
import sys
import tarfile
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
        child = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err, start_new_session=True)
        try:
            code = child.wait(timeout=timeout)
            if code:
                raise InstallError("installer step failed: " + label)
        except BaseException:
            # A second caller signal must not abandon a still-running installer.
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
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
            finally:
                for signum, handler in handlers.items():
                    signal.signal(signum, handler)
            raise
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
    files, paths, total = {}, set(), 0
    with tarfile.open(fileobj=io.BytesIO(expanded), mode="r:") as archive:
        for index, member in enumerate(archive):
            if index >= MAX_NPM_MEMBERS:
                raise InstallError("too many npm archive members")
            clean = member.name.rstrip("/") if member.isdir() else member.name
            parts = relative(clean)
            if parts[0] != "package" or (not member.isdir() and len(parts) < 2):
                raise InstallError("npm archive must stay inside package")
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
            files[clean[len("package/"):]] = data
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


def selected_npm_closure(files: dict[str, bytes], plan: dict) -> dict:
    """Bind npm's concrete lock to the existing owner's pinned artifact closure."""
    project = unique_json(files["closure/package-json"])
    lock = unique_json(files["closure/package-lock"])
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
        if not path.startswith("closure/") or path in ("closure/package-json", "closure/package-lock"):
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
    entry = plan["entrypoint"]["executable"]
    relative(entry)
    if entry not in component["files"] or not entry.endswith((".js", ".mjs", ".cjs")):
        raise InstallError("selected Node entrypoint must be a retained JavaScript payload")
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
    return {"rows": rows, "archives": archives, "component_path": component_path,
            "entrypoint": entry, "paths": all_paths}


def verify_selected_npm(destination: Path, closure: dict) -> dict:
    """Require exact payload bytes and no extra executable files or symlinks."""
    graph = {}
    for path, (_, archive) in closure["rows"].items():
        for filename, raw in archive["files"].items():
            if read(destination, path + "/" + filename) != raw:
                raise InstallError("installed npm payload differs from retained archive")
        graph[path] = {"name": archive["name"], "version": archive["version"],
                       "integrity": archive["integrity"], "files": len(archive["files"])}
    actual = set()
    for root, dirs, filenames in os.walk(destination / "node_modules", followlinks=False):
        for child in dirs + filenames:
            if (Path(root) / child).is_symlink():
                raise InstallError("unexpected installed npm symlink")
        actual.update(str((Path(root) / file).relative_to(destination)) for file in filenames)
    if actual - {"node_modules/.package-lock.json"} != closure["paths"]:
        raise InstallError("installed npm file graph differs from closure")
    return graph


def install_selected_node(args: argparse.Namespace) -> dict:
    """An explicit trusted-local effect; this is not a native agent grant."""
    bundle = Path(args.selected_node).resolve(strict=True)
    destination = new_destination(args.destination, bundle)
    if not re.fullmatch(r"[a-f0-9]{64}", args.selection_sha256):
        raise InstallError("expected raw selection SHA-256 required")
    selection = read(bundle, "activation.json", 1024 * 1024)
    if sha(selection) != args.selection_sha256:
        raise InstallError("activation differs from operator-selected bytes")
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
    files = {"package.json": package, "activation.json": selection}
    for filename, raw in files.items():
        (frozen / filename).write_bytes(raw)
    (destination / "tmp").mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(destination / "tmp")}
    # The executable invokes ActivationManifest::validate on these exact frozen
    # documents. Never deserialize a claim that a prior plan was validated.
    plan = unique_json(run([validator, "--package", str(frozen / "package.json"),
                           "--selection", str(frozen / "activation.json"), "--component", args.component],
                          destination, env, "validate-selection"))
    if plan["schema_version"] != 1 or plan["component"] != args.component:
        raise InstallError("unsupported validator projection")
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
    observed = json.loads(run([runtime, "-p", "JSON.stringify([process.version,process.platform,process.arch])"],
                              destination, env, "node-runtime"))
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
    graph = verify_selected_npm(destination, closure)
    launch = [runtime, str(destination / closure["component_path"] / closure["entrypoint"]),
              *plan["entrypoint"]["arguments"]]
    configuration = {"mcpServers": {plan["mcp_name"]: {"command": launch[0], "args": launch[1:]}}}
    receipt = {"schema_version": 1, "scope": "observed_software_installation_only",
               "selection_sha256": args.selection_sha256, "manifest_digest": plan["manifest_digest"],
               "inputs": {key: sha(raw) for key, raw in sorted(files.items())}, "runtime": observed,
               "installed_graph": graph, "command": launch, "destination": str(destination),
               "mcp_configuration": configuration,
               "launch_environment": "Owned by the consuming host; this configuration does not clear ambient environment or filter tools."}
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
    expected_tags = {"-".join((a, b, c)) for a in pieces[-3].split(".")
                     for b in pieces[-2].split(".") for c in pieces[-1].split(".")}
    if (wheel.get_all("Wheel-Version") != ["1.0"]
            or wheel.get_all("Root-Is-Purelib") not in (["true"], ["false"])
            or set(wheel.get_all("Tag", [])) != expected_tags):
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
    scripts = {}
    if dist + "/entry_points.txt" in files:
        entries = configparser.ConfigParser(interpolation=None, strict=True)
        entries.optionxform = str
        entries.read_string(files[dist + "/entry_points.txt"].decode("utf-8"))
        if entries.defaults():
            raise InstallError("entry point defaults are unsupported")
        for group in ("console_scripts", "gui_scripts"):
            if not entries.has_section(group):
                continue
            for script, target in entries.items(group):
                if (not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", script) or script in scripts
                        or not re.fullmatch(r"[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*:[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*(?:\s+\[[A-Za-z0-9][A-Za-z0-9_.-]*(?:\s*,\s*[A-Za-z0-9][A-Za-z0-9_.-]*)*\])?", target, re.ASCII)):
                    raise InstallError("unsupported wheel script entry point")
                scripts[script] = target
    return {"name": package, "version": version, "filename": filename, "sha256": sha(raw),
            "files": files, "dist_info": dist, "scripts": scripts, "expanded_bytes": total}


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
    module_parts = [*parts[:-1], parts[-1].removesuffix(".py")]
    if (not entry.endswith(".py") or parts[-1] == "__init__.py" or entry not in primary["files"]
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
            or any(stem + "/__init__" + suffix in primary["files"] for suffix in [".py", *extensions])):
        raise InstallError("package or extension shadows selected module source")
    if any(path.split("/")[0].split(".")[0] == namespace for wheel in wheels[1:] for path in wheel["files"]):
        raise InstallError("selected module namespace has multiple wheel owners")
    return {"wheels": wheels, "versions": {key: value[0] for key, value in identities.items()},
            "module": plan["module"]}


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


def install_selected_python(args: argparse.Namespace) -> dict:
    """Realize one concrete selection; native authority and launch stay outside."""
    bundle = Path(args.selected_python).resolve(strict=True)
    destination = new_destination(args.destination, bundle)
    if not re.fullmatch(r"[a-f0-9]{64}", args.selection_sha256):
        raise InstallError("expected raw selection SHA-256 required")
    selection = read(bundle, "activation.json", 1024 * 1024)
    if sha(selection) != args.selection_sha256:
        raise InstallError("activation differs from operator-selected bytes")
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
    files = {"package.json": package, "activation.json": selection}
    for filename, raw in files.items():
        (frozen / filename).write_bytes(raw)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(destination / "tmp"),
           "PIP_CONFIG_FILE": os.devnull, "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1",
           "PIP_NO_INDEX": "1", "PIP_NO_CACHE_DIR": "1"}
    plan = unique_json(run([validator, "--package", str(frozen / "package.json"),
                           "--selection", str(frozen / "activation.json"), "--component", args.component],
                          destination, env, "validate-selection"))
    if plan["schema_version"] != 1 or plan["component"] != args.component:
        raise InstallError("unsupported validator projection")
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
    probe = "import importlib.machinery,json,platform,sys; print(json.dumps({'runtime':[sys.version,sys.version_info[:2],sys.platform,platform.machine(),sys.implementation.name],'stdlib':sorted(sys.stdlib_module_names),'extensions':importlib.machinery.EXTENSION_SUFFIXES}))"
    observed = unique_json(run([runtime, "-I", "-S", "-B", "-c", probe], destination, env, "python-runtime"))
    if observed["runtime"][1:] != [[3, 11], "darwin", "arm64", "cpython"]:
        raise InstallError("selected wheel profile requires CPython 3.11 on macOS arm64")
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
    launch = [python, "-I", "-B", "-m", closure["module"], *plan["entrypoint"]["arguments"]]
    configuration = {"mcpServers": {plan["mcp_name"]: {"command": launch[0], "args": launch[1:]}}}
    receipt = {"schema_version": 1, "scope": "observed_software_installation_only",
               "selection_sha256": args.selection_sha256, "manifest_digest": plan["manifest_digest"],
               "inputs": {key: sha(raw) for key, raw in sorted(files.items())}, "runtime": observed["runtime"],
               "installed_graph": graph, "bootstrap_distributions": bootstrap, "verification": payloads,
               "command": launch, "destination": str(destination), "mcp_configuration": configuration,
               "launch_environment": "Owned by the consuming host; this configuration does not clear ambient environment, supply external executables or filter tools.",
               "installer_generated_files": "Recorded hashes, not retained wheel payloads; module launch bypasses generated console scripts, whose optional extras and usability are not asserted."}
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
    return {"schema_version": 1, "scope": "retained_payload_and_launch_binding_only", "verified": True,
            "family": args.family, "destination": str(destination), "contents_sha256": args.contents_sha256,
            "historical_receipt": {"sha256": sha(receipt_raw), "recorded_scope": receipt["scope"]},
            "configuration_sha256": sha(configuration) if configuration is not None else None,
            "command": command, "selected_runtime_path": runtime, "retained_payloads": proof,
            "excluded": ["Runtime executable bytes, compatibility and prior probe claims are not verified.",
                         "Python pip/setuptools bootstrap trees, their known startup file, generated metadata/console wrappers and source-associated pyc bytes are not verified.",
                         "npm-generated hidden lock metadata is not verified.",
                         "No parser, MCP server, authentication, account access, grant or native activation was executed or established."],
            "custody": "Point-in-time observation of operator-controlled directories; not a filesystem sandbox or a claim the environment is safe to execute."}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--bundle", help="previously delivered library consumer bundle")
    selection.add_argument("--selected-node", help="explicitly selected artifact-backed Node MCP bundle")
    selection.add_argument("--selected-python", help="explicitly selected wheel-backed Python MCP bundle")
    parser.add_argument("--contents-sha256", help="expected digest from the selected library delivery")
    parser.add_argument("--family", choices=("agno", "services", "activepieces"))
    parser.add_argument("--config", help="absolute selected nonsecret library configuration file")
    parser.add_argument("--config-sha256", help="expected SHA-256 of the exact configuration bytes")
    parser.add_argument("--server-name", help="explicit MCP configuration key, up to 128 ASCII letters, digits, dots, underscores or hyphens")
    parser.add_argument("--selection-sha256", help="operator-selected SHA-256 of raw activation.json")
    parser.add_argument("--component", help="exact component ID from the selected activation")
    parser.add_argument("--validator", help="absolute trusted toolkit-node-plan or toolkit-python-plan executable")
    parser.add_argument("--destination", required=True, help="absolute new directory for installation, or existing directory for --verify-existing")
    parser.add_argument("--python", help="absolute selected Python 3.11 interpreter")
    parser.add_argument("--node", help="absolute selected Node runtime")
    parser.add_argument("--npm-cli", help="absolute selected npm-cli.js, run with --node")
    parser.add_argument("--verify-existing", action="store_true", help="inert library-only readback against original selected inputs; no installation or repair")
    args = parser.parse_args()
    if args.verify_existing and not args.bundle:
        parser.error("verify-existing requires library mode")
    configured = (args.config, args.config_sha256, args.server_name)
    if any(value is not None for value in configured):
        if not args.bundle or any(value is None for value in configured):
            parser.error("configuration requires library mode and exact config, config-sha256 and server-name together")
    if args.selected_node:
        if (not all((args.selection_sha256, args.component, args.validator, args.node, args.npm_cli))
                or args.contents_sha256 or args.family or args.python):
            parser.error("selected Node mode requires selection digest, component, validator, Node and npm only")
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
        if args.selected_node:
            result = install_selected_node(args)
        elif args.selected_python:
            result = install_selected_python(args)
        else:
            result = verify_existing(args) if args.verify_existing else install(args)
    except (OSError, EOFError, ValueError, KeyError, TypeError, StopIteration, subprocess.TimeoutExpired,
            zipfile.BadZipFile, tarfile.TarError, configparser.Error, csv.Error, KeyboardInterrupt):
        if args.verify_existing:
            print("existing installation verification failed; no installation or repair was performed", file=sys.stderr)
        else:
            print("installation status unconfirmed; inspect the destination's ready/installed.json (or installed.json) and step logs before using or retrying it; keep incomplete installations unused and select a new directory", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
