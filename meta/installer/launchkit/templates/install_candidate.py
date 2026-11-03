#!/usr/bin/env python3
"""Install an explicitly host-selected candidate; never imply public release trust.

The copied integrity reader rechecks previously accepted artifacts. This adapter
does not verify signatures or confer admission on a tuple or local SDK wheel.
"""
from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from email.parser import BytesParser
import gzip
import hashlib
import importlib.util
import inspect
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import stat
import subprocess
import tarfile
import tempfile
import time
import zipfile

if __package__ == "launchkit.templates":
    from .. import runtime_candidates as _candidate_reader
    from ..runtime_candidates import (
        ArtifactFile, CandidateArtifactError, CandidateBinding, read_candidate_artifacts,
        MAX_ARCHIVE_BYTES, MAX_BINARY_BYTES, MAX_TAR_OVERHEAD, MAX_METADATA_BYTES,
        MAX_WHEEL_BYTES,
    )
else:
    import runtime_candidates as _candidate_reader
    from runtime_candidates import (
        ArtifactFile, CandidateArtifactError, CandidateBinding, read_candidate_artifacts,
        MAX_ARCHIVE_BYTES, MAX_BINARY_BYTES, MAX_TAR_OVERHEAD, MAX_METADATA_BYTES,
        MAX_WHEEL_BYTES,
    )

ROOT = Path(__file__).resolve().parent
BINARIES = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}
SELECTION_FIELDS = {"format", "tuple_path", "artifact_root", "accepted"}
BINDING_FIELDS = set(CandidateBinding.__dataclass_fields__)
OVERRIDES = ("MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_GATEWAY_BIN", "META_FORGE_RPC_GATEWAY_BIN")
STDLIB_CHECK_TIMEOUT = 10
BYTECODE_BATCH_FILES = 64
BYTECODE_BATCH_BYTES = 4 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise CandidateArtifactError(message)


def digest(payload):
    return hashlib.sha256(payload).hexdigest()


def unique_json(payload):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "Candidate installation JSON has duplicate keys.")
            result[key] = value
        return result
    def nonfinite(value):
        raise CandidateArtifactError("Candidate installation JSON contains a non-finite value.")
    try:
        result = json.loads(payload, object_pairs_hook=unique, parse_constant=nonfinite)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise CandidateArtifactError("Candidate installation JSON is invalid.") from error
    require(isinstance(result, dict), "Candidate installation JSON must be an object.")
    return result


def safe_path(value):
    require(isinstance(value, (str, Path)), "Candidate installation path is invalid.")
    path = Path(value)
    require(path.is_absolute() and str(path) == str(value) and ".." not in path.parts,
        "Candidate installation paths must be absolute canonical paths.")
    for parent in (path, *path.parents):
        require(not parent.is_symlink(), "Candidate installation paths must not contain links.")
    return path


def fingerprint(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
        info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def read_owned(path, limit, *, private=False, empty=False):
    path = safe_path(path)
    before = path.lstat()
    require(stat.S_ISREG(before.st_mode) and before.st_uid == os.getuid()
        and before.st_nlink == 1 and not before.st_mode & (0o077 if private else 0o022)
        and (0 if empty else 1) <= before.st_size <= limit,
        "Candidate installation requires bounded owned ordinary files.")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as handle:
        opened = os.fstat(handle.fileno())
        require(fingerprint(before) == fingerprint(opened), "Candidate installation input changed while opening.")
        data = handle.read(limit + 1)
        require(len(data) == opened.st_size and len(data) <= limit
            and fingerprint(opened) == fingerprint(os.fstat(handle.fileno()))
            and fingerprint(opened) == fingerprint(path.lstat()),
            "Candidate installation input changed while reading.")
    return data


def owned_directory(path, *, private=False):
    path = safe_path(path)
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()
        and not info.st_mode & (0o077 if private else 0o022),
        "Candidate installation requires an owned ordinary directory.")
    return path


def platform_target():
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "AMD64": "x86_64"}.get(platform.machine())
    system = platform.system()
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(system)
    require(bool(arch and suffix), "Candidate native tools require macOS or glibc Linux on Intel or ARM64.")
    if system == "Linux":
        libc, version = platform.libc_ver()
        try:
            compatible = libc == "glibc" and tuple(int(part) for part in version.split(".")[:2]) >= (2, 31)
        except ValueError:
            compatible = False
        require(compatible, "Candidate native tools require glibc 2.31 or newer.")
    return f"{arch}-{suffix}"


@dataclass(frozen=True)
class CandidateSelection:
    path: Path
    root: Path
    tuple_path: Path
    artifact_root: Path
    accepted: CandidateBinding
    artifacts: object


def _recheck_selection_artifacts(context):
    # An older standalone reader remains usable without explicit reuse. Its
    # own class and full-reader contract cannot be replaced by a host proof.
    recheck = getattr(_candidate_reader, "recheck_candidate_artifacts", None)
    require(callable(recheck), "This candidate reader does not support explicit artifact reuse.")
    return recheck(context.artifacts, tuple_path=context.tuple_path,
        accepted=context.accepted, artifact_root=context.artifact_root)


def load_candidate_selection(selection_path=None, *, expected_native_version=None,
        expected_platform=None, gateway_override=None, verified_artifacts=None):
    require(not gateway_override and not any(os.environ.get(name) for name in OVERRIDES),
        "Candidate installation does not admit gateway overrides.")
    path = safe_path(Path(selection_path).absolute() if selection_path is not None else ROOT / "runtime-candidate.json")
    require(path.name == "runtime-candidate.json", "The compiler-owned candidate selection filename is required.")
    root = owned_directory(path.parent)
    value = unique_json(read_owned(path, MAX_METADATA_BYTES))
    require(set(value) == SELECTION_FIELDS and value["format"] == "forge-runtime-candidate-selection-v1"
        and isinstance(value["accepted"], dict) and set(value["accepted"]) == BINDING_FIELDS,
        "The compiler-owned candidate selection has an invalid shape.")
    binding = CandidateBinding(**value["accepted"])
    deployment = unique_json(read_owned(root / "deployment.json", MAX_METADATA_BYTES))
    require(deployment.get("runtimeCandidate") == {"source": "signed-candidate",
        "selection": "runtime-candidate.json", "tuple_sha256": binding.tuple_sha256},
        "The protected deployment does not select this candidate binding.")
    require(deployment.get("mobkitVersion") == binding.native_version
        and binding.native_version == (expected_native_version or deployment.get("mobkitVersion"))
        and binding.platform == (expected_platform or platform_target())
        and binding.core_version == "0.8.51",
        "The candidate SDK/native version, Core or local platform differs from the deployment.")
    tuple_path, artifact_root = safe_path(value["tuple_path"]), safe_path(value["artifact_root"])
    if verified_artifacts is None:
        artifacts = read_candidate_artifacts(tuple_path, accepted=binding, artifact_root=artifact_root)
    else:
        context = CandidateSelection(path, root, tuple_path, artifact_root, binding, verified_artifacts)
        artifacts = _recheck_selection_artifacts(context)
    return CandidateSelection(path, root, tuple_path, artifact_root, binding, artifacts)


