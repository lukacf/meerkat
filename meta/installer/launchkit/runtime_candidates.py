"""Read external candidate bytes against an independently accepted host binding.

This is an integrity reader, not candidate admission or signature verification.
The caller must already have accepted the producer's cryptographic provenance
and the exact native/local-SDK tuple. A digest, a constructed CandidateBinding,
or a JSON ``verified=true`` flag cannot establish that authority. No artifact is
extracted, imported, executed, installed, downloaded, or changed here.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
from email.parser import BytesParser
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import tarfile
import zipfile

if __package__:
    from .sdk_wheel_integrity import (WheelByteExpectation, WheelIntegrityError,
        check_meta_toolkit_wheels, read_wheel_members, verify_wheel_record)
else:
    from sdk_wheel_integrity import (WheelByteExpectation, WheelIntegrityError,
        check_meta_toolkit_wheels, read_wheel_members, verify_wheel_record)

# Existing installer native and distribution bounds, applied to external bytes.
MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
# A sanity bound: _native expands and hashes a payload in 1 MiB chunks after
# its archive matched the accepted record. The same 1 GiB as deployment_worker.
MAX_BINARY_BYTES = 1024 * 1024 * 1024
MAX_TAR_OVERHEAD = 1024 * 1024
MAX_METADATA_BYTES = 1024 * 1024
MAX_WHEEL_BYTES = 32 * 1024 * 1024
MAX_WHEEL_TOTAL = 64 * 1024 * 1024
MAX_WHEEL_FILES = 4096
HEX = re.compile(r"[a-f0-9]{64}")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
PLATFORMS = frozenset({"aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"})
BINARIES = {"mobkit_gateway": "mobkit-gateway", "mobkit_repair": "mobkit-repair",
    "rpc_gateway": "mobkit-rpc-gateway"}


class CandidateArtifactError(ValueError):
    """Candidate identity, ownership, bounds, or artifact integrity changed."""


@dataclass(frozen=True)
class CandidateBinding:
    """Exact caller-accepted identity. Constructing it does not confer admission."""

    tuple_sha256: str
    repository: str
    source_sha: str
    native_version: str
    core_version: str
    platform: str
    native_run_id: int
    native_run_attempt: int
    native_manifest_sha256: str
    sdk_wheel_sha256: str


@dataclass(frozen=True)
class ArtifactFile:
    path: Path
    size: int
    sha256: str


@dataclass(frozen=True)
class MetaToolkitRuntimeTarget:
    """Application-owner selection of existing producer outputs, not admission.

    The assembly and its producer evidence select the complete native product.
    SDK and offline dependency bytes remain separate explicit inputs. None of
    these fields belong to Toolkit's activation/policy bootstrap selection.
    An explicit source_manifest selects a relocated exact copy; omission keeps
    the original producer locator for historical selections.
    """

    artifact_root: Path
    target: str
    assembly: ArtifactFile
    meta_wheel: ArtifactFile
    meta: WheelByteExpectation
    toolkit_wheel: ArtifactFile
    toolkit: WheelByteExpectation
    dependency_requirements: ArtifactFile
    dependency_wheels: tuple[ArtifactFile, ...]
    model_catalog: ArtifactFile
    source_manifest: ArtifactFile | None = None


@dataclass(frozen=True)
class ProductPrefixEntry:
    name: str
    kind: str
    mode: int
    size: int = 0
    sha256: str | None = None
    link: str | None = None


@dataclass(frozen=True)
class MetaToolkitRuntimeArtifacts:
    selection: MetaToolkitRuntimeTarget
    archive: ArtifactFile
    prefix: str
    entries: tuple[ProductPrefixEntry, ...]
    candidate: dict
    catalog: dict
    evidence: tuple[ArtifactFile, ...]
    origin: str = "installed-gateway-assembly"
    # The oldest glibc the product's Linux programs run on, as declared.
    glibc_floor: tuple[int, int] | None = None
    _verified_inputs: tuple = field(default=(), init=False, repr=False, compare=False)
    _verified_metadata: tuple = field(default=(), init=False, repr=False, compare=False)
    _inventory_source: str = field(default="archive", init=False, repr=False, compare=False)
    _known_fingerprints: tuple = field(default=(), init=False, repr=False, compare=False)


@dataclass(frozen=True)
class RetainedPrefixInventory:
    """An inventory kept after a successful full verification of these pins.

    Only the Start reader accepts it, and only for the same selected assembly,
    archive pin and prefix. Its owner binds it to receipt.json by sha256.
    """
    entries: tuple[ProductPrefixEntry, ...]
    prefix: str
    assembly_sha256: str
    archive_sha256: str
    archive_bytes: int
    inventory_sha256: str
    receipt_sha256: str
    runtime_candidate_sha256: str
    fingerprints: tuple = ()

    def matches(self, assembly_sha256, archive, prefix):
        return (self.assembly_sha256 == assembly_sha256 and self.prefix == prefix
            and (self.archive_sha256, self.archive_bytes) == (archive.sha256, archive.size))


@dataclass(frozen=True)
class NativeArtifact:
    binary: str
    archive: ArtifactFile
    inner_name: str
    inner_size: int
    inner_sha256: str


@dataclass(frozen=True)
class SdkArtifact:
    wheel: ArtifactFile
    payload: tuple[tuple[str, str], ...]
    origin: str = "local-build-from-exact-frozen-source"
    signed: bool = False
    published: bool = False


@dataclass(frozen=True)
class CandidateArtifacts:
    identity: CandidateBinding
    tuple_file: ArtifactFile
    native: tuple[NativeArtifact, ...]
    sdk: SdkArtifact
    evidence: tuple[ArtifactFile, ...]
    native_origin: str = "signed-candidate"
    _artifact_root: Path | None = field(default=None, init=False, repr=False, compare=False)
    _verified_inputs: tuple = field(default=(), init=False, repr=False, compare=False)


def _require(condition, message):
    if not condition:
        raise CandidateArtifactError(message)


def _digest(value):
    return hashlib.sha256(value).hexdigest()


def _hex(value):
    return isinstance(value, str) and HEX.fullmatch(value) is not None


def _positive(value):
    return type(value) is int and value > 0


PRODUCT_TARGETS = {"macos-arm64": "aarch64-apple-darwin",
    "linux-x86_64": "x86_64-unknown-linux-gnu"}
MAX_PRODUCT_ARCHIVE_BYTES = 3 * 1024 * 1024 * 1024
MAX_PRODUCT_FILE_BYTES = 2 * 1024 * 1024 * 1024
MAX_PRODUCT_BYTES = 4 * 1024 * 1024 * 1024
MAX_PRODUCT_ENTRIES = 50_032
MAX_PRODUCT_METADATA = 32 * 1024 * 1024


def product_prefix_entries(content, prefix):
    """Inventory selected archive bytes, without duplicating resource semantics.

    The native InstalledResources owner still verifies its manifest and loader.
    This reader checks generic archive confinement and preserves every selected
    file, directory mode and confined link, rather than reducing it to a binary.
    """
    from pathlib import PurePosixPath
    _require(type(content) is bytes and 0 < len(content) <= MAX_PRODUCT_ARCHIVE_BYTES,
        "The native product archive exceeds its bound.")
    entries, total = {}, 0
    class Expanded:
        def __init__(self, source):
            self.source, self.total = source, 0
        def read(self, size=-1):
            remaining = MAX_PRODUCT_BYTES + MAX_PRODUCT_ENTRIES * 2048 + 1 - self.total
            value = self.source.read(remaining if size < 0 else min(size, remaining))
            self.total += len(value)
            _require(self.total <= MAX_PRODUCT_BYTES + MAX_PRODUCT_ENTRIES * 2048,
                "The native product expansion exceeds its bound.")
            return value
    with gzip.GzipFile(fileobj=io.BytesIO(content), mode="rb") as source:
        expanded = Expanded(source)
        with tarfile.open(fileobj=expanded, mode="r|") as archive:
            for member in archive:
                name = member.name.rstrip("/")
                path = PurePosixPath(name)
                _require(name and len(name) <= 4096 and not path.is_absolute()
                    and path.as_posix() == name and ".." not in path.parts
                    and path.parts[0] == prefix and name not in entries
                    and len(entries) < MAX_PRODUCT_ENTRIES and member.sparse is None
                    and not member.mode & ~0o777, "The native product archive has an unsafe entry.")
                if member.isdir():
                    entry = ProductPrefixEntry(name, "directory", member.mode)
                elif member.issym():
                    link = member.linkname
                    _require(link and len(link.encode("utf-8")) <= 4096 and not link.startswith("/")
                        and "\x00" not in link and "\\" not in link,
                        "The native product link escapes its prefix.")
                    entry = ProductPrefixEntry(name, "link", member.mode, link=link)
                elif member.isfile():
                    _require(0 <= member.size <= MAX_PRODUCT_FILE_BYTES,
                        "A native product file exceeds its bound.")
                    total += member.size
                    _require(total <= MAX_PRODUCT_BYTES, "The native product exceeds its total bound.")
                    stream = archive.extractfile(member)
                    _require(stream is not None, "A native product file is missing.")
                    hashed, count = hashlib.sha256(), 0
                    with stream:
                        while chunk := stream.read(1024 * 1024):
                            count += len(chunk)
                            hashed.update(chunk)
                    _require(count == member.size, "A native product file is truncated.")
                    entry = ProductPrefixEntry(name, "file", member.mode, count, hashed.hexdigest())
                else:
                    raise CandidateArtifactError("The native product contains a special file or hardlink.")
                entries[name] = entry
        while expanded.read(1024 * 1024):
            pass
    _require(prefix in entries and entries[prefix].kind == "directory",
        "The native product root is missing.")
    for name, entry in entries.items():
        parent = str(PurePosixPath(name).parent)
        if name != prefix:
            _require(parent in entries and entries[parent].kind == "directory",
                "The native product has an absent or linked parent.")
        if entry.kind == "link":
            from collections import deque
            pending = deque((*PurePosixPath(name).parent.parts, *entry.link.split("/")))
            resolved, followed = [], 0
            while pending:
                part = pending.popleft()
                if part in ("", "."):
                    continue
                if part == "..":
                    _require(len(resolved) > 1, "The native product link escapes its prefix.")
                    resolved.pop()
                    continue
                resolved.append(part)
                current = "/".join(resolved)
                _require(current in entries, "The native product has a dangling link.")
                selected = entries[current]
                if selected.kind == "link":
                    followed += 1
                    _require(followed <= 40, "The native product has a cyclic link.")
                    resolved.pop()
                    pending.extendleft(reversed(selected.link.split("/")))
                    _require(len(pending) <= 4096, "The native product link expansion exceeds its bound.")
                elif pending:
                    _require(selected.kind == "directory", "The native product link traverses a file.")
    return tuple(entries[name] for name in sorted(entries))


def _declared_glibc_floor(record, platform):
    """The oldest glibc a Linux product runs on, as its resource build declares.

    The Connect resource packager records the host the product supports. A
    record without that declaration states no floor; a present one must be
    complete.
    """
    if "runtime_compatibility" not in record:
        return None
    compatibility = record["runtime_compatibility"]
    host = compatibility.get("supported_host") if type(compatibility) is dict else None
    if not platform.startswith("linux-"):
        _require(type(host) is dict and "glibc" not in host,
            "The selected native product declares an invalid supported host.")
        return None
    declared = host.get("glibc") if type(host) is dict else None
    found = re.fullmatch(r"glibc ([0-9]+)\.([0-9]+)", declared) if type(declared) is str else None
    _require(found is not None, "The selected native product declares an invalid supported host.")
    return int(found[1]), int(found[2])


def declared_glibc_floor(target):
    """Read only a selected product's declared glibc floor.

    It opens the pinned assembly and its resource build record, not the whole
    product, so a refusal can come before the full read. The full read still
    verifies both again.
    """
    try:
        _require(type(target) is MetaToolkitRuntimeTarget,
            "The native product requires its original application selection.")
        inputs = _Inputs(target.artifact_root)
        _, raw = inputs.read(target.assembly.path, target.assembly.sha256, MAX_PRODUCT_METADATA,
            size=target.assembly.size)
        _, raw = inputs.read(target.assembly.path.parent / "resource-build-record.json",
            _json(raw)["resource_build_record_sha256"], MAX_PRODUCT_METADATA)
        inputs.finish()
        return _declared_glibc_floor(_json(raw), target.target)
    except CandidateArtifactError:
        raise
    except (OSError, ValueError, TypeError, KeyError) as error:
        raise CandidateArtifactError("The selected native product has an absent, changed or malformed input.") from error


def _read_meta_toolkit_artifacts(target, retained=None):
    """Read existing producer relationships against an already selected input.

    No qualification is inferred or replayed. The selected final assembly's
    existing build and installed-product verifier identities must agree. The
    independently admitted source owner remains responsible for accepting them.
    """
    _require(type(target) is MetaToolkitRuntimeTarget,
        "The native product requires its original application selection.")
    _require(retained is None or type(retained) is RetainedPrefixInventory,
        "A retained native prefix inventory needs its bound owner.")
    inputs = _Inputs(target.artifact_root)
    def read(item, limit=MAX_PRODUCT_METADATA, *, keep=True):
        _require(type(item) is ArtifactFile and _positive(item.size),
            "A native product artifact binding is invalid.")
        return inputs.read(item.path, item.sha256, limit, size=item.size, keep=keep)[1]
    def related(path, hashed):
        _, raw = inputs.read(path, hashed, MAX_PRODUCT_METADATA)
        return _json(raw)
    assembly = _json(read(target.assembly))
    development = assembly.get("scope") == "source_built_development_gateway"
    if development:
        expected = {"schema_version", "scope", "origin", "target", "binary_kind", "candidate",
            "staging_record_sha256", "build_record_sha256", "resource_build_record_sha256",
            "resource_component_sha256", "archive", "archive_sha256", "archive_bytes",
            "model_catalog", "sdk_wheels"}
        _require(set(assembly) == expected and type(assembly.get("schema_version")) is int
            and assembly.get("origin") == "source-built-development"
            and target.source_manifest is not None,
            "Development use requires its exact source-built descriptor and original source manifest.")
        candidate = assembly["candidate"]
        _require(type(candidate) is dict and type(candidate.get("binary")) is dict,
            "The development descriptor lacks its actual staged native candidate.")
        # These values are projections of the unchanged actual staging identity,
        # not a fabricated final qualification or installed verifier result.
        assembly = {**assembly, "binary_sha256": candidate["binary"].get("sha256"),
            "binary_bytes": candidate["binary"].get("size"),
            "resources_sha256": candidate.get("resources_sha256")}
    _require(assembly.get("schema_version") == 1
        and assembly.get("scope") == ("source_built_development_gateway" if development
            else "installed_gateway_schema4_resource_startup_and_shutdown_preview")
        and assembly.get("binary_kind") == "toolkit_gateway"
        and assembly.get("target") in PRODUCT_TARGETS and assembly.get("target") == target.target,
        "The selected native product is not its original supported assembly.")
    platform = assembly["target"]
    prefix = "meerkat-connect-" + platform
    qualification = None if development else assembly["qualification"]
    candidate = assembly["candidate"] if development else qualification["candidate"]
    _require(type(candidate) is dict and set(candidate) == {"target", "source_manifest_sha256",
        "resources_sha256", "binary_kind", "binary", "resource_build_record_sha256"}
        and candidate["target"] == platform and candidate["binary_kind"] == "toolkit_gateway"
        and candidate["binary"] == {"sha256": assembly["binary_sha256"], "size": assembly["binary_bytes"]}
        and candidate["resources_sha256"] == assembly["resources_sha256"]
        and candidate["resource_build_record_sha256"] == assembly["resource_build_record_sha256"],
        "The native product candidate differs from its selected final assembly.")
    directory = target.assembly.path.parent
    build = related(directory / "build-record.json", assembly["build_record_sha256"])
    resources = related(directory / "resource-build-record.json", assembly["resource_build_record_sha256"])
    if development:
        staging = related(directory / "staging.json", assembly["staging_record_sha256"])
        _require(type(staging.get("schema_version")) is int and staging.get("schema_version") == 1
            and staging.get("scope") == "unqualified_product_prefix_for_verification"
            and staging.get("target") == platform and staging.get("binary_kind") == "toolkit_gateway"
            and staging.get("prefix") == prefix and staging.get("candidate") == candidate
            and staging.get("build_record_sha256") == assembly["build_record_sha256"]
            and staging.get("resource_build_record_sha256") == assembly["resource_build_record_sha256"]
            and staging.get("resource_component_sha256") == assembly["resource_component_sha256"],
            "The development descriptor differs from its actual original staging record.")
        cargo = build.get("cargo")
        _require(type(cargo) is dict and type(cargo.get("exit_code")) is int and cargo["exit_code"] == 0
            and type(cargo.get("argv")) is list and cargo["argv"]
            and all(type(item) is str for item in cargo["argv"])
            and cargo["argv"].count("--bin") == 1
            and cargo["argv"].index("--bin") + 1 < len(cargo["argv"])
            and cargo["argv"][cargo["argv"].index("--bin") + 1] == "toolkit_gateway"
            and type(cargo.get("env")) is dict
            and cargo["env"].get("MEERKAT_CONNECT_RESOURCES_SHA256") == candidate["resources_sha256"],
            "The development gateway requires its actual successful resource-bound source build.")
        report = None
    else:
        report = related(directory / "gateway-verification.json", qualification["installed_gateway_verification_sha256"])
    _require(build.get("schema_version") == 1 and build.get("target") == platform
        and build.get("binary_kind") == "toolkit_gateway"
        and {key: build["binary"][key] for key in ("sha256", "size")} == candidate["binary"]
        and build["resources_sha256"] == candidate["resources_sha256"]
        and build["resource_build_record_sha256"] == candidate["resource_build_record_sha256"]
        and build["source_manifest"]["sha256"] == candidate["source_manifest_sha256"]
        and resources["target"] == platform and resources["resources_sha256"] == candidate["resources_sha256"]
        and resources["archive_sha256"] == assembly["resource_component_sha256"],
        "The selected native build, resource component or source binding changed.")
    glibc_floor = _declared_glibc_floor(resources, platform)
    source = build["source_manifest"]
    if target.source_manifest is None:
        related(Path(source["path"]), source["sha256"])
    if not development:
        _require(report.get("schema") == 1 and report.get("status") == "passed"
        and report.get("installed_candidate") == candidate
        and report.get("toolkit_gateway_sha256") == candidate["binary"]["sha256"]
        and report.get("raw_gateway") is None and report.get("raw_gateway_sha256") is None
        and [case.get("case") for case in report["cases"]]
            == ["installed-product-success", "installed-product-resource-corruption"],
            "The selected existing installed-product verification differs from the final assembly.")
        observed = report["installed_inputs"]
        _require(observed["native_build"]["sha256"] == assembly["build_record_sha256"]
        and observed["resource_build"]["sha256"] == assembly["resource_build_record_sha256"]
        and observed["source_manifest"]["sha256"] == candidate["source_manifest_sha256"]
        and observed["resource_manifest"]["sha256"] == candidate["resources_sha256"],
            "The existing verifier used a different native product closure.")
    if target.source_manifest is not None:
        manifest = target.source_manifest
        _require(type(manifest) is ArtifactFile and manifest.sha256 == source["sha256"]
            and (development or manifest.sha256 == observed["source_manifest"]["sha256"]
                and type(observed["source_manifest"].get("bytes")) is int
                and manifest.size == observed["source_manifest"]["bytes"]),
            "The selected local source manifest differs from the native build or verifier.")
        # Producer paths remain unchanged provenance. Only this explicit local
        # pin is opened, under the same owned artifact root and byte checks.
        _json(read(manifest))
    if development:
        _require(assembly["model_catalog"] == {"sha256": target.model_catalog.sha256,
                "bytes": target.model_catalog.size},
            "The selected development catalog differs from its producer's actual emitted bytes.")
    else:
        catalog_pin = report["cases"][0]["model_catalog"]
        _require(type(catalog_pin) is dict and set(catalog_pin) == {"path", "sha256"}
            and isinstance(catalog_pin["path"], str) and Path(catalog_pin["path"]).is_absolute()
            and catalog_pin["sha256"] == target.model_catalog.sha256,
            "The selected catalog is not the existing native verifier's actual catalog.")
    catalog = _json(read(target.model_catalog))
    meta_content = read(target.meta_wheel, MAX_WHEEL_BYTES)
    toolkit_content = read(target.toolkit_wheel, MAX_WHEEL_BYTES)
    for artifact, expectation in ((target.meta_wheel, target.meta), (target.toolkit_wheel, target.toolkit)):
        _require((artifact.size, artifact.sha256) == (expectation.size, expectation.sha256),
            "A selected SDK artifact differs from its byte expectation.")
    check_meta_toolkit_wheels(meta_content, toolkit_content, meta=target.meta, toolkit=target.toolkit)
    if development:
        _require(assembly["sdk_wheels"] == {"meta_sha256": target.meta_wheel.sha256,
                "toolkit_sha256": target.toolkit_wheel.sha256},
            "The development descriptor differs from its selected actual SDK wheels.")
    payload = {name: (hashed, size) for expectation in (target.meta, target.toolkit)
        for name, hashed, size in expectation.source_payload}
    sdk_sources = [] if development else report["sdk_sources"]
    sdk_files = {"meerkat_toolkit/host_control.py", "meerkat_toolkit/application_runtime.py",
        "meerkat_meta/__init__.py", "meerkat_meta/builder.py", "meerkat_meta/runtime.py",
        "meerkat_meta/_transport.py", "meerkat_meta/agent_builder.py",
        "meerkat_meta/identity_first_models.py", "meerkat_meta/identity_first_providers.py",
        "meerkat_meta/types.py"}
    _require(development or isinstance(sdk_sources, list) and len(sdk_sources) == len(sdk_files)
        and {"/".join(Path(row["path"]).parts[-2:]) for row in sdk_sources} == sdk_files,
        "The existing native verifier's imported SDK source closure is incomplete.")
    for observed_source in sdk_sources:
        name = "/".join(Path(observed_source["path"]).parts[-2:])
        _require(name in payload and payload[name][0] == observed_source["sha256"],
            "The selected SDK bytes differ from the native verifier's imported SDK source.")
    read(target.dependency_requirements, MAX_METADATA_BYTES)
    _require(type(target.dependency_wheels) is tuple and 0 < len(target.dependency_wheels) <= 256,
        "The native product requires a complete selected offline dependency batch.")
    for dependency in target.dependency_wheels:
        read(dependency, MAX_WHEEL_BYTES, keep=False)
    if __package__:
        from .templates.install_candidate import _read_selected_dependency_wheels
    else:
        from install_candidate import _read_selected_dependency_wheels
    _read_selected_dependency_wheels(target.dependency_requirements, target.dependency_wheels)
    archive_name = assembly["archive"]
    _require(isinstance(archive_name, str) and Path(archive_name).name == archive_name,
        "The native product archive locator is invalid.")
    archive = ArtifactFile(directory / archive_name, assembly["archive_bytes"], assembly["archive_sha256"])
    if retained is not None and retained.matches(target.assembly.sha256, archive, prefix):
        # The inventory receipt.json names was rebuilt from these exact pins
        # by a successful full verification. The Start compares the installed
        # prefix with it; the unread archive keeps its size and fingerprint pin.
        inputs.pin(archive, MAX_PRODUCT_ARCHIVE_BYTES)
        entries, source = retained.entries, "retained"
    else:
        entries, source = product_prefix_entries(read(archive, MAX_PRODUCT_ARCHIVE_BYTES), prefix), "archive"
    inventory = {item.name: item for item in entries}
    gateway = inventory.get(prefix + "/bin/toolkit_gateway")
    manifest = inventory.get(prefix + "/libexec/meerkat-connect/resources.json")
    _require(gateway is not None and gateway.kind == "file" and gateway.mode == 0o755
        and (gateway.sha256, gateway.size) == (candidate["binary"]["sha256"], candidate["binary"]["size"])
        and manifest is not None and manifest.kind == "file" and manifest.mode == 0o444
        and manifest.sha256 == candidate["resources_sha256"],
        "The complete native prefix differs from the selected gateway or resource manifest.")
    inputs.finish()
    result = MetaToolkitRuntimeArtifacts(target, archive, prefix, entries, candidate, catalog,
        tuple(item[0] for item in inputs.files.values()),
        origin="source-built-development" if development else "installed-gateway-assembly",
        glibc_floor=glibc_floor)
    object.__setattr__(result, "_verified_inputs", tuple(inputs.files.values()))
    object.__setattr__(result, "_inventory_source", source)
    if source == "retained":
        object.__setattr__(result, "_known_fingerprints", retained.fingerprints)
    object.__setattr__(result, "_verified_metadata", tuple(json.dumps(value, sort_keys=True,
        ensure_ascii=True, allow_nan=False, separators=(",", ":")).encode()
        for value in (candidate, catalog, result.origin, result.glibc_floor)))
    return result


def read_meta_toolkit_artifacts(target, retained=None):
    try:
        return _read_meta_toolkit_artifacts(target, retained)
    except CandidateArtifactError:
        raise
    except (OSError, ValueError, TypeError, KeyError, IndexError, tarfile.TarError, EOFError) as error:
        raise CandidateArtifactError("The selected native product has an absent, changed or malformed input.") from error


def recheck_meta_toolkit_artifacts(verified, target):
    _require(type(verified) is MetaToolkitRuntimeArtifacts and verified.selection == target
        and verified._verified_inputs, "The checked native product belongs to another selection.")
    try:
        metadata = tuple(json.dumps(value, sort_keys=True, ensure_ascii=True,
            allow_nan=False, separators=(",", ":")).encode()
            for value in (verified.candidate, verified.catalog, verified.origin, verified.glibc_floor))
    except (ValueError, TypeError, RecursionError) as error:
        raise CandidateArtifactError("The checked native product metadata changed.") from error
    _require(verified._verified_metadata and metadata == verified._verified_metadata,
        "The checked native product metadata changed.")
    inputs = _Inputs(target.artifact_root)
    for artifact, fingerprint in verified._verified_inputs:
        with inputs.open(artifact.path, artifact.size) as (_, info):
            _require(_fingerprint(info) == fingerprint, "A selected native product artifact changed.")
            inputs.files[artifact.path] = artifact, fingerprint
    inputs.finish()
    return verified


def _unique(pairs):
    result = {}
    for name, value in pairs:
        _require(name not in result, "Candidate JSON has duplicate keys.")
        result[name] = value
    return result


def _json(content):
    def invalid_constant(_):
        raise CandidateArtifactError("Candidate JSON contains a non-finite number.")
    try:
        result = json.loads(content, object_pairs_hook=_unique, parse_constant=invalid_constant)
        _require(isinstance(result, dict), "Candidate JSON must be an object.")
        return result
    except (UnicodeError, json.JSONDecodeError) as error:
        raise CandidateArtifactError("Candidate JSON is invalid.") from error


def _fingerprint(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
        info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def prefix_inventory_bytes(entries):
    """The exact canonical bytes behind a proof's prefix_inventory_sha256."""
    return json.dumps([asdict(entry) for entry in entries], sort_keys=True, separators=(",", ":")).encode()


