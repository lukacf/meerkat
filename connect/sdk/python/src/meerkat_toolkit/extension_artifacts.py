"""Verify host-selected installed Python extensions before importing an entrypoint.

Package compatibility and selection belong to the canonical package inspector
and host. This module does not install, resolve dependencies, authorize access,
activate a frontend, or own the loaded extension's lifetime. The extracted
verification is licensed under MIT OR Apache-2.0 (see LICENSE-EXTRACTED and
EXTENSIONS.md).
"""

from __future__ import annotations

from collections.abc import Collection
import csv
from dataclasses import dataclass
from email.parser import BytesParser
import importlib
from importlib.machinery import PathFinder
import io
import os
from pathlib import Path, PurePosixPath
import sys
from typing import Any
import unicodedata

from packaging.markers import default_environment
from packaging.requirements import InvalidRequirement, Requirement
from packaging.specifiers import InvalidSpecifier, SpecifierSet
from packaging.utils import InvalidWheelFilename, parse_wheel_filename
from packaging.version import InvalidVersion, Version

from ._extension_wheels import (
    ExtensionArtifactError,
    MAX_TOTAL_BYTES,
    _INSTALLER_FILES,
    _assert_path_chain,
    _canonical_name,
    _digest,
    _dist_info_component,
    _regular_file,
    _sha256,
    _verify_record,
    read_wheel_payload,
    verify_installed,
)
from .host_services import HostServiceError, StagedHostServices


@dataclass(frozen=True)
class SelectedWheel:
    """Exact artifact selected by a trusted host; this is not authorization."""

    path: Path
    sha256: str


@dataclass(frozen=True)
class ExtensionArtifacts:
    """Primary wheel and the complete, already selected dependency wheel set.

    ``reserved_import_roots`` names the trusted host's own top-level packages.
    A selected wheel that provides one of them is refused before any import.
    """

    primary: SelectedWheel
    dependencies: tuple[SelectedWheel, ...] = ()
    reserved_import_roots: tuple[str, ...] = ()


@dataclass(frozen=True)
class ExtensionArtifactObservation:
    """Observed software identity only, with no grants or lifecycle meaning."""

    distribution: str
    version: str
    wheel_sha256: str
    dependencies: tuple[tuple[str, str, str], ...]
    import_roots: tuple[str, ...]
    primary_dist_info: Path
    primary_files: tuple[tuple[str, str], ...]
    primary_requirements: tuple[str, ...]


@dataclass
class _Wheel:
    name: str
    version: str
    digest: str
    info: str
    files: dict[str, bytes]
    roots: tuple[str, ...]
    requirements: tuple[str, ...]
    archive_bytes: int


# Import roots of the Meerkat platform and the verifier's own dependency.
_PLATFORM_ROOTS = frozenset({"meerkat", "meerkat_meta", "meerkat_mobkit", "meerkat_toolkit", "packaging"})


def _inspect(selected: SelectedWheel, environment: dict[str, str],
             host_roots: frozenset[str] = frozenset()) -> _Wheel:
    if not isinstance(selected, SelectedWheel):
        raise ExtensionArtifactError("expected an exact selected wheel")
    path = Path(selected.path)
    _digest(selected.sha256, label="wheel digest")
    data = _regular_file(path, label="selected wheel")
    if _sha256(data) != selected.sha256:
        raise ExtensionArtifactError("selected wheel digest differs")
    try:
        filename_name, filename_version, _build, tags = parse_wheel_filename(path.name)
    except InvalidWheelFilename as error:
        raise ExtensionArtifactError("invalid wheel filename") from error
    if {str(tag) for tag in tags} != {"py3-none-any"}:
        raise ExtensionArtifactError("extension profile requires py3-none-any wheels")
    files = read_wheel_payload(data)
    roots = {PurePosixPath(name).parts[0] for name in files}
    infos = {name for name in roots if name.endswith(".dist-info")}
    if len(infos) != 1:
        raise ExtensionArtifactError("wheel must have one dist-info root")
    info = infos.pop()
    if any(f"{info}/{name}" not in files for name in ("METADATA", "WHEEL", "RECORD")):
        raise ExtensionArtifactError("wheel metadata is incomplete")
    metadata = BytesParser().parsebytes(files[f"{info}/METADATA"])
    names, versions = metadata.get_all("Name", []), metadata.get_all("Version", [])
    if len(names) != 1 or len(versions) != 1:
        raise ExtensionArtifactError("wheel identity is ambiguous")
    name, version = _canonical_name(names[0]), versions[0]
    try:
        version_matches = Version(version) == filename_version
    except InvalidVersion as error:
        raise ExtensionArtifactError("invalid wheel version") from error
    if (name != filename_name or not version_matches
            or info != f"{_dist_info_component(name)}-{_dist_info_component(version)}.dist-info"):
        raise ExtensionArtifactError("wheel filename and metadata identity differ")
    wheel = BytesParser().parsebytes(files[f"{info}/WHEEL"])
    if ([value.lower() for value in wheel.get_all("Root-Is-Purelib", [])] != ["true"]
            or wheel.get_all("Wheel-Version", []) != ["1.0"]
            or set(wheel.get_all("Tag", [])) != {"py3-none-any"}):
        raise ExtensionArtifactError("wheel is not supported pure Python")
    constraints = metadata.get_all("Requires-Python", [])
    try:
        if len(constraints) > 1 or (constraints and not SpecifierSet(constraints[0]).contains(
                environment["python_full_version"], prereleases=True)):
            raise ExtensionArtifactError("wheel Requires-Python is not satisfied")
    except InvalidSpecifier as error:
        raise ExtensionArtifactError("invalid wheel Requires-Python") from error
    if any(PurePosixPath(p).suffix.lower() in {".so", ".pyd", ".dll", ".dylib", ".pyc"}
           for p in files):
        raise ExtensionArtifactError("native or precompiled wheel code is unsupported")
    if any(f"{info}/{name}" in files for name in _INSTALLER_FILES - {"RECORD"}):
        raise ExtensionArtifactError("wheel contains installer-owned metadata")
    import_roots = []
    for root in roots - {info}:
        import_name = root[:-3] if root.endswith(".py") else root
        reserved = sys.stdlib_module_names | _PLATFORM_ROOTS | host_roots
        if (not import_name.isidentifier() or import_name.casefold() in {name.casefold() for name in reserved}
                or (not root.endswith(".py") and f"{root}/__init__.py" not in files)):
            raise ExtensionArtifactError("wheel import root is unsupported")
        import_roots.append(import_name)
    if not import_roots:
        raise ExtensionArtifactError("wheel has no Python import root")
    _verify_record(data=files[f"{info}/RECORD"], record_path=f"{info}/RECORD",
                   payloads=files, label="wheel RECORD")
    return _Wheel(name, version, selected.sha256, info, files, tuple(sorted(import_roots)),
                  tuple(metadata.get_all("Requires-Dist", [])), len(data))