def binding_fields(context):
    identity = context.accepted
    return {"candidate_binding": asdict(identity), "candidate_tuple_sha256": identity.tuple_sha256,
        "sdk_wheel_sha256": identity.sdk_wheel_sha256, "source_sha": identity.source_sha,
        "native_run_id": identity.native_run_id, "native_run_attempt": identity.native_run_attempt}


def checked_wheel(context):
    wheel = context.artifacts.sdk.wheel
    data = read_owned(wheel.path, MAX_WHEEL_BYTES)
    require(len(data) == wheel.size and digest(data) == wheel.sha256 == context.accepted.sdk_wheel_sha256,
        "The selected SDK wheel changed after candidate verification.")
    return data


def sdk_location(context, sdk_python=None, site_packages=None):
    return _sdk_location(context.root, sdk_python, site_packages)


def _sdk_location(root, sdk_python=None, site_packages=None):
    python = Path(sdk_python) if sdk_python is not None else root / ".venv/bin/python"
    require(python == root / ".venv/bin/python", "Candidate SDK installation uses the deployment's selected virtual environment.")
    venv = owned_directory(root / ".venv")
    if site_packages is None:
        # Before Python 3.14, -S can leave sys.prefix at the base interpreter.
        # Explicit scheme variables preserve this owned venv without allowing
        # site startup, .pth files or sitecustomize to execute installed code.
        result = subprocess.run([str(python), "-I", "-S", "-c",
            "import json, sysconfig, sys; print(json.dumps(sysconfig.get_paths(scheme='posix_venv' if 'posix_venv' in sysconfig.get_scheme_names() else 'posix_prefix', vars={'base': sys.argv[1], 'platbase': sys.argv[1]})))",
            str(venv)],
            check=True, capture_output=True, text=True, timeout=STDLIB_CHECK_TIMEOUT)
        paths = unique_json(result.stdout)
        require(paths.get("purelib") == paths.get("platlib"), "Candidate pure Python SDK installation paths differ.")
        site_packages = paths.get("purelib")
    site = owned_directory(safe_path(site_packages))
    require(site.is_relative_to(venv), "The installed candidate SDK must remain inside the selected virtual environment.")
    return python, site, venv / "forge-sdk.receipt.json"


def _selected_command_outcome(execute_command, argv, *, input=None, pip=False):
    """Use the trusted existing process owner without admitting its output."""
    require(callable(execute_command), "Selected interpreter commands need their original executor.")
    outcome = execute_command(argv, input=input, pip=pip)
    if inspect.isawaitable(outcome):
        if inspect.iscoroutine(outcome):
            outcome.close()
        raise CandidateArtifactError("Selected interpreter commands need a synchronous executor.")
    require(type(outcome) is dict and set(outcome) == {"returncode", "stdout", "stderr"}
        and type(outcome["returncode"]) is int
        and type(outcome["stdout"]) is bytes and type(outcome["stderr"]) is bytes,
        "The selected interpreter owner did not retain exact exited streams.")
    require(outcome["returncode"] == 0,
        f"The selected interpreter command failed (exit {outcome['returncode']}). Inspect the private, redacted logs.")
    return outcome


@contextmanager
def _selected_staging(prefix, root, execute_command):
    """Keep this attempt's bytes if its original child cleanup is unconfirmed."""
    if execute_command is None:
        with tempfile.TemporaryDirectory(prefix=prefix, dir=root / ".venv") as temp:
            yield temp
        return
    temp = tempfile.TemporaryDirectory(prefix=prefix, dir=root / ".venv")
    try:
        yield temp.name
    except BaseException as error:
        if getattr(error, "_forge_retain_selected_staging", False):
            # Retain only this private attempt. A GC finalizer must not remove
            # files still needed by the original unreconciled process handle.
            temp._finalizer.detach()
        else:
            temp.cleanup()
        raise
    else:
        temp.cleanup()


def _bytecode_request(path, source_path, source):
    data = read_owned(path, MAX_WHEEL_BYTES)
    optimize = 2 if ".opt-2." in path.name else 1 if ".opt-1." in path.name else 0
    return {"cache": base64.b64encode(data).decode(),
        "source": base64.b64encode(source).decode(), "filename": str(source_path), "optimize": optimize}


def _check_bytecode_batch(rows, python, *, execute_command=None):
    # The installed Python, not the caller, owns bytecode format and compilation.
    # Pass the already owned/read bytes; the helper reads no SDK or provider files.
    script = """import base64, importlib.util, json, marshal, sys
request = json.load(sys.stdin)
rows = request if isinstance(request, list) else [request]
valid = True
for v in rows:
    data, source = base64.b64decode(v['cache']), base64.b64decode(v['source'])
    row_valid = False
    try:
        row_valid = len(data) > 16 and data[:4] == importlib.util.MAGIC_NUMBER and marshal.loads(data[16:]) == compile(source, v['filename'], 'exec', dont_inherit=True, optimize=v['optimize'])
    except (ValueError, TypeError, EOFError, SyntaxError):
        pass
    valid = valid and row_valid
print(json.dumps({'valid': valid}))
"""
    # Preserve the single-file executor request used by the original caller.
    payload = json.dumps(rows[0] if len(rows) == 1 else rows)
    if execute_command is None:
        result = subprocess.run([str(python), "-I", "-S", "-c", script], check=True,
            capture_output=True, text=True, input=payload, timeout=STDLIB_CHECK_TIMEOUT)
        stdout = result.stdout
    else:
        stdout = _selected_command_outcome(execute_command,
            [str(python), "-I", "-S", "-c", script], input=payload.encode("utf-8"))["stdout"]
    require(unique_json(stdout) == {"valid": True},
        "Installed SDK bytecode differs from its exact source payload or selected interpreter.")


def check_bytecode(path, source_path, source, python, *, execute_command=None):
    _check_bytecode_batch([_bytecode_request(path, source_path, source)], python,
        execute_command=execute_command)


def _check_bytecodes(entries, python, *, execute_command=None):
    rows, size = [], 0
    for path, source_path, source in entries:
        row = _bytecode_request(path, source_path, source)
        row_size = len(json.dumps(row).encode("utf-8"))
        if rows and (len(rows) >= BYTECODE_BATCH_FILES or size + row_size > BYTECODE_BATCH_BYTES):
            _check_bytecode_batch(rows, python, execute_command=execute_command)
            rows, size = [], 0
        rows.append(row)
        size += row_size
    if rows:
        # One larger file retains its original MAX_WHEEL_BYTES allowance.
        _check_bytecode_batch(rows, python, execute_command=execute_command)