def prefix_inventory_sha256(entries):
    return hashlib.sha256(prefix_inventory_bytes(entries)).hexdigest()


class _Inputs:
    def __init__(self, root):
        self.root = Path(root)
        _require(self.root.is_absolute() and str(self.root) == str(root) and ".." not in self.root.parts,
            "The external artifact root must be an absolute canonical path.")
        _require(not any(path.is_symlink() for path in (self.root, *self.root.parents)),
            "The external artifact root contains a link.")
        info = self.root.lstat()
        _require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid(),
            "The external artifact root must be an owned ordinary directory.")
        self.files = {}

    def path(self, value):
        _require(isinstance(value, (str, Path)), "An artifact path is missing.")
        path = Path(value)
        _require(path.is_absolute() and str(path) == str(value)
            and ".." not in path.parts and path.is_relative_to(self.root),
            "Candidate artifacts must stay inside the external artifact root.")
        _require(not any(parent.is_symlink() for parent in (path, *path.parents)),
            "A candidate artifact path contains a link.")
        _require(path.parent.stat().st_uid == os.getuid(),
            "A candidate artifact directory is not owned by this user.")
        return path

    @contextmanager
    def open(self, path, limit):
        path = self.path(path)
        before = path.lstat()
        _require(stat.S_ISREG(before.st_mode), "A candidate artifact is not an ordinary file.")
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as handle:
            info = os.fstat(handle.fileno())
            _require(_fingerprint(before) == _fingerprint(info)
                and info.st_uid == os.getuid() and info.st_nlink == 1
                and 0 < info.st_size <= limit,
                "A candidate artifact is not an owned bounded ordinary file.")
            if path in self.files:
                _require(self.files[path][1] == _fingerprint(info),
                    "A candidate artifact changed during verification.")
            yield handle, info
            _require(_fingerprint(info) == _fingerprint(os.fstat(handle.fileno()))
                and _fingerprint(info) == _fingerprint(path.lstat()),
                "A candidate artifact changed during verification.")

    def read(self, path, expected, limit, *, keep=True, size=None):
        _require(_hex(expected), "An artifact SHA-256 binding is invalid.")
        path = self.path(path)
        hashed, total, chunks = hashlib.sha256(), 0, []
        with self.open(path, limit) as (handle, info):
            while chunk := handle.read(1024 * 1024):
                total += len(chunk)
                _require(total <= limit, "A candidate artifact exceeds its byte bound.")
                hashed.update(chunk)
                if keep:
                    chunks.append(chunk)
            _require(total == info.st_size and (size is None or total == size)
                and hashed.hexdigest() == expected, "Candidate artifact bytes do not match their binding.")
            artifact = ArtifactFile(path, total, expected)
            self.files[path] = artifact, _fingerprint(info)
        return artifact, b"".join(chunks) if keep else None

    def pin(self, artifact, limit):
        """Bind an unread input by size and fingerprint for the later rechecks."""
        path = self.path(artifact.path)
        with self.open(path, limit) as (_, info):
            _require(info.st_size == artifact.size, "Candidate artifact bytes do not match their binding.")
            self.files[path] = artifact, _fingerprint(info)
        return artifact

    def finish(self):
        for path, (_, fingerprint) in self.files.items():
            self.path(path)
            _require(_fingerprint(path.lstat()) == fingerprint,
                "A candidate artifact changed during verification.")