def _validate_graph(wheels: list[_Wheel], environment: dict[str, str]) -> None:
    by_name = {wheel.name: wheel for wheel in wheels}
    if len(by_name) != len(wheels):
        raise ExtensionArtifactError("selected distribution identity is duplicated")
    pending, reached = [wheels[0].name], set()
    while pending:
        name = pending.pop()
        if name in reached:
            continue
        reached.add(name)
        for raw in by_name[name].requirements:
            try:
                requirement = Requirement(raw)
            except InvalidRequirement as error:
                raise ExtensionArtifactError("invalid Requires-Dist") from error
            if requirement.extras or requirement.url:
                raise ExtensionArtifactError("dependency extras and direct URLs are unsupported")
            if requirement.marker and not requirement.marker.evaluate(environment):
                continue
            child = _canonical_name(requirement.name)
            if child not in by_name or not requirement.specifier.contains(
                    by_name[child].version, prereleases=True):
                raise ExtensionArtifactError("dependency is outside the selected closure")
            pending.append(child)
    if reached != set(by_name):
        raise ExtensionArtifactError("selected closure includes unreachable dependencies")


def _import_ownership(wheels: list[_Wheel], site: Path) -> None:
    owners: dict[str, _Wheel] = {}
    for wheel in wheels:
        for root in wheel.roots:
            key = unicodedata.normalize("NFC", root).casefold()
            if key in owners:
                raise ExtensionArtifactError("selected import roots overlap")
            owners[key] = wheel
    selected_infos = {wheel.info for wheel in wheels}
    for info in site.glob("*.dist-info"):
        if info.name in selected_infos:
            continue
        _assert_path_chain(site, f"{info.name}/METADATA")
        metadata = BytesParser().parsebytes(_regular_file(info / "METADATA", label="installed identity"))
        if any(_canonical_name(name) in {w.name for w in wheels}
               for name in metadata.get_all("Name", [])):
            raise ExtensionArtifactError("another installed distribution has the selected identity")
        _assert_path_chain(site, f"{info.name}/RECORD")
        raw = _regular_file(info / "RECORD", label="installed ownership record")
        # Other distributions may own scripts outside site-packages. Normalize
        # lexically, never read those paths, and compare only selected roots.
        for row in csv.reader(io.StringIO(raw.decode("utf-8"))):
            if not row:
                continue
            claimed = Path(os.path.normpath(str(site / row[0])))
            try:
                relative = claimed.relative_to(site)
            except ValueError:
                continue
            first = relative.parts[0] if relative.parts else ""
            root = first[:-3] if first.endswith(".py") else first
            if unicodedata.normalize("NFC", root).casefold() in owners:
                raise ExtensionArtifactError("another installed distribution owns a selected root")
    for wheel in wheels:
        for root in wheel.roots:
            expected = site / (root + ".py" if root + ".py" in wheel.files else root + "/__init__.py")
            visible = False
            for path in sys.path:
                spec = PathFinder.find_spec(root, [path or os.getcwd()])
                if spec is None:
                    continue
                if spec.origin != str(expected):
                    raise ExtensionArtifactError("selected root exists on another import path")
                visible = True
            if not visible:
                raise ExtensionArtifactError("selected root is not on the current import path")
            for module_name, module in tuple(sys.modules.items()):
                if module_name != root and not module_name.startswith(root + "."):
                    continue
                namespace = getattr(module, "__dict__", {})
                path = module_name.replace(".", "/")
                candidates = [p for p in (path + ".py", path + "/__init__.py") if p in wheel.files]
                origin = namespace.get("__file__")
                spec = namespace.get("__spec__")
                if (len(candidates) != 1 or origin != str(site / candidates[0])
                        or spec is None or spec.origin != origin
                        or ("__path__" in namespace and list(namespace["__path__"]) != [str(Path(origin).parent)])):
                    raise ExtensionArtifactError("loaded module origin differs from selected payload")