def _compare_sdk_payload(site, python, package_name, expected, *, sizes=None, strict=False,
        cache_limits=None, execute_command=None):
    package = owned_directory(site / package_name)
    actual, caches = set(), []
    cache_bytes = 0
    directories = {str(Path(name).parent) for name in expected}
    directories |= {str(parent) for name in expected for parent in Path(name).parents
                    if str(parent) != "."}
    for path in package.rglob("*"):
        require(not path.is_symlink(), "Installed SDK package contains a link.")
        if path.is_dir():
            owned_directory(path)
            if strict:
                name = path.relative_to(site).as_posix()
                require(name in directories or path.name == "__pycache__"
                    and path.parent.relative_to(site).as_posix() in directories,
                    "Installed SDK payload has an unexpected directory.")
        else:
            name = path.relative_to(site).as_posix()
            if path.suffix == ".pyc" and path.parent.name == "__pycache__":
                if cache_limits is not None:
                    cache_bytes += path.lstat().st_size
                    require(len(caches) < cache_limits[0] and cache_bytes <= cache_limits[1],
                        "Installed SDK bytecode exceeds its bounded inventory.")
                caches.append(path)
            else:
                if strict:
                    require(name in expected, "Installed SDK payload inventory differs from the exact wheel.")
                actual.add(name)
    require(actual == set(expected), "Installed SDK payload inventory differs from the exact wheel.")
    sources = {}
    for name, hashed in expected.items():
        data = read_owned(site / name, MAX_WHEEL_BYTES, empty=True)
        require(digest(data) == hashed and (sizes is None or len(data) == sizes[name]),
            "Installed SDK payload differs from the exact source wheel.")
        sources[name] = data
    def selected_caches():
        for path in caches:
            try:
                source_path = Path(importlib.util.source_from_cache(str(path)))
            except ValueError as error:
                raise CandidateArtifactError("Installed SDK has an unexpected bytecode path.") from error
            name = source_path.relative_to(site).as_posix()
            require(name in sources, "Installed SDK bytecode has no exact source owner.")
            yield path, source_path, sources[name]
    _check_bytecodes(selected_caches(), python, execute_command=execute_command)


def _sdk_metadata_directory(site, prefix, distribution_pattern):
    distributions = [path.name for path in site.iterdir()
        if re.fullmatch(distribution_pattern, path.name)]
    require(distributions == [prefix], "Installed SDK has an unexpected or duplicate distribution identity.")
    return owned_directory(site / prefix)


def _compare_sdk_metadata(metadata_dir, expected, *, original_bytes=None,
        nested=False, limit=MAX_METADATA_BYTES):
    metadata = {}
    for name, (hashed, size) in expected.items():
        value = read_owned(metadata_dir / name, limit, empty=nested)
        require(digest(value) == hashed and len(value) == size
            and (original_bytes is None or value == original_bytes[name]),
            "Installed SDK distribution metadata differs from its selected wheel.")
        metadata[name] = hashed
    generated = {"RECORD", "INSTALLER", "REQUESTED", "direct_url.json"}
    if not nested:
        allowed = {"METADATA", "WHEEL", "top_level.txt"} | generated
        require({path.name for path in metadata_dir.iterdir()} <= allowed,
            "Installed SDK has unexpected distribution metadata.")
        for path in metadata_dir.iterdir():
            read_owned(path, MAX_METADATA_BYTES, empty=True)
    else:
        directories = {parent.as_posix() for name in expected
                       for parent in PurePosixPath(name).parents if parent.as_posix() != "."}
        for path in metadata_dir.rglob("*"):
            require(not path.is_symlink(), "Installed SDK metadata contains a link.")
            name = path.relative_to(metadata_dir).as_posix()
            if path.is_dir():
                owned_directory(path)
                require(name in directories, "Installed SDK has unexpected distribution metadata.")
            else:
                require(name in expected or name in generated,
                    "Installed SDK has unexpected distribution metadata.")
                read_owned(path, limit if name in expected else MAX_METADATA_BYTES, empty=True)
    return metadata


def sdk_proof(context, site, python):
    wheel = checked_wheel(context)
    expected = dict(context.artifacts.sdk.payload)
    _compare_sdk_payload(site, python, "meerkat_mobkit", expected)
    prefix = f"meerkat_mobkit-{context.accepted.native_version}.dist-info"
    metadata_dir = _sdk_metadata_directory(site, prefix, r"(?i)meerkat[-_]mobkit-.*\.(dist-info|egg-info)")
    with zipfile.ZipFile(io.BytesIO(wheel)) as archive:
        values = {name: archive.read(prefix + "/" + name)
                  for name in ("METADATA", "WHEEL", "top_level.txt")}
    metadata = _compare_sdk_metadata(metadata_dir,
        {name: (digest(value), len(value)) for name, value in values.items()}, original_bytes=values)
    return {"format": "forge-installed-candidate-sdk-v1", "source": "local-build-from-exact-frozen-source",
        "signed": False, "published": False, "version": context.accepted.native_version,
        "site_packages": str(site), "payload_sha256": expected, "metadata_sha256": metadata,
        **binding_fields(context)}


def _sdk_fact_inventory(records, *, package=None, max_files, max_total):
    require(type(records) is tuple and 0 < len(records) <= max_files,
        "Installed SDK comparison needs a bounded explicit inventory.")
    values = {}
    for row in records:
        require(type(row) is tuple and len(row) == 3, "Installed SDK comparison has an invalid inventory entry.")
        name, hashed, size = row
        require(isinstance(name, str) and name and name not in values
            and isinstance(hashed, str) and re.fullmatch(r"[a-f0-9]{64}", hashed)
            and type(size) is int and 0 <= size <= MAX_WHEEL_BYTES,
            "Installed SDK comparison has an invalid inventory entry.")
        path = PurePosixPath(name)
        allowed_name = (name.startswith(package + "/") and not name.endswith(".pth")
                        if package else name in {"METADATA", "WHEEL", "top_level.txt"}
                        or name.startswith("licenses/") and len(path.parts) > 1)
        require(not path.is_absolute() and path.as_posix() == name
            and not any(part in ("", ".", "..") for part in path.parts)
            and not any(ord(char) < 32 or char in "\\:" for char in name)
            and allowed_name,
            "Installed SDK comparison requires ordinary canonical package and metadata paths.")
        values[name] = hashed, size
    require(len({name.casefold() for name in values}) == len(values)
        and sum(size for _, size in values.values()) <= max_total,
        "Installed SDK comparison has a duplicate or oversized inventory.")
    return values