class _BoundedExpansion:
    def __init__(self, handle):
        self.handle, self.total = handle, 0

    def read(self, size=-1):
        remaining = MAX_BINARY_BYTES + MAX_TAR_OVERHEAD + 1 - self.total
        data = self.handle.read(remaining if size < 0 else min(size, remaining))
        self.total += len(data)
        _require(self.total <= MAX_BINARY_BYTES + MAX_TAR_OVERHEAD,
            "A native archive expands beyond the installer bound.")
        return data


def _native(inputs, directory, record, binding):
    _require(isinstance(record, dict) and set(record) == {"binary", "inner", "name", "sha256", "size", "target"},
        "A native archive record is invalid.")
    binary, inner = record["binary"], record["inner"]
    _require(binary in BINARIES and record["target"] == binding.platform
        and record["name"] == f"{BINARIES[binary]}-{binding.native_version}-{binding.platform}.tar.gz"
        and _positive(record["size"]) and record["size"] <= MAX_ARCHIVE_BYTES
        and isinstance(inner, dict) and set(inner) == {"name", "sha256", "size"}
        and inner["name"] == binary and _positive(inner["size"])
        and inner["size"] <= MAX_BINARY_BYTES and _hex(inner["sha256"]),
        "A native archive identity or size is invalid.")
    artifact, _ = inputs.read(directory / record["name"], record["sha256"],
        MAX_ARCHIVE_BYTES, keep=False, size=record["size"])
    with inputs.open(artifact.path, MAX_ARCHIVE_BYTES) as (handle, _):
        with gzip.GzipFile(fileobj=handle, mode="rb") as compressed:
            expanded = _BoundedExpansion(compressed)
            with tarfile.open(fileobj=expanded, mode="r|") as archive:
                member = archive.next()
                _require(member is not None and member.name == binary and member.isfile()
                    and member.sparse is None and member.size == inner["size"]
                    and member.mode & 0o111, "A native archive must contain one flat ordinary executable.")
                source = archive.extractfile(member)
                _require(source is not None, "The native archive payload is missing.")
                hashed, size = hashlib.sha256(), 0
                with source:
                    while chunk := source.read(1024 * 1024):
                        size += len(chunk)
                        _require(size <= MAX_BINARY_BYTES, "A native archive payload exceeds its bound.")
                        hashed.update(chunk)
                _require(size == inner["size"] and hashed.hexdigest() == inner["sha256"]
                    and archive.next() is None, "The native archive payload or flat inventory changed.")
            while expanded.read(1024 * 1024):
                pass
    return NativeArtifact(binary, artifact, inner["name"], inner["size"], inner["sha256"])


