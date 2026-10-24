"""Pure integrity checks for already selected SDK wheel bytes.

Callers own source selection, provenance and native compatibility. These
functions read no paths, import no SDK, install nothing and confer no admission.
"""
from __future__ import annotations

import base64
import csv
from dataclasses import dataclass
from email.parser import BytesParser
import hashlib
import io
from pathlib import PurePosixPath
import re
import stat
import zipfile


MAX_WHEEL_BYTES = 32 * 1024 * 1024
MAX_WHEEL_TOTAL = 64 * 1024 * 1024
MAX_WHEEL_FILES = 4096
HEX = re.compile(r"[a-f0-9]{64}")
# Supported byte formats, not source selection or native compatibility evidence.
SUPPORTED_META_VERSIONS = frozenset({"0.9.0", "0.9.1", "0.9.4", "0.9.5"})
SUPPORTED_TOOLKIT_VERSIONS = frozenset({"0.9.0", "0.9.4", "0.9.5"})


class WheelIntegrityError(ValueError):
    """Selected bytes or their exact declared inventory do not match."""


def _require(condition, message):
    if not condition:
        raise WheelIntegrityError(message)


def read_wheel_members(content, *, expected_names, max_file_bytes, max_total_bytes, max_files,
        allow_directories=False):
    """Read only a bounded, exact ordinary ZIP inventory, without extraction."""
    with zipfile.ZipFile(io.BytesIO(content)) as archive:
        entries = archive.infolist()
        names = [entry.filename for entry in entries]
        _require(len(names) == len(set(names)) == len({name.casefold() for name in names})
            and set(names) == set(expected_names) and len(names) <= max_files,
            "The SDK wheel has a duplicate or unexpected inventory.")
        if allow_directories:
            canonical_names = [name[:-1] if entry.is_dir() else name
                               for name, entry in zip(names, entries)]
            _require(len(canonical_names) == len({name.casefold() for name in canonical_names}),
                "The dependency wheel has a conflicting file or directory path.")
        _require(sum(entry.file_size for entry in entries) <= max_total_bytes,
            "The SDK wheel exceeds its total byte bound.")
        values = {}
        for entry in entries:
            name, mode = entry.filename, entry.external_attr >> 16
            directory = entry.is_dir()
            canonical = name[:-1] if directory else name
            path = PurePosixPath(canonical)
            _require(canonical and not path.is_absolute() and path.as_posix() == canonical
                and not any(part in ("", ".", "..") for part in path.parts)
                and not any(ord(char) < 32 or char in "\\:" for char in name)
                and (allow_directories and directory and entry.file_size == 0
                     and stat.S_IFMT(mode) in (0, stat.S_IFDIR)
                     or not directory and stat.S_IFMT(mode) in (0, stat.S_IFREG))
                and not entry.flag_bits & 1 and 0 <= entry.file_size <= max_file_bytes,
                "The SDK wheel contains an invalid path, link, or oversized member.")
            with archive.open(entry) as source:
                value = source.read(max_file_bytes + 1)
            _require(len(value) == entry.file_size, "An SDK wheel member exceeds its byte bound.")
            if not directory:
                values[name] = value
    return values


def verify_wheel_record(values, record_name):
    """Verify every RECORD member, including its single unhashed self-entry."""
    rows = list(csv.reader(io.StringIO(values[record_name].decode("utf-8"))))
    _require(all(len(row) == 3 for row in rows) and len(rows) == len(values)
        and len({row[0] for row in rows}) == len(rows) and {row[0] for row in rows} == set(values),
        "The SDK wheel RECORD has a duplicate or incomplete inventory.")
    for name, recorded_hash, recorded_size in rows:
        if name == record_name:
            _require(recorded_hash == recorded_size == "", "The SDK wheel RECORD self-entry is invalid.")
        else:
            hashed = base64.urlsafe_b64encode(hashlib.sha256(values[name]).digest()).decode().rstrip("=")
            _require(recorded_hash == "sha256=" + hashed and recorded_size == str(len(values[name])),
                "An SDK wheel RECORD digest or size changed.")