def compare_installed_meta_toolkit_wheels(meta, toolkit, *, deployment_root,
        site_packages, sdk_python, execute_command=None):
    """Read back explicit installed SDK bytes and return the original held facts.

    The caller obtains these facts from check_meta_toolkit_wheels and retains
    responsibility for that prior byte check. Constructing facts or comparing
    them here is not provenance, selection, a receipt or native compatibility.
    Both filesystem locations and the selected interpreter must be explicit;
    only existing bytecode checks may run that interpreter. No SDK is imported,
    installed or admitted, and no dependency closure is inferred. This checks
    the two selected package inventories, not the whole site-packages environment.
    """
    if __package__ == "launchkit.templates":
        from ..sdk_wheel_integrity import (WheelIntegrityFacts, MAX_WHEEL_FILES, MAX_WHEEL_TOTAL,
            SUPPORTED_META_VERSIONS, SUPPORTED_TOOLKIT_VERSIONS)
    else:
        from sdk_wheel_integrity import (WheelIntegrityFacts, MAX_WHEEL_FILES, MAX_WHEEL_TOTAL,
            SUPPORTED_META_VERSIONS, SUPPORTED_TOOLKIT_VERSIONS)
    facts = (meta, toolkit)
    require(execute_command is None or callable(execute_command),
        "Installed SDK comparison needs its original selected interpreter executor.")
    selected = []
    required = ({"meerkat_meta/builder.py", "meerkat_meta/runtime.py", "meerkat_meta/_transport.py"},
                {"meerkat_toolkit/application_runtime.py", "meerkat_toolkit/host_control.py"})
    for item, (distribution, versions), modules in zip(facts,
            (("meerkat-meta", SUPPORTED_META_VERSIONS),
             ("meerkat-toolkit", SUPPORTED_TOOLKIT_VERSIONS)), required):
        require(type(item) is WheelIntegrityFacts and item.distribution == distribution
            and type(item.version) is str and item.version in versions and isinstance(item.sha256, str)
            and re.fullmatch(r"[a-f0-9]{64}", item.sha256)
            and type(item.size) is int and 0 < item.size <= MAX_WHEEL_BYTES,
            "Installed SDK comparison requires the exact held Meta/Toolkit facts.")
        package = distribution.replace("-", "_")
        payload = _sdk_fact_inventory(item.source_payload, package=package,
            max_files=MAX_WHEEL_FILES, max_total=MAX_WHEEL_TOTAL)
        metadata = _sdk_fact_inventory(item.metadata,
            max_files=MAX_WHEEL_FILES, max_total=MAX_WHEEL_TOTAL)
        require(modules <= payload.keys() and {"METADATA", "WHEEL"} <= metadata.keys()
            and len(payload) + len(metadata) + 1 <= MAX_WHEEL_FILES
            and sum(size for _, size in (*payload.values(), *metadata.values())) <= MAX_WHEEL_TOTAL,
            "Installed SDK comparison has an incomplete or oversized SDK inventory.")
        selected.append((package, item.version, payload, metadata))
    require(sdk_python is not None and site_packages is not None,
        "Installed SDK comparison requires explicit site-packages and interpreter paths.")
    root = owned_directory(safe_path(deployment_root))
    python, site, _ = _sdk_location(root, sdk_python, site_packages)
    command_options = {} if execute_command is None else {"execute_command": execute_command}
    for package, version, payload, metadata in selected:
        _compare_sdk_payload(site, python, package,
            {name: hashed for name, (hashed, _) in payload.items()},
            sizes={name: size for name, (_, size) in payload.items()}, strict=True,
            cache_limits=(MAX_WHEEL_FILES, MAX_WHEEL_TOTAL), **command_options)
        pattern = r"(?i)" + r"[-_.]+".join(package.split("_")) + r"(?:-.*)?\.(dist-info|egg-info)"
        directory = _sdk_metadata_directory(site, f"{package}-{version}.dist-info", pattern)
        _compare_sdk_metadata(directory, metadata, nested=True, limit=MAX_WHEEL_BYTES)
    return facts


def install_meta_toolkit_wheels(meta_path, toolkit_path, *, meta, toolkit,
        deployment_root, site_packages, sdk_python, require_current,
        dependency_requirements=None, dependency_wheels=None, execute_command=None):
    """Install caller-selected exact wheels within the existing owned venv.

    The synchronous caller fence owns selection and installation permission.
    This joins byte checking, hash-pinned local pip installation and installed
    readback; it creates no receipt, dependency closure or native admission.
    An optional caller-selected dependency batch is checked before any install,
    then installed from those held bytes and followed by another SDK readback.
    Failures can leave partial installed changes and are not rolled back.
    """
    if __package__ == "launchkit.templates":
        from ..sdk_wheel_integrity import check_meta_toolkit_wheels
    else:
        from sdk_wheel_integrity import check_meta_toolkit_wheels
    require(callable(require_current), "SDK installation needs its original synchronous owner fence.")
    require(execute_command is None or callable(execute_command),
        "SDK installation needs its original selected interpreter executor.")
    command_options = {} if execute_command is None else {"execute_command": execute_command}

    def current():
        result = require_current()
        if inspect.isawaitable(result):
            if inspect.iscoroutine(result):
                result.close()
            raise CandidateArtifactError("SDK installation needs a synchronous owner fence.")
        require(result is not False, "The selected SDK installation owner is no longer current.")

    current()
    require((dependency_requirements is None) == (dependency_wheels is None),
        "SDK installation requires both selected dependency inputs or neither.")
    require(site_packages is not None and sdk_python is not None,
        "SDK installation requires explicit site-packages and interpreter paths.")
    root = owned_directory(safe_path(deployment_root))
    data = (read_owned(meta_path, MAX_WHEEL_BYTES), read_owned(toolkit_path, MAX_WHEEL_BYTES))
    facts = check_meta_toolkit_wheels(*data, meta=meta, toolkit=toolkit)
    python, site, _ = _sdk_location(root, sdk_python, site_packages)
    dependencies = (_read_selected_dependency_wheels(dependency_requirements, dependency_wheels)
        if dependency_requirements is not None else None)
    current()
    with _selected_staging(".forge-selected-sdk-pair-", root, execute_command) as temp:
        directory = owned_directory(Path(temp), private=True)
        lines = []
        for content, fact in zip(data, facts):
            wheel = directory / f"{fact.distribution.replace('-', '_')}-{fact.version}-py3-none-any.whl"
            atomic_write(wheel, content, 0o600)
            lines.append(f"{fact.distribution} @ {wheel.as_uri()} --hash=sha256:{fact.sha256}")
        requirements = directory / "requirements.txt"
        atomic_write(requirements, ("\n".join(lines) + "\n").encode(), 0o600)
        current()
        arguments = [str(python), "-I", "-m", "pip", "--isolated", "install",
            "--disable-pip-version-check", "--no-index", "--no-deps", "--no-compile",
            "--force-reinstall", "--require-hashes", "-r", str(requirements)]
        if execute_command is None:
            subprocess.run(arguments, check=True, env=dict(os.environ, PIP_CONFIG_FILE=os.devnull))
        else:
            _selected_command_outcome(execute_command, arguments, pip=True)
        current()
        compare_installed_meta_toolkit_wheels(*facts, deployment_root=root,
            site_packages=site, sdk_python=python, **command_options)
        current()
        if dependencies is not None:
            _install_selected_dependency_bytes(dependencies, root=root, python=python, current=current,
                **command_options)
            current()
            compare_installed_meta_toolkit_wheels(*facts, deployment_root=root,
                site_packages=site, sdk_python=python, **command_options)
            current()
    current()
    return facts