def _wheel(content, version, payload, sizes):
    _require(isinstance(payload, dict) and payload and len(payload) <= MAX_WHEEL_FILES
        and all(isinstance(name, str) and name.startswith("meerkat_mobkit/")
            and _hex(hashed) for name, hashed in payload.items()), "The SDK source inventory is invalid.")
    prefix = f"meerkat_mobkit-{version}.dist-info/"
    metadata_names = {prefix + name for name in ("METADATA", "WHEEL", "RECORD", "top_level.txt")}
    try:
        values = read_wheel_members(content, expected_names=set(payload) | metadata_names,
            max_file_bytes=MAX_WHEEL_BYTES, max_total_bytes=MAX_WHEEL_TOTAL, max_files=MAX_WHEEL_FILES)
    except WheelIntegrityError as error:
        raise CandidateArtifactError(str(error)) from error
    metadata = BytesParser().parsebytes(values[prefix + "METADATA"])
    wheel = BytesParser().parsebytes(values[prefix + "WHEEL"])
    _require(metadata.get_all("Name") == ["meerkat-mobkit"]
        and metadata.get_all("Version") == [version]
        and wheel.get_all("Wheel-Version") == ["1.0"]
        and wheel.get_all("Root-Is-Purelib") == ["true"]
        and wheel.get_all("Tag") == ["py3-none-any"]
        and values[prefix + "top_level.txt"].strip() == b"meerkat_mobkit",
        "The SDK wheel has an unexpected package or platform identity.")
    try:
        verify_wheel_record(values, prefix + "RECORD")
    except WheelIntegrityError as error:
        raise CandidateArtifactError(str(error)) from error
    _require(all(_digest(values[name]) == hashed and type(sizes[name]) is int
        and len(values[name]) == sizes[name] for name, hashed in payload.items()),
        "The SDK wheel payload differs from its exact source inventory.")
    return tuple(sorted(payload.items()))