@dataclass(frozen=True)
class WheelByteExpectation:
    """Caller-held byte expectations only; no path or release authority."""

    distribution: str
    version: str
    sha256: str
    size: int
    # Full package paths; declared dist-info names are relative to that directory.
    source_payload: tuple[tuple[str, str, int], ...]
    metadata: tuple[tuple[str, str, int], ...]


@dataclass(frozen=True)
class WheelIntegrityFacts:
    """Observed byte identity, not installation or native compatibility."""

    distribution: str
    version: str
    sha256: str
    size: int
    source_payload: tuple[tuple[str, str, int], ...]
    metadata: tuple[tuple[str, str, int], ...]


def _normalized(value):
    return re.sub(r"[-_.]+", "-", value).lower()


def _inventory(records, *, prefix=None):
    _require(type(records) is tuple and 0 < len(records) <= MAX_WHEEL_FILES,
        "The selected SDK inventory must be explicit and bounded.")
    result = {}
    for item in records:
        _require(type(item) is tuple and len(item) == 3,
            "The selected SDK inventory entry is invalid.")
        name, hashed, size = item
        _require(isinstance(name, str) and name and name not in result
            and (prefix is None or name.startswith(prefix))
            and isinstance(hashed, str) and HEX.fullmatch(hashed) is not None
            and type(size) is int and 0 <= size <= MAX_WHEEL_BYTES,
            "The selected SDK inventory entry is invalid.")
        result[name] = hashed, size
    return result


def _check_selected_wheel(content, expected, *, distribution, version, required):
    _require(type(expected) is WheelByteExpectation and type(content) is bytes
        and expected.distribution == distribution and expected.version == version
        and isinstance(expected.sha256, str) and HEX.fullmatch(expected.sha256) is not None
        and type(expected.size) is int and 0 < expected.size <= MAX_WHEEL_BYTES
        and len(content) == expected.size and hashlib.sha256(content).hexdigest() == expected.sha256,
        "The SDK wheel bytes differ from the caller-held package identity.")
    package = distribution.replace("-", "_")
    prefix = f"{package}-{version}.dist-info/"
    payload = _inventory(expected.source_payload, prefix=package + "/")
    declared = _inventory(expected.metadata)
    _require(required <= payload.keys()
        and {"METADATA", "WHEEL"} <= declared.keys()
        and all(name in {"METADATA", "WHEEL", "top_level.txt"}
            or name.startswith("licenses/") and len(name) > len("licenses/") for name in declared),
        "The selected SDK modules or declared producer metadata are incomplete.")
    _require(not any(name.endswith(".pth") for name in payload),
        "Selected SDK wheels cannot include startup path hooks.")
    metadata_names = {prefix + name for name in declared}
    values = read_wheel_members(content, expected_names=set(payload) | metadata_names | {prefix + "RECORD"},
        max_file_bytes=MAX_WHEEL_BYTES, max_total_bytes=MAX_WHEEL_TOTAL, max_files=MAX_WHEEL_FILES)
    metadata = BytesParser().parsebytes(values[prefix + "METADATA"])
    wheel = BytesParser().parsebytes(values[prefix + "WHEEL"])
    _require(metadata.get_all("Name") == [distribution] and metadata.get_all("Version") == [version]
        and wheel.get_all("Wheel-Version") == ["1.0"]
        and wheel.get_all("Root-Is-Purelib") == ["true"]
        and wheel.get_all("Tag") == ["py3-none-any"]
        and ("top_level.txt" not in declared or values[prefix + "top_level.txt"].strip() == package.encode()),
        "The SDK wheel has an unexpected package or platform identity.")
    licenses = metadata.get_all("License-File", [])
    _require(len(licenses) == len(set(licenses))
        and {"licenses/" + name for name in licenses} == {name for name in declared if name.startswith("licenses/")},
        "The SDK wheel licenses differ from its declared producer metadata.")
    verify_wheel_record(values, prefix + "RECORD")
    for name, (hashed, size) in {**payload, **{prefix + name: pin for name, pin in declared.items()}}.items():
        _require(hashlib.sha256(values[name]).hexdigest() == hashed and len(values[name]) == size,
            "The SDK wheel payload or metadata differs from the caller-held exact inventory.")
    facts = WheelIntegrityFacts(distribution, version, expected.sha256, expected.size,
        tuple(sorted(expected.source_payload)), tuple(sorted(expected.metadata)))
    return facts, metadata