def _native_prefix_progress(report_progress, action):
    """Report completed regular-file work, never a heartbeat or readiness claim.

    Whole-file reads/writes retain their existing bounds. A single long file
    operation can remain silent; counts advance only after that work succeeds.
    """
    files = total_bytes = 0
    last_counts = (0, 0)
    last_report = time.monotonic()
    if report_progress is not None:
        report_progress(f"{action}: 0 files, 0 bytes completed.")

    def completed(size=None, *, finish=False):
        nonlocal files, total_bytes, last_counts, last_report
        if size is not None:
            files += 1
            total_bytes += size
        if report_progress is None:
            return
        now = time.monotonic()
        if (files, total_bytes) != last_counts and (finish or now - last_report >= 2):
            report_progress(f"{action}: {files} files, {total_bytes} bytes completed.")
            last_counts, last_report = (files, total_bytes), now

    return completed


def verify_meta_toolkit_prefix(artifacts, *, deployment_root, prefix_path=None,
        report_progress=None, known=None, record=None):
    """Compare generic installed bytes/modes/links to the selected full archive.

    Native resource loading and application admission stay with their existing
    owners. This creates no installed-resource manifest or release receipt.
    ``known`` maps a regular file to the fingerprint a verified, hashed walk
    observed; an unchanged fingerprint skips that file's read. ``record``
    collects each file's fingerprint, taken after an fsync, for such a walk.
    """
    if __package__ == "launchkit.templates":
        from ..runtime_candidates import MetaToolkitRuntimeArtifacts, MAX_PRODUCT_FILE_BYTES
    else:
        from runtime_candidates import MetaToolkitRuntimeArtifacts, MAX_PRODUCT_FILE_BYTES
    require(type(artifacts) is MetaToolkitRuntimeArtifacts, "A complete selected native prefix is required.")
    if prefix_path is None:
        root = owned_directory(safe_path(deployment_root)) / ".bin"
        owned_directory(root, private=True)
        selected = root / artifacts.prefix
    else:
        selected = safe_path(prefix_path)
        require(selected.name == artifacts.prefix, "The original startup selected another product prefix.")
        owned_directory(selected.parent)
    expected = {entry.name: entry for entry in artifacts.entries}
    gateway = artifacts.prefix + "/bin/toolkit_gateway"
    progress = _native_prefix_progress(report_progress, "Checking native runtime files")
    observed = set()
    def walk(path, name):
        info = path.lstat()
        require(name in expected and info.st_uid == os.getuid(), "The installed native prefix has an unexpected or unowned path.")
        entry = expected[name]
        observed.add(name)
        if entry.kind == "directory":
            require(stat.S_ISDIR(info.st_mode) and stat.S_IMODE(info.st_mode) == entry.mode,
                "An installed native prefix directory mode changed.")
            for child in sorted(path.iterdir()):
                walk(child, name + "/" + child.name)
        elif entry.kind == "link":
            require(stat.S_ISLNK(info.st_mode) and info.st_nlink == 1
                and stat.S_IMODE(info.st_mode) == entry.mode and os.readlink(path) == entry.link,
                "An installed native prefix link changed.")
        else:
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                and stat.S_IMODE(info.st_mode) == entry.mode and info.st_size == entry.size,
                "An installed native prefix file or mode changed.")
            if record is not None:
                # Writeback write-protects every shared mapping of the file, so
                # any later write through one faults and moves its ctime.
                descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                try:
                    os.fsync(descriptor)
                finally:
                    os.close(descriptor)
                info = path.lstat()
                require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                    and stat.S_IMODE(info.st_mode) == entry.mode and info.st_size == entry.size,
                    "An installed native prefix file or mode changed.")
            # The executed gateway is always hashed, whatever its fingerprint.
            if (known is None or name == gateway or tuple(known.get(name, ())) != fingerprint(info)):
                data = read_owned(path, MAX_PRODUCT_FILE_BYTES, empty=True)
                require(digest(data) == entry.sha256, "An installed native prefix file changed.")
        require(fingerprint(path.lstat()) == fingerprint(info), "The native prefix changed during readback.")
        if entry.kind == "file":
            if record is not None:
                record[name] = (fingerprint(info), time.time_ns())
            progress(entry.size)
    walk(selected, artifacts.prefix)
    require(observed == set(expected), "The complete installed native prefix inventory changed.")
    progress(finish=True)
    return selected / "bin/toolkit_gateway"