def recheck_candidate_artifacts(verified, *, tuple_path, accepted, artifact_root):
    """Recheck owned files for explicit reuse inside one synchronous transition.

    Only a frozen result populated by the full reader has an input snapshot.
    Independent admission fences must still call the full reader afresh. This
    does not persist admission, cache artifacts, or follow an authored flag.
    """
    try:
        _require(type(verified) is CandidateArtifacts and verified._verified_inputs
            and type(accepted) is CandidateBinding,
            "Explicit reuse requires a full reader's verified artifact result.")
        inputs = _Inputs(artifact_root)
        _require(verified._artifact_root == inputs.root and inputs.path(tuple_path) == verified.tuple_file.path
            and all(type(getattr(verified.identity, key)) is type(getattr(accepted, key))
                and getattr(verified.identity, key) == getattr(accepted, key)
                for key in CandidateBinding.__dataclass_fields__),
            "The verified artifacts belong to another exact candidate selection.")
        for artifact, fingerprint in verified._verified_inputs:
            with inputs.open(artifact.path, artifact.size) as (_, info):
                _require(_fingerprint(info) == fingerprint,
                    "A candidate artifact changed since the full verification.")
                inputs.files[artifact.path] = artifact, fingerprint
        inputs.finish()
        return verified
    except CandidateArtifactError:
        raise
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        raise CandidateArtifactError("The verified artifact snapshot or owned files changed.") from None