def _verify(selection: ExtensionArtifacts, site_packages: Path) -> list[_Wheel]:
    if not isinstance(selection, ExtensionArtifacts) or not isinstance(selection.dependencies, tuple):
        raise ExtensionArtifactError("expected an immutable artifact selection")
    host_roots = selection.reserved_import_roots
    if (not isinstance(host_roots, tuple) or len(host_roots) > 64
            or any(not isinstance(root, str) or not root.isidentifier() for root in host_roots)):
        raise ExtensionArtifactError("reserved import roots must be identifiers")
    selected = (selection.primary, *selection.dependencies)
    if not 1 <= len(selected) <= 256:
        raise ExtensionArtifactError("selected wheel count exceeds its limit")
    site = Path(site_packages)
    if not site.is_absolute() or site.resolve() != site or not site.is_dir():
        raise ExtensionArtifactError("site-packages must be an existing canonical absolute directory")
    environment = {**default_environment(), "extra": ""}
    wheels = []
    total = archive_total = 0
    for artifact in selected:
        wheel = _inspect(artifact, environment, frozenset(host_roots))
        total += sum(len(data) for data in wheel.files.values())
        archive_total += wheel.archive_bytes
        if max(total, archive_total) > MAX_TOTAL_BYTES:
            raise ExtensionArtifactError("selected artifact bytes exceed their limit")
        wheels.append(wheel)
    _validate_graph(wheels, environment)
    _import_ownership(wheels, site)
    for wheel in wheels:
        verify_installed(site, distribution=wheel.name, version=wheel.version,
                         installed_files=[{"file": name, "sha256": _sha256(data)}
                                          for name, data in wheel.files.items()
                                          if name != f"{wheel.info}/RECORD"])
    return wheels


def verify_extension_artifacts(selection: ExtensionArtifacts, *, site_packages: Path) -> ExtensionArtifactObservation:
    """Re-read selected artifacts and installed bytes without importing their code.

    Manifest-derived bytecode caches are discarded by the original verifier;
    other installed files are never rewritten. This is a local software check,
    not a sandbox, authorization token or continuous tamper-proof attestation.
    """
    wheels = _verify(selection, site_packages)
    first = wheels[0]
    return ExtensionArtifactObservation(first.name, first.version, first.digest,
        tuple((w.name, w.version, w.digest) for w in wheels[1:]),
        tuple(root for w in wheels for root in w.roots),
        Path(site_packages) / first.info,
        tuple((name, _sha256(data)) for name, data in sorted(first.files.items())
              if name != f"{first.info}/RECORD"),
        first.requirements)


def load_extension_entrypoint(
    selection: ExtensionArtifacts, *, site_packages: Path, entrypoint: str,
    services: StagedHostServices, required_services: Collection[str] = (),
) -> Any:
    """Revalidate and import a selected entrypoint, without calling or activating it.

    The trusted host obtains entrypoint and service requirements from its
    canonical validated selection. Service version/authority validation remains
    there; the supplied handles must enforce their own revocation and lifetime.
    """
    if not isinstance(services, StagedHostServices) or not services.ready:
        raise ExtensionArtifactError("host services are not bound")
    if (isinstance(required_services, (str, bytes))
            or not isinstance(required_services, Collection)
            or any(not isinstance(name, str) or not name for name in required_services)):
        raise ExtensionArtifactError("required services must be explicit names")
    try:
        for name in required_services:
            services.get(name)
    except HostServiceError as error:
        raise ExtensionArtifactError("required host service is unavailable") from error
    if not isinstance(entrypoint, str) or entrypoint.count(":") != 1:
        raise ExtensionArtifactError("invalid extension entrypoint")
    module_name, attribute = entrypoint.split(":")
    if not all(part.isidentifier() for part in (*module_name.split("."), *attribute.split("."))):
        raise ExtensionArtifactError("invalid extension entrypoint")
    wheels = _verify(selection, site_packages)
    source = module_name.replace(".", "/")
    if not any(path in wheels[0].files for path in (source + ".py", source + "/__init__.py")):
        raise ExtensionArtifactError("entrypoint is not owned by the primary wheel")
    value = importlib.import_module(module_name)
    for part in attribute.split("."):
        value = getattr(value, part)
    return value