def install_meta_toolkit_prefix(artifacts, *, deployment_root, require_current,
        report_progress=None):
    """Copy the whole selected prefix under the original stopped-project fence.

    Failed copies are retained for current-transaction recovery. No package
    program is imported or executed and no fallback native runtime is selected.
    """
    if __package__ == "launchkit.templates":
        from ..runtime_candidates import product_prefix_entries, recheck_meta_toolkit_artifacts, MAX_PRODUCT_ARCHIVE_BYTES
    else:
        from runtime_candidates import product_prefix_entries, recheck_meta_toolkit_artifacts, MAX_PRODUCT_ARCHIVE_BYTES
    def current():
        result = require_current()
        require(not inspect.isawaitable(result) and result is not False,
            "Native product installation needs its original synchronous owner fence.")
        recheck_meta_toolkit_artifacts(artifacts, artifacts.selection)
    current()
    if report_progress is not None:
        report_progress("Checking the selected native runtime archive.")
    content = read_owned(artifacts.archive.path, MAX_PRODUCT_ARCHIVE_BYTES)
    require(len(content) == artifacts.archive.size and digest(content) == artifacts.archive.sha256
        and product_prefix_entries(content, artifacts.prefix) == artifacts.entries,
        "The selected complete native prefix changed before installation.")
    root = owned_directory(safe_path(deployment_root))
    binaries = root / ".bin"
    if not binaries.exists():
        binaries.mkdir(mode=0o700)
    owned_directory(binaries, private=True)
    selected = binaries / artifacts.prefix
    if selected.exists() or selected.is_symlink():
        current()
        return verify_meta_toolkit_prefix(artifacts, deployment_root=root, report_progress=report_progress)
    progress = _native_prefix_progress(report_progress, "Copying native runtime files")
    inventory = {entry.name: entry for entry in artifacts.entries}
    directories = sorted((entry for entry in artifacts.entries if entry.kind == "directory"),
        key=lambda item: (item.name.count("/"), item.name))
    for entry in directories:
        current()
        (binaries / entry.name).mkdir(mode=0o700)
    observed = set()
    with tarfile.open(fileobj=io.BytesIO(content), mode="r|gz") as archive:
        for member in archive:
            current()
            name = member.name.rstrip("/")
            require(name in inventory and name not in observed,
                "The selected native prefix inventory changed during copy.")
            entry = inventory[name]
            require(member.mode == entry.mode and member.sparse is None,
                "A selected native prefix member changed during copy.")
            observed.add(name)
            path = binaries / name
            if entry.kind == "directory":
                require(member.isdir(), "A selected native prefix directory changed.")
            elif entry.kind == "link":
                require(member.issym() and member.linkname == entry.link,
                    "A selected native prefix link changed.")
                os.symlink(entry.link, path)
                if stat.S_IMODE(path.lstat().st_mode) != entry.mode:
                    os.chmod(path, entry.mode, follow_symlinks=False)
                require(stat.S_IMODE(path.lstat().st_mode) == entry.mode,
                    "A selected native prefix link mode changed.")
            else:
                require(member.isfile() and member.size == entry.size,
                    "A selected native prefix file changed.")
                source = archive.extractfile(member)
                require(source is not None, "A selected native prefix payload is missing.")
                with source:
                    data = source.read(entry.size + 1)
                require(len(data) == entry.size and digest(data) == entry.sha256,
                    "A selected native prefix payload changed.")
                atomic_write(path, data, entry.mode)
                progress(entry.size)
    require(observed == set(inventory), "The selected native prefix inventory is incomplete.")
    for entry in reversed(directories):
        current()
        os.chmod(binaries / entry.name, entry.mode)
    current()
    progress(finish=True)
    gateway = verify_meta_toolkit_prefix(artifacts, deployment_root=root, report_progress=report_progress)
    current()
    return gateway


def install_selected_dependency_wheels(requirements, wheels, *, deployment_root,
        site_packages, sdk_python, require_current, execute_command=None):
    """Install an owner-selected local dependency batch, without resolution.

    The caller selects the complete graph for its actual interpreter/platform
    and final SDKs. Hashes are integrity inputs, not provenance or admission.
    A successful pip exit is an installation action result, not installed-byte
    or native startup proof. Failures preserve possibly partial installed state.
    """
    require(callable(require_current), "Dependency installation needs its original synchronous owner fence.")
    require(execute_command is None or callable(execute_command),
        "Dependency installation needs its original selected interpreter executor.")

    def current():
        result = require_current()
        if inspect.isawaitable(result):
            if inspect.iscoroutine(result):
                result.close()
            raise CandidateArtifactError("Dependency installation needs a synchronous owner fence.")
        require(result is not False, "The selected dependency installation owner is no longer current.")

    current()
    require(site_packages is not None and sdk_python is not None,
        "Dependency installation requires explicit site-packages and interpreter paths.")
    root = owned_directory(safe_path(deployment_root))
    python, _, _ = _sdk_location(root, sdk_python, site_packages)
    pending = _read_selected_dependency_wheels(requirements, wheels)
    command_options = {} if execute_command is None else {"execute_command": execute_command}
    _install_selected_dependency_bytes(pending, root=root, python=python, current=current,
        **command_options)
    return wheels


def _read_selected_dependency_wheels(requirements, wheels):
    """Read and check the original selected batch without changing the venv."""
    if __package__ == "launchkit.templates":
        from ..sdk_wheel_integrity import read_wheel_members, verify_wheel_record
    else:
        from sdk_wheel_integrity import read_wheel_members, verify_wheel_record

    def read_selected(artifact, limit):
        require(type(artifact) is ArtifactFile and type(artifact.size) is int
            and 0 < artifact.size <= limit and isinstance(artifact.sha256, str)
            and re.fullmatch(r"[a-f0-9]{64}", artifact.sha256),
            "Dependency inputs require exact bounded artifact references.")
        content = read_owned(artifact.path, limit)
        require(len(content) == artifact.size and digest(content) == artifact.sha256,
            "The selected dependency artifact changed.")
        return content

    def normalized(name):
        return re.sub(r"[-_.]+", "-", name).lower()

    require(type(wheels) is tuple and 0 < len(wheels) <= 256,
        "Dependency installation requires a bounded explicit wheel batch.")
    pins = {}
    for line in read_selected(requirements, MAX_METADATA_BYTES).decode("utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9_.+!]*) --hash=sha256:([a-f0-9]{64})", line)
        require(match is not None, "Dependency requirements must contain only exact single-hash package pins.")
        name = normalized(match[1])
        require(name not in pins and name not in {"meerkat-meta", "meerkat-toolkit", "meerkat-mobkit"},
            "Dependency pins must be unique and cannot replace the selected SDKs.")
        pins[name] = match[2], match[3]
    require(len(pins) == len(wheels), "The selected dependency graph and wheel batch differ.")
    pending, names, total = [], set(), 0
    for artifact in wheels:
        content = read_selected(artifact, MAX_WHEEL_BYTES)
        total += len(content)
        require(total <= MAX_ARCHIVE_BYTES, "The selected dependency wheel batch exceeds its byte bound.")
        filename = Path(artifact.path).name
        match = re.fullmatch(r"([A-Za-z0-9_\.]+)-([A-Za-z0-9_.+!]+)(?:-([0-9][A-Za-z0-9_]*))?-([A-Za-z0-9_.]+)-([A-Za-z0-9_.]+)-([A-Za-z0-9_.]+)\.whl", filename)
        require(match is not None, "Dependency artifacts must retain their original wheel filenames.")
        name, version = normalized(match[1]), match[2]
        require(name not in names and pins.get(name) == (version, artifact.sha256),
            "Dependency wheel identity does not match its exact selected pin.")
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            inventory = [entry.filename for entry in archive.infolist()]
        values = read_wheel_members(content, expected_names=inventory,
            max_file_bytes=MAX_WHEEL_BYTES, max_total_bytes=64 * 1024 * 1024, max_files=4096,
            allow_directories=True)
        prefix = f"{match[1]}-{version}.dist-info/"
        require({prefix + "METADATA", prefix + "WHEEL", prefix + "RECORD"} <= values.keys()
            and not any(".dist-info/" in path and not path.startswith(prefix) for path in values),
            "Dependency wheel metadata must have one exact package identity.")
        metadata = BytesParser().parsebytes(values[prefix + "METADATA"])
        wheel = BytesParser().parsebytes(values[prefix + "WHEEL"])
        package_names = metadata.get_all("Name", [])
        tags = {f"{py}-{abi}-{platform_tag}" for py in match[4].split(".")
            for abi in match[5].split(".") for platform_tag in match[6].split(".")}
        require(len(package_names) == 1 and normalized(package_names[0]) == name
            and metadata.get_all("Version") == [version]
            and wheel.get_all("Wheel-Version") == ["1.0"]
            and wheel.get_all("Root-Is-Purelib") in (["true"], ["false"])
            and len(wheel.get_all("Tag", [])) == len(tags)
            and set(wheel.get_all("Tag", [])) == tags,
            "Dependency wheel metadata or platform tags differ from its filename and selected pin.")
        verify_wheel_record(values, prefix + "RECORD")
        names.add(name)
        pending.append((name, filename, content, artifact.sha256))
    return tuple(pending)