def _native_host_marker(marker):
    """Accept only a single PEP 508 extra equality, with optional grouping."""
    marker = marker.strip()
    while marker.startswith("(") and marker.endswith(")"):
        marker = marker[1:-1].strip()
    quoted = r"(?P<quote>['\"])(?P<extra>[A-Za-z0-9_.-]+)(?P=quote)"
    for pattern in (r"extra\s*==\s*" + quoted, quoted + r"\s*==\s*extra"):
        match = re.fullmatch(pattern, marker)
        if match is not None:
            return _normalized(match.group("extra")) == "native-host"
    return False


def check_meta_toolkit_wheels(meta_content, toolkit_content, *, meta, toolkit):
    """Check the source-declared SDK pair; do not select or admit a runtime.

    Matching native binaries, dependency closure and catalogs remain producer
    inputs. Other dependency declarations are retained as exact metadata bytes;
    this is not a general PEP 508 validator. Only the native-host Meta relation
    and absence of a legacy SDK relation are checked here. The selected package
    modules are verified as bytes, never imported.
    """
    try:
        _require(type(meta) is WheelByteExpectation and type(toolkit) is WheelByteExpectation,
            "The SDK pair requires explicit caller-held byte expectations.")
        _require(type(meta.version) is str and meta.version in SUPPORTED_META_VERSIONS,
            "The selected Meta SDK version is not supported by this byte reader.")
        _require(type(toolkit.version) is str and toolkit.version in SUPPORTED_TOOLKIT_VERSIONS,
            "The selected Toolkit SDK version is not supported by this byte reader.")
        meta_facts, meta_metadata = _check_selected_wheel(meta_content, meta,
            distribution="meerkat-meta", version=meta.version,
            required={"meerkat_meta/builder.py", "meerkat_meta/runtime.py", "meerkat_meta/_transport.py"})
        toolkit_facts, toolkit_metadata = _check_selected_wheel(toolkit_content, toolkit,
            distribution="meerkat-toolkit", version=toolkit.version,
            required={"meerkat_toolkit/application_runtime.py", "meerkat_toolkit/host_control.py"})
        meta_dependencies = []
        for metadata in (meta_metadata, toolkit_metadata):
            for requirement in metadata.get_all("Requires-Dist", []):
                name = re.match(r"[A-Za-z0-9][A-Za-z0-9_.-]*", requirement)
                _require(name is not None, "The SDK wheel has an invalid dependency declaration.")
                normalized = _normalized(name.group())
                _require(normalized != "meerkat-mobkit", "The selected SDK pair cannot depend on the legacy SDK.")
                if metadata is toolkit_metadata and normalized == "meerkat-meta":
                    meta_dependencies.append(requirement[name.end():])
        extras = toolkit_metadata.get_all("Provides-Extra", [])
        _require("native-host" in {_normalized(value) for value in extras}
            and len(meta_dependencies) == 1,
            "Toolkit must declare exactly one selected native-host Meta dependency.")
        dependency, separator, marker = meta_dependencies[0].partition(";")
        version = re.escape(meta.version)
        exact_dependency = rf"\s*(?:==\s*{version}|\(\s*==\s*{version}\s*\))\s*"
        _require(separator and re.fullmatch(exact_dependency, dependency)
            and _native_host_marker(marker),
            "Toolkit native-host must require exactly the selected Meta SDK version.")
        return meta_facts, toolkit_facts
    except WheelIntegrityError:
        raise
    except (ValueError, TypeError, KeyError, AttributeError, UnicodeError, zipfile.BadZipFile,
            NotImplementedError, EOFError) as error:
        raise WheelIntegrityError("The selected SDK wheel bytes or metadata are invalid.") from error