def read_candidate_artifacts(tuple_path, *, accepted: CandidateBinding, artifact_root: Path) -> CandidateArtifacts:
    """Recheck one already admitted external tuple without changing any state.

    ``accepted`` must come from the caller's protected host acceptance, never
    reconstructed as authority from this file. The external root keeps native
    payloads outside the bounded text application package. Returned paths are
    verified read-time metadata, not open handles or permission to install.
    """
    try:
        _require(type(accepted) is CandidateBinding, "An explicit caller-accepted candidate binding is required.")
        identity = asdict(accepted)
        tuple_digest = identity.pop("tuple_sha256")
        _require(_hex(tuple_digest) and accepted.repository == "lukacf/meerkat-mobkit"
            and isinstance(accepted.source_sha, str) and re.fullmatch(r"[a-f0-9]{40}", accepted.source_sha)
            and all(isinstance(value, str) and VERSION.fullmatch(value)
                for value in (accepted.native_version, accepted.core_version))
            and accepted.platform in PLATFORMS and _positive(accepted.native_run_id)
            and _positive(accepted.native_run_attempt) and _hex(accepted.native_manifest_sha256)
            and _hex(accepted.sdk_wheel_sha256), "The caller-accepted candidate identity is invalid.")
        inputs = _Inputs(artifact_root)
        tuple_file, content = inputs.read(tuple_path, tuple_digest, MAX_METADATA_BYTES)
        value = _json(content)
        _require(value.get("format") == "forge-private-consumer-tuple-preparation-v1"
            and value.get("identity") == identity and all(type(value["identity"][key]) is type(item)
                for key, item in identity.items()), "The candidate tuple differs from the caller-accepted identity.")
        native, sdk = value["native"], value["sdk"]
        _require(native["origin"] == "github-actions-signed-candidate"
            and sdk["origin"] == "local-build-from-exact-frozen-source"
            and sdk["signed"] is False and sdk["published"] is False,
            "The candidate native and local SDK origins must remain distinct.")
        metadata, target = native["metadata"], native["target"]
        def proof(container, path_key, hash_key):
            return inputs.read(container[path_key], container[hash_key], MAX_METADATA_BYTES)[1]
        intake = _json(proof(native, "intake", "intake_sha256"))
        selection = _json(proof(native, "selection", "selection_sha256"))
        manifest = _json(proof(metadata, "manifest_path", "manifest_sha256"))
        expected = {"repository": accepted.repository, "source_sha": accepted.source_sha,
            "version": accepted.native_version, "tag": "v" + accepted.native_version,
            "run_id": accepted.native_run_id, "run_attempt": accepted.native_run_attempt,
            "kind": "candidate-manifest", "schema_version": 1}
        _require(all(type(manifest.get(key)) is type(item) and manifest.get(key) == item for key, item in expected.items())
            and metadata["manifest_sha256"] == accepted.native_manifest_sha256
            and manifest["workflow"] == {"path": ".github/workflows/release.yml", "ref": "refs/heads/main", "sha": accepted.source_sha},
            "The native manifest source, version, workflow or run binding changed.")
        _require(all(type(intake.get(key)) is type(item) and intake.get(key) == item for key, item in {
            "repository": accepted.repository, "source_sha": accepted.source_sha,
            "native_version": accepted.native_version, "core_version": accepted.core_version,
            "run_id": accepted.native_run_id, "run_attempt": accepted.native_run_attempt,
            "target": accepted.platform}.items()) and intake["workflow"] == manifest["workflow"]
            and intake["metadata_artifact"] == metadata and intake["target_artifact"] == target
            and intake["selection_path"] == native["selection"], "The native intake binding changed.")
        _require(type(selection.get("schema_version")) is int
            and type(selection.get("run_id")) is int and type(selection.get("run_attempt")) is int
            and type(selection.get("manifest_artifact", {}).get("id")) is int
            and selection == {"kind": "accepted-candidate", "schema_version": 1,
            "repository": accepted.repository, "run_id": accepted.native_run_id,
            "run_attempt": accepted.native_run_attempt, "manifest_artifact": {
                "id": metadata["id"], "digest": metadata["outer_sha256"],
                "manifest_sha256": accepted.native_manifest_sha256}}, "The native selection binding changed.")
        artifacts = manifest["artifacts"]
        _require(isinstance(artifacts, list) and all(isinstance(item, dict) for item in artifacts)
            and len({item["target"] for item in artifacts}) == len(artifacts)
            and len({item["id"] for item in artifacts}) == len(artifacts), "The native manifest has duplicate target artifacts.")
        selected = [item for item in artifacts if item["target"] == accepted.platform]
        _require(len(selected) == 1 and _positive(target["id"]) and _positive(metadata["id"])
            and selected[0]["id"] == target["id"] and selected[0]["digest"] == target["outer_sha256"]
            and selected[0]["archives"] == target["archives"], "The native target artifact binding changed.")
        proof(metadata, "original_provenance_path", "original_provenance_sha256")
        proof(target, "original_provenance_path", "original_provenance_sha256")
        inputs.read(metadata["path"], metadata["outer_sha256"], MAX_ARCHIVE_BYTES, keep=False)
        _require(_positive(target["bytes"]), "The native target byte binding is invalid.")
        inputs.read(target["path"], target["outer_sha256"], MAX_ARCHIVE_BYTES, keep=False, size=target["bytes"])
        records = target["archives"]
        _require(isinstance(records, list) and len(records) == len(BINARIES)
            and all(isinstance(record, dict) for record in records)
            and {record["binary"] for record in records} == set(BINARIES), "The native archive inventory is incomplete or duplicated.")
        directory = inputs.path(target["original_provenance_path"]).parent
        natives = tuple(sorted((_native(inputs, directory, record, accepted) for record in records), key=lambda item: item.binary))
        provenance = _json(proof(sdk, "provenance", "provenance_sha256"))
        review = _json(proof(sdk, "independent_review", "independent_review_sha256"))
        _require(provenance["format"] == "forge-local-candidate-sdk-preparation-v1"
            and provenance["repository"] == accepted.repository and provenance["source_sha"] == accepted.source_sha
            and provenance["native_candidate_reference"]["run_id"] == accepted.native_run_id
            and provenance["native_candidate_reference"]["run_attempt"] == accepted.native_run_attempt
            and review["source_sha"] == accepted.source_sha and review["provenance_sha256"] == sdk["provenance_sha256"]
            and review["wheel_sha256"] == sdk["wheel_sha256"] == accepted.sdk_wheel_sha256,
            "The local SDK source or native reference binding changed.")
        declared = provenance["sdk"]
        _require(declared["name"] == "meerkat-mobkit" and declared["version"] == accepted.native_version
            and declared["sha256"] == sdk["wheel_sha256"] and declared["bytes"] == sdk["bytes"]
            and _positive(sdk["bytes"]) and declared["origin"] == sdk["origin"]
            and declared["signed"] is False and declared["published"] is False
            and declared["native_executable_included"] is False,
            "The local SDK identity or origin changed.")
        source_files = provenance["source_files"]
        _require(isinstance(source_files, list) and all(isinstance(item, dict) and isinstance(item["path"], str) for item in source_files)
            and len({item["path"] for item in source_files}) == len(source_files), "The SDK source file inventory is invalid.")
        source_payload = {item["path"].removeprefix("sdk/python/"): item["sha256"] for item in source_files
            if item["path"].startswith("sdk/python/meerkat_mobkit/")}
        _require(source_payload == sdk["source_payload"], "The SDK payload inventory differs from its exact source.")
        wheel_path = inputs.path(sdk["wheel"])
        _require(wheel_path.name == f"meerkat_mobkit-{accepted.native_version}-py3-none-any.whl",
            "The SDK wheel filename identity changed.")
        wheel_file, wheel_bytes = inputs.read(wheel_path, accepted.sdk_wheel_sha256, MAX_WHEEL_BYTES, size=sdk["bytes"])
        payload_sizes = {item["path"].removeprefix("sdk/python/"): item["bytes"] for item in source_files
            if item["path"].startswith("sdk/python/meerkat_mobkit/")}
        payload = _wheel(wheel_bytes, accepted.native_version, sdk["source_payload"], payload_sizes)
        inputs.finish()
        evidence = tuple(sorted((item[0] for item in inputs.files.values()
            if item[0] not in (tuple_file, wheel_file) and item[0] not in {native.archive for native in natives}),
            key=lambda item: str(item.path)))
        result = CandidateArtifacts(accepted, tuple_file, natives, SdkArtifact(wheel_file, payload), evidence)
        object.__setattr__(result, "_artifact_root", inputs.root)
        object.__setattr__(result, "_verified_inputs", tuple(inputs.files.values()))
        return result
    except CandidateArtifactError:
        raise
    except (OSError, ValueError, KeyError, TypeError, AttributeError, NotImplementedError,
            tarfile.TarError, zipfile.BadZipFile, EOFError) as error:
        raise CandidateArtifactError("The candidate artifact tuple or archive is invalid.") from error