def verify_selected_application_dependencies(content, artifacts):
    """Check accepted application pins against the original exact offline batch."""
    from ..runtime_candidates import MetaToolkitRuntimeArtifacts, recheck_meta_toolkit_artifacts
    require(type(artifacts) is MetaToolkitRuntimeArtifacts and type(content) is bytes
        and len(content) <= MAX_METADATA_BYTES,
        "Application dependency compatibility needs its bounded accepted source and selected product.")
    recheck_meta_toolkit_artifacts(artifacts, artifacts.selection)
    target = artifacts.selection
    pending = _read_selected_dependency_wheels(target.dependency_requirements, target.dependency_wheels)
    selected = {name: (filename.split("-")[1], sha256)
        for name, filename, _, sha256 in pending}
    names = set()
    for line in content.decode("utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9_.+!]*)(?: --hash=sha256:([a-f0-9]{64}))?", line)
        require(match is not None,
            "The selected product can preserve only exact application package pins in its offline batch.")
        name = re.sub(r"[-_.]+", "-", match[1]).lower()
        require(name not in names and name in selected and selected[name][0] == match[2]
            and (match[3] is None or selected[name][1] == match[3]),
            "An accepted application dependency is absent or different in the selected offline batch. No runtime files were changed.")
        names.add(name)
    recheck_meta_toolkit_artifacts(artifacts, target)


def _install_selected_dependency_bytes(pending, *, root, python, current, execute_command=None):
    """Stage only the preflighted bytes under the original synchronous fence."""
    current()
    with _selected_staging(".forge-selected-dependencies-", root, execute_command) as temp:
        directory = owned_directory(Path(temp), private=True)
        lines = []
        for name, filename, content, hashed in pending:
            path = directory / filename
            atomic_write(path, content, 0o600)
            lines.append(f"{name} @ {path.as_uri()} --hash=sha256:{hashed}")
        selected = directory / "requirements.txt"
        atomic_write(selected, ("\n".join(lines) + "\n").encode(), 0o600)
        current()
        arguments = [str(python), "-I", "-m", "pip", "--isolated", "install",
            "--disable-pip-version-check", "--no-index", "--no-deps", "--no-compile",
            "--force-reinstall", "--require-hashes", "--only-binary=:all:", "-r", str(selected)]
        if execute_command is None:
            subprocess.run(arguments, check=True, env=dict(os.environ, PIP_CONFIG_FILE=os.devnull))
        else:
            _selected_command_outcome(execute_command, arguments, pip=True)
        current()
    current()


def atomic_write(path, payload, mode):
    owned_directory(path.parent)
    require(not path.is_symlink(), "Candidate destination must not be a link.")
    if path.exists():
        read_owned(path, max(MAX_BINARY_BYTES, MAX_METADATA_BYTES), private=True, empty=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(prefix=f".{path.name}-", dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            os.fchmod(handle.fileno(), mode)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def write_json(path, value):
    atomic_write(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode(), 0o600)


def verify_installed_candidate_sdk(selection_path=None, *, expected_native_version=None,
        expected_platform=None, sdk_python=None, site_packages=None, verified_artifacts=None):
    context = load_candidate_selection(selection_path, expected_native_version=expected_native_version,
        expected_platform=expected_platform, verified_artifacts=verified_artifacts)
    python, site, receipt_path = sdk_location(context, sdk_python, site_packages)
    proof = sdk_proof(context, site, python)
    require(unique_json(read_owned(receipt_path, MAX_METADATA_BYTES, private=True)) == proof,
        "The installed candidate SDK receipt differs from its actual payload and binding.")
    if verified_artifacts is not None:
        _recheck_selection_artifacts(context)
    return proof


def install_candidate_sdk(selection_path=None, *, expected_native_version=None,
        expected_platform=None, sdk_python=None, site_packages=None):
    context = load_candidate_selection(selection_path, expected_native_version=expected_native_version,
        expected_platform=expected_platform)
    python, site, receipt_path = sdk_location(context, sdk_python, site_packages)
    data = checked_wheel(context)
    with tempfile.TemporaryDirectory(prefix=".forge-candidate-sdk-", dir=context.root / ".venv") as temp:
        wheel = Path(temp) / context.artifacts.sdk.wheel.path.name
        atomic_write(wheel, data, 0o600)
        subprocess.run([str(python), "-m", "pip", "install", "--disable-pip-version-check",
            "--no-index", "--no-deps", "--no-compile", "--force-reinstall", str(wheel)], check=True)
    proof = sdk_proof(context, site, python)
    write_json(receipt_path, proof)
    return proof


def install_candidate_requirements(selection_path=None, *, expected_native_version=None,
        expected_platform=None, sdk_python=None, site_packages=None):
    context = load_candidate_selection(selection_path, expected_native_version=expected_native_version,
        expected_platform=expected_platform)
    python, _, _ = sdk_location(context, sdk_python, site_packages)
    verify_installed_candidate_sdk(context.path, expected_native_version=context.accepted.native_version,
        expected_platform=context.accepted.platform, sdk_python=python, site_packages=site_packages)
    text = read_owned(context.root / "requirements.txt", MAX_METADATA_BYTES).decode("utf-8")
    pins, sdk_pins = [], []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*(?:\[[A-Za-z0-9_,.-]+\])?==[A-Za-z0-9][A-Za-z0-9_.+!-]*", line),
            "Candidate requirements must retain ordinary exact package pins.")
        name = re.split(r"\[|==", line, 1)[0].lower().replace("_", "-").replace(".", "-")
        if name == "meerkat-mobkit":
            sdk_pins.append(line)
        else:
            pins.append(line)
    require(sdk_pins == ["meerkat-mobkit==" + context.accepted.native_version],
        "Requirements must contain exactly one selected candidate SDK pin.")
    with tempfile.TemporaryDirectory(prefix=".forge-candidate-requirements-", dir=context.root / ".venv") as temp:
        requirements = Path(temp) / "requirements.txt"
        atomic_write(requirements, ("\n".join(pins) + "\n").encode(), 0o600)
        if pins:
            subprocess.run([str(python), "-m", "pip", "install", "--disable-pip-version-check",
                "-r", str(requirements)], check=True)
    return verify_installed_candidate_sdk(context.path, expected_native_version=context.accepted.native_version,
        expected_platform=context.accepted.platform, sdk_python=python, site_packages=site_packages)


def native_payload(native):
    content = read_owned(native.archive.path, MAX_ARCHIVE_BYTES)
    require(len(content) == native.archive.size and digest(content) == native.archive.sha256,
        "The native candidate archive changed after verification.")
    try:
        with gzip.GzipFile(fileobj=io.BytesIO(content)) as compressed:
            expanded = compressed.read(MAX_BINARY_BYTES + MAX_TAR_OVERHEAD + 1)
        require(len(expanded) <= MAX_BINARY_BYTES + MAX_TAR_OVERHEAD,
            "The native candidate archive exceeded its decompressed bound.")
        with tarfile.open(fileobj=io.BytesIO(expanded), mode="r:") as archive:
            item = archive.next()
            require(item is not None and item.name == native.binary == native.inner_name
                and item.isfile() and item.sparse is None and item.mode & 0o111
                and 0 < item.size == native.inner_size <= MAX_BINARY_BYTES,
                "The native candidate archive must contain one flat ordinary executable.")
            handle = archive.extractfile(item)
            require(handle is not None, "The native candidate payload is missing.")
            with handle:
                payload = handle.read(MAX_BINARY_BYTES + 1)
            require(len(payload) == native.inner_size and digest(payload) == native.inner_sha256
                and archive.next() is None, "The native candidate payload or inventory changed.")
    except (tarfile.TarError, EOFError, OSError) as error:
        raise CandidateArtifactError("The native candidate archive is invalid.") from error
    return payload


def native_receipt(context, native):
    return {"source": "signed-candidate", "version": context.accepted.native_version,
        "platform": context.accepted.platform, "archive": native.archive.path.name,
        "archive_sha256": native.archive.sha256, "binary_sha256": native.inner_sha256,
        **binding_fields(context)}


def verify_installed_candidate_runtime(selection_path=None, *, expected_native_version=None,
        expected_platform=None, sdk_python=None, site_packages=None, tools_dir=None, verified_artifacts=None):
    context = load_candidate_selection(selection_path, expected_native_version=expected_native_version,
        expected_platform=expected_platform, verified_artifacts=verified_artifacts)
    sdk = verify_installed_candidate_sdk(context.path, expected_native_version=context.accepted.native_version,
        expected_platform=context.accepted.platform, sdk_python=sdk_python, site_packages=site_packages,
        verified_artifacts=verified_artifacts)
    directory = Path(tools_dir) if tools_dir is not None else context.root / ".bin"
    require(directory == context.root / ".bin", "Candidate native tools must use the deployment's canonical .bin directory.")
    owned_directory(directory, private=True)
    receipts = {}
    for native in context.artifacts.native:
        if native.binary not in BINARIES:
            continue
        payload = read_owned(directory / native.binary, MAX_BINARY_BYTES, private=True)
        require(len(payload) == native.inner_size and digest(payload) == native.inner_sha256
            and os.access(directory / native.binary, os.X_OK), "Installed candidate native executable changed.")
        receipt = unique_json(read_owned(directory / (native.binary + ".receipt.json"), MAX_METADATA_BYTES, private=True))
        require(receipt == native_receipt(context, native), "Installed candidate native receipt binding changed.")
        receipts[native.binary] = receipt
    require(set(receipts) == set(BINARIES), "The two installed candidate native tools are required.")
    if verified_artifacts is not None:
        _recheck_selection_artifacts(context)
    return {"source": "signed-candidate", "version": context.accepted.native_version,
        "platform": context.accepted.platform, **binding_fields(context), "sdk": sdk, "native": receipts}


def install_candidate_native_tools(selection_path=None, *, expected_native_version=None,
        expected_platform=None, sdk_python=None, site_packages=None, tools_dir=None):
    context = load_candidate_selection(selection_path, expected_native_version=expected_native_version,
        expected_platform=expected_platform)
    verify_installed_candidate_sdk(context.path, expected_native_version=context.accepted.native_version,
        expected_platform=context.accepted.platform, sdk_python=sdk_python, site_packages=site_packages)
    directory = Path(tools_dir) if tools_dir is not None else context.root / ".bin"
    require(directory == context.root / ".bin", "Candidate native tools must use the deployment's canonical .bin directory.")
    require(not directory.is_symlink(), "Candidate native directory must not be a link.")
    if directory.exists():
        owned_directory(directory, private=True)
    selected = {native.binary: native for native in context.artifacts.native if native.binary in BINARIES}
    require(set(selected) == set(BINARIES), "Both candidate native tools must be present.")
    pending = {name: native_payload(native) for name, native in selected.items()}
    # Every archive, SDK source and installed destination is checked before writes.
    for name in selected:
        for path in (directory / name, directory / (name + ".receipt.json")):
            require(not path.is_symlink(), "Candidate native destination must not be a link.")
            if path.exists():
                read_owned(path, MAX_BINARY_BYTES, private=True, empty=True)
    directory.mkdir(mode=0o700, exist_ok=True)
    for name, native in selected.items():
        atomic_write(directory / name, pending[name], 0o700)
        write_json(directory / (name + ".receipt.json"), native_receipt(context, native))
    return verify_installed_candidate_runtime(context.path, expected_native_version=context.accepted.native_version,
        expected_platform=context.accepted.platform, sdk_python=sdk_python, site_packages=site_packages)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("sdk-install", "requirements-install", "sdk-verify", "native-install", "runtime-verify"))
    parser.add_argument("--selection", default=str(ROOT / "runtime-candidate.json"))
    parser.add_argument("--expected-native-version")
    parser.add_argument("--expected-platform")
    args = parser.parse_args(argv)
    owner = {"sdk-install": install_candidate_sdk, "requirements-install": install_candidate_requirements,
        "sdk-verify": verify_installed_candidate_sdk, "native-install": install_candidate_native_tools,
        "runtime-verify": verify_installed_candidate_runtime}[args.action]
    try:
        result = owner(args.selection, expected_native_version=args.expected_native_version,
            expected_platform=args.expected_platform)
    except (CandidateArtifactError, OSError, subprocess.SubprocessError, UnicodeError) as error:
        raise SystemExit(f"Candidate installation refused: {error}") from error
    print(json.dumps(result, sort_keys=True))
    return result


if __name__ == "__main__":
    main()
