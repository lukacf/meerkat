#!/usr/bin/env python3
"""Copy the accepted catalog and local installation inputs into a new directory.

This exports existing bytes. It does not install, download, discover, authenticate
or activate anything. pip and npm remain the installation tools.
"""
from __future__ import annotations

import argparse
import base64
import email.parser
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
from typing import NamedTuple
import zipfile

ROOT = Path(__file__).resolve().parents[1]
AGNO = "docs/verification/20261003-agno-postgres-batch"
AGNO_SEED = 'dist/agno-consumer-20261004-wikipedia-metadata'
AGNO_INPUTS = 'docs/verification/20261011-agno-startup-isolation/agno-seed'
AP = "docs/verification/20261003-activepieces-effects-expansion"
CONFIG = "docs/verification/20261003-config-assistance"
AUTH = "docs/verification/20261003-auth-prerequisites"
CREDENTIAL = 'docs/verification/20261003-credential-environment'
PROMOTION = 'docs/verification/20261003-credential-environment/promotion'
AP_SEED = 'docs/verification/20261004-activepieces-notice-delivery/author/consumer-seed'
SERVICES = 'docs/verification/20261008-native-connector-packages/canonical-adoption/services'
SERVICES_ACCEPTANCE = SERVICES + '/component-inputs.json'
SERVICES_SEED = SERVICES
SERVICES_ARTIFACT = 'docs/verification/20261008-native-connector-packages/services/artifacts/meerkat_toolkit_services-0.1.23-py3-none-any.whl'
CATALOG = "crates/toolkit-package/catalog/adopted.json"
CATALOG_SHA = '55ee02453599051cb0a668ef2102db6a01222063651637473411c41a489b65e6'
DISCOVERY_CATALOG = "crates/toolkit-package/catalog/discovery.json"
DISCOVERY_CATALOG_SHA = '75318c335af424794142887d405b0d9e4e8ed5e390c51885527049f3e466111a'
AGNO_WHEEL = "meerkat_toolkit_agno-0.1.5-py3-none-any.whl"
AP_ARCHIVE = "rkat-toolkit-activepieces-0.1.0.tgz"
SERVICES_WHEEL = 'meerkat_toolkit_services-0.1.23-py3-none-any.whl'
SERVICES_SHA = 'd96f63711cf16b383319e4f34e1d1ac282b6e8849e6cc838d16cc2386d7819d9'
# Export the accepted library edition while selected-package work evolves.
INSTALLER = "dist/local-consumers-20261003-permalink-macos/install-library-consumer.py"
INSTALLER_SHA = "106ba8c23a411ed16f119b3345dec6574bfab27bc72b794ec63f3859c8230c1c"
INSTALLER_ACCEPTANCE = "docs/verification/20261003-library-reconciliation/acceptance.json"
AGNO_ARTIFACT = 'docs/verification/20261011-agno-startup-isolation/artifacts/meerkat_toolkit_agno-0.1.5-py3-none-any.whl'
AGNO_ACCEPTANCE = 'docs/verification/20261011-agno-startup-isolation/agno-adoption.json'
AP_ARTIFACT = 'docs/verification/20261004-activepieces-source/artifacts/rkat-toolkit-activepieces-0.1.0.tgz'
AP_ACCEPTANCE = 'docs/verification/20261004-activepieces-notice-delivery/author/activepieces-adoption.json'
AGNO_SHA = '70aba6b1ac7d9ca65c5af11bd68d57f6e34599bb81243df7c1576871705c77a3'
AP_SHA = 'dc5cd3ea4d36203524370fe841730c8d2d81fc932d1ef56fa3e5a4feab6123dc'
PINS = {
    AP_ACCEPTANCE: "058f91cc418ea5bc4ac7e23672089d07138b04d845e858f688561d396d8c28b1",
    AGNO_ACCEPTANCE: '81eff3fa696d2a50161506dd0408d9b431316213d7a36892a984881699f205d5',
    AGNO_INPUTS + "/seed-manifest.json": "13c795b03186c43e9d20c75c988831f21d942bb6a5de03c85513edf7cf9d0801",
    "docs/verification/20261003-credential-environment/promotion/activepieces-adoption.json": "f849ca5b5b8e0afa9ac745dbeddc47001e588223ef0ba5f79f1426c7d1721fc1",
    SERVICES_ACCEPTANCE: '91198de25b76574318e4c565265aae6a4c7f6a84b17893f9337d8e1d9c0262d3',
    INSTALLER_ACCEPTANCE: "fd194a14a5b2220c842d1f388ece51cb502c21b544d43271dfc5cd8cbbce05da",
}
DEPENDENCIES = 'dist/consumer-dependencies-20261003-zendesk-oauth-macos'
DEPENDENCY_CLOSURE = 'docs/verification/20261011-agno-startup-isolation/closure-macos.json'
ACQUISITION = "docs/verification/20261003-offline-consumers/acquisition"
DEPENDENCY_PINS = {
    DEPENDENCY_CLOSURE: '2ae2dc59d4b974599f897d56596ed98daf3ccff0e336eac34d94de5d1e03f2e9',
    "dist/consumer-dependencies-20261003-zendesk-oauth-macos/closure.json": "ac8f69dda4f00bac0e788f342d491af715a923cc0c66c620dbb432c1a18b1f5b",
    "docs/verification/20261003-offline-consumers/acquisition/acquire.py": "dd6f2dd45494372b313eb7d35ab9feab8812983ffb1080c11f401861c8e2074c"
}
LINUX_DEPENDENCIES = "dist/consumer-dependencies-20261003-zendesk-oauth-linux"
LINUX_DEPENDENCY_CLOSURE = 'docs/verification/20261011-agno-startup-isolation/closure-linux.json'
LINUX_CLOSURE_SHA = '8b8f7bcf6a0ce17a27a168e9cb4478a08c05a7ab3d6a3d1d1e570991668e5fa0'
LINUX_DEPENDENCY_PINS = {
    LINUX_DEPENDENCY_CLOSURE: LINUX_CLOSURE_SHA,
    LINUX_DEPENDENCIES + '/closure.json': 'e861c49d7e0b44f1648d0843bab432bdd11feb67998b8515398b9b0896e8b448',
}
LINUX_RUNTIME_TARGET = {
    "python": {"implementation": "cpython", "version": "3.11.16", "platform": "linux",
               "machine": "x86_64", "pointer_bits": 64, "glibc": "glibc 2.39"},
    "node": {"version": "v22.23.2", "platform": "linux", "arch": "x64", "npm_version": "10.9.8"},
}
TARGETS = ("darwin-arm64-cpython311", "linux-x86_64-cpython311")
# Linux selects the same component artifacts with its own pinned wheel closure.
# Frozen previous distributions retain their original selections.
LINUX_SERVICES = SERVICES
LINUX_SERVICES_ACCEPTANCE = SERVICES_ACCEPTANCE
LINUX_SERVICES_SEED = SERVICES_SEED
LINUX_SERVICES_ARTIFACT = SERVICES_ARTIFACT
LINUX_SERVICES_WHEEL = SERVICES_WHEEL
LINUX_SERVICES_SHA = SERVICES_SHA
LINUX_CATALOG = CATALOG
LINUX_CATALOG_SHA = CATALOG_SHA
LINUX_INSTALLER = INSTALLER
LINUX_INSTALLER_SHA = INSTALLER_SHA
LINUX_PINS = dict(PINS)
LINUX_AGNO_ARTIFACT = AGNO_ARTIFACT
LINUX_AGNO_SHA = AGNO_SHA
LINUX_AGNO_ACCEPTANCE = AGNO_ACCEPTANCE
LINUX_AP_SEED = AP_SEED
LINUX_AP_ARTIFACT = AP_ARTIFACT
LINUX_AP_SHA = AP_SHA
LINUX_AP_ACCEPTANCE = AP_ACCEPTANCE

# Exact maintainer associations and accepted profile captures, not a runtime registry.
ROUTE_SOURCES = {'inventory': {'path': 'docs/adoption/oss-inventory.json', 'sha256': '8e96203830b987430b0a2be9bee6b998148c0bf938de30d8e8e217ca39d410f9'}, 'agno': {'path': 'docs/verification/20261011-agno-startup-isolation/agno-seed/profiles.stdout', 'sha256': '378d9d83915d24390c2dcd1f5f1f66e2675727e051436fdf9c5399e13350176e'}, 'activepieces': {'path': 'docs/verification/20261003-remaining-provider-guidance/installed/profiles.stdout', 'sha256': 'a96d92d7b53f67899e46e6ff6884aa99bd6beb4247ba8d62c64798bdc54f0840'}, 'services': {'path': 'docs/verification/20261008-native-connector-packages/canonical-adoption/services/profiles.stdout', 'sha256': 'aa18661ca9970afbb8dcae9bb7d1c6fe15d5dc6b85ae1f4a33ef8464adfc8bab'}}

MAX_FILE = 64 * 1024 * 1024
MAX_DEPENDENCY_BYTES = 256 * 1024 * 1024
DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW

# Pinned mode (macOS arm64): the bundle from a committed pin file, files this checkout carries and the three
# components built in the same run. None of the retained records named above is read in this mode.
CARRIED = "sdk/consumer-bundle"
MACOS_GUIDE = CARRIED + "/MACOS-CONSUMERS.md"
LINUX_ROUTES = CARRIED + "/linux-routes.json"
# Byte copies of members of the public Linux library; SOURCES.json beside them records where each is public.
CARRIED_PINS = {
    CARRIED + "/install-library-consumer.py": INSTALLER_SHA,
    CARRIED + "/agno/runtime-constraints.txt": "5a115ba33029a05e7357595fea97d984d7ac28c523102ad8a9c532a73dcb67d7",
    CARRIED + "/services/runtime-constraints.txt": "c83585d2633f11d0d4fb13c710d5a89ee6ea52ae3f5e6d6eff9757152a5755e5",
    CARRIED + "/activepieces/package.json": "051f2da4acb70bd08df7b380a27ff5a239dbcd7176d89f1083a361c60e59e69c",
    CARRIED + "/activepieces/package-lock.json": "dc577275169223fabc962e0b2337e73b6d21e81af05ec0c63b856325b5829272",
    LINUX_ROUTES: "41732384d5ada688b43d26c188df99781995a1ca0a2526565e66df4f1e8cd5cb",
}
# The discovery catalog of this repository. The retained paths keep their own pin above.
PINNED_DISCOVERY_CATALOG_SHA = "37e19a7600ba09858c16b4a0eecf9692c001038c8261c327d4020d758de09487"
PINS_SCHEMA = "connect-library-pins/1"
PINS_TARGET = "macos-arm64"
AP_COMPONENT = "@rkat/toolkit-activepieces"
MACOS_PYTHON_PLATFORM = {"implementation": "cpython", "version": "3.11", "system": "Darwin", "machine": "arm64"}
HEX_64 = re.compile(r"[0-9a-f]{64}")


class ExportError(ValueError):
    pass


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encoded(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode()


def relative_parts(value: str) -> tuple[str, ...]:
    path = PurePosixPath(value)
    if (not value or path.is_absolute() or str(path) != value or ".." in path.parts
            or not re.fullmatch(r"[A-Za-z0-9_./-]+", value)):
        raise ExportError("invalid relative input path")
    return path.parts


def open_directory(path: Path) -> int:
    """Walk an absolute path without following links after its explicit resolve."""
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


def read_file(root: Path, relative: str, expected: str | None = None) -> bytes:
    parts = relative_parts(relative)
    directory = open_directory(root)
    try:
        for part in parts[:-1]:
            child = os.open(part, DIR_FLAGS, dir_fd=directory)
            os.close(directory)
            directory = child
        handle = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(handle, "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_FILE:
                raise ExportError("input must be a bounded regular file")
            data = source.read(MAX_FILE + 1)
    finally:
        os.close(directory)
    if len(data) > MAX_FILE or (expected is not None and sha(data) != expected):
        raise ExportError("input differs from accepted bytes: " + relative)
    return data


def dependency_selection(target: str) -> tuple[str, str, dict[str, str], dict]:
    if target == TARGETS[0]:
        return DEPENDENCIES, DEPENDENCY_CLOSURE, DEPENDENCY_PINS, {
            "implementation": "cpython", "version": "3.11", "system": "Darwin",
            "machine": "arm64", "acquired_with": "3.11.15"}
    if target == TARGETS[1]:
        python = LINUX_RUNTIME_TARGET["python"]
        return LINUX_DEPENDENCIES, LINUX_DEPENDENCY_CLOSURE, LINUX_DEPENDENCY_PINS, {
            "implementation": python["implementation"], "version": ".".join(python["version"].split(".")[:2]),
            "system": "Linux", "machine": python["machine"], "acquired_with": python["version"]}
    raise ExportError("select an explicit supported consumer target")


def service_selection(target: str) -> dict:
    """Bind each existing build target to its accepted component, catalog and installer."""
    if target == TARGETS[0]:
        return {"evidence": SERVICES, "artifact": SERVICES_ARTIFACT, "acceptance": SERVICES_ACCEPTANCE, "seed": SERVICES_SEED, "wheel": SERVICES_WHEEL,
                "sha256": SERVICES_SHA, "version": "0.1.23", "pins": PINS,
                "catalog": CATALOG, "catalog_sha256": CATALOG_SHA,
                "installer": INSTALLER, "installer_sha256": INSTALLER_SHA,
                "ap_seed": AP_SEED, "ap_artifact": AP_ARTIFACT,
                "ap_sha256": AP_SHA, "ap_acceptance": AP_ACCEPTANCE,
                "agno_artifact": AGNO_ARTIFACT, "agno_sha256": AGNO_SHA,
                "agno_acceptance": AGNO_ACCEPTANCE}
    if target == TARGETS[1]:
        return {"evidence": LINUX_SERVICES, "artifact": LINUX_SERVICES_ARTIFACT, "acceptance": LINUX_SERVICES_ACCEPTANCE, "seed": LINUX_SERVICES_SEED, "wheel": LINUX_SERVICES_WHEEL,
                "sha256": LINUX_SERVICES_SHA, "version": "0.1.23", "pins": LINUX_PINS,
                "catalog": LINUX_CATALOG, "catalog_sha256": LINUX_CATALOG_SHA,
                "installer": LINUX_INSTALLER, "installer_sha256": LINUX_INSTALLER_SHA,
                "ap_seed": LINUX_AP_SEED, "ap_artifact": LINUX_AP_ARTIFACT,
                "ap_sha256": LINUX_AP_SHA, "ap_acceptance": LINUX_AP_ACCEPTANCE,
                "agno_artifact": LINUX_AGNO_ARTIFACT, "agno_sha256": LINUX_AGNO_SHA,
                "agno_acceptance": LINUX_AGNO_ACCEPTANCE}
    raise ExportError("select an explicit supported consumer target")


def collect_dependencies(root: Path, files: dict[str, bytes], origins: dict[str, dict],
                         target: str = TARGETS[0]) -> None:
    """Copy the pinned consumer closure; pip and npm still own dependency installation."""
    dependencies, closure_path, pins, platform = dependency_selection(target)
    records = {path: read_file(root, path, pin) for path, pin in pins.items()}
    closure = json.loads(records[closure_path])
    predecessor_path = dependencies + "/closure.json"
    predecessor = json.loads(records[predecessor_path])
    binding = closure.get("predecessor_closure", {})
    # New component bindings do not reacquire or replace dependency objects.
    # Require the complete retained artifact records and platform to stay exact.
    if (binding.get("path") != predecessor_path
            or binding.get("sha256") != pins[predecessor_path]
            or any(closure.get(key) != predecessor.get(key)
                   for key in ("artifacts", "python_platform", "node_scope"))):
        raise ExportError("dependency closure differs from retained predecessor")
    service = service_selection(target)
    bound = ("agno/runtime-constraints.txt", "services/runtime-constraints.txt",
             "activepieces/package.json", "activepieces/package-lock.json",
             "agno/" + AGNO_WHEEL, "services/" + service["wheel"], "activepieces/" + AP_ARCHIVE)
    if (closure["schema_version"] != 1
            or closure["scope"] != "dependency_bytes_for_existing_consumer_selection"
            or closure["accepted_inputs"] != {path: sha(files[path]) for path in bound}
            or closure["python_platform"] != platform):
        raise ExportError("dependency closure does not match selected consumer inputs")
    requirements: dict[str, dict[str, str]] = {}
    for family in ("agno", "services"):
        pairs = []
        for line in files[family + "/runtime-constraints.txt"].decode("ascii").splitlines():
            match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9.!+_-]*)", line)
            if match is None:
                raise ExportError("consumer constraints must be exact local package pins")
            pairs.append((re.sub(r"[-_.]+", "-", match[1]).lower(), match[2]))
        requirements[family] = dict(pairs)
        if len(pairs) != len(requirements[family]):
            raise ExportError("duplicate consumer dependency pin")
    lock = json.loads(files["activepieces/package-lock.json"])["packages"]
    npm_expected = {key.removeprefix("node_modules/"): value for key, value in lock.items()
                    if key and key != "node_modules/@rkat/toolkit-activepieces"}
    selected = {family: {} for family in ("agno", "services", "activepieces")}
    seen = set()
    total = 0
    if len(closure["artifacts"]) != 115:
        raise ExportError("unexpected dependency closure size")
    for record in closure["artifacts"]:
        ecosystem, name, version = record["ecosystem"], record["name"], record["version"]
        filename, digest, size = record["filename"], record["sha256"], record["bytes"]
        if (not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*\.(whl|tgz)", filename)
                or not re.fullmatch(r"[0-9a-f]{64}", digest)
                or type(size) is not int or not 0 < size <= MAX_FILE
                or record["path"] != "objects/" + digest + "/" + filename
                or (ecosystem, name) in seen):
            raise ExportError("invalid or duplicate dependency artifact")
        seen.add((ecosystem, name))
        total += size
        if total > MAX_DEPENDENCY_BYTES:
            raise ExportError("dependency closure exceeds byte limit")
        if ecosystem == "python":
            if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", name) or not filename.endswith(".whl"):
                raise ExportError("invalid Python dependency identity")
            families = [family for family in ("agno", "services") if requirements[family].get(name) == version]
            if not families or record["families"] != families:
                raise ExportError("Python dependency differs from selected constraints")
        elif ecosystem == "npm":
            expected = npm_expected.get(name)
            families = ["activepieces"]
            if (expected is None or not filename.endswith(".tgz") or record["families"] != families
                    or version != expected["version"] or record["integrity"] != expected["integrity"]
                    or record["source"] != {"kind": "locked_public_tarball", "url": expected["resolved"]}):
                raise ExportError("npm dependency differs from unchanged consumer lock")
        else:
            raise ExportError("unsupported dependency ecosystem")
        relative = dependencies + "/" + record["path"]
        data = read_file(root, relative, digest)
        if len(data) != size:
            raise ExportError("dependency artifact size differs")
        if ecosystem == "npm" and record["integrity"] != "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode():
            raise ExportError("npm dependency integrity differs")
        for family in families:
            output = family + "/" + filename
            if output in files and files[output] != data:
                raise ExportError("dependency would replace selected consumer bytes")
            files[output] = data
            origins[output] = {"path": relative, "sha256": digest}
            selected[family][name] = record
    for family, package in (("agno", "meerkat-toolkit-agno"), ("services", "meerkat-toolkit-services")):
        if set(selected[family]) != set(requirements[family]):
            raise ExportError("incomplete Python dependency closure")
        wheel = AGNO_WHEEL if family == "agno" else service["wheel"]
        lines = {name: name + "==" + row["version"] + " --hash=sha256:" + row["sha256"]
                 for name, row in selected[family].items()}
        version = "0.1.5" if family == "agno" else service["version"]
        lines[package] = package + "==" + version + " --hash=sha256:" + sha(files[family + "/" + wheel])
        output = family + "/install-offline.txt"
        files[output] = ("\n".join(line for _, line in sorted(lines.items())) + "\n").encode("ascii")
        origins[output] = {"kind": "hash_locked_requirements_from_selected_closure",
                           "closure_sha256": pins[closure_path]}
        if target == TARGETS[1]:
            output = family + "/install-requirements.txt"
            files[output] = ("\n".join("./" + filename for filename in sorted(
                path.split("/", 1)[1] for path in files if path.startswith(family + "/") and path.endswith(".whl"))) + "\n").encode("ascii")
            origins[output] = {"kind": "wheel_list_from_selected_linux_closure",
                               "closure_sha256": pins[closure_path]}
    if set(selected["activepieces"]) != set(npm_expected):
        raise ExportError("incomplete npm dependency closure")
    files["activepieces/offline-cache.json"] = encoded({"schema_version": 1, "tarballs": [
        {"name": name, "version": row["version"], "path": row["filename"],
         "sha256": row["sha256"], "integrity": row["integrity"]}
        for name, row in sorted(selected["activepieces"].items())]})
    origins["activepieces/offline-cache.json"] = {"kind": "local_tarballs_for_ordinary_npm_cache_add",
        "closure_sha256": pins[closure_path]}


def collect(root: Path, target: str = TARGETS[0]) -> tuple[dict[str, bytes], dict[str, dict]]:
    dependency_selection(target)
    service = service_selection(target)
    files: dict[str, bytes] = {}
    origins: dict[str, dict] = {}

    def copy(relative: str, name: str, expected: str) -> bytes:
        data = read_file(root, relative, expected)
        files[name] = data
        origins[name] = {"path": relative, "sha256": expected}
        return data

    anchors = {path: json.loads(read_file(root, path, pin)) for path, pin in service["pins"].items()}
    if (anchors[service["agno_acceptance"]]["wheel_sha256"] != service["agno_sha256"]
            or anchors[service["ap_acceptance"]]["archive_sha256"] != service["ap_sha256"]
            or anchors[service["acceptance"]]["wheel_sha256"] != service["sha256"]):
        raise ExportError("accepted artifact identity mismatch")
    retained = anchors[service["acceptance"]]["retained_acceptance"]
    accepted = json.loads(read_file(root, retained["path"], retained["sha256"]))
    if accepted.get("status") != "passed" or accepted.get("wheel_sha256") != service["sha256"]:
        raise ExportError("retained services acceptance differs from selected component")
    if target == TARGETS[0] and anchors[INSTALLER_ACCEPTANCE]["installer_sha256"] != INSTALLER_SHA:
        raise ExportError("accepted installer identity mismatch")
    manifest = anchors[AGNO_INPUTS + "/seed-manifest.json"]
    requirements = copy(AGNO_INPUTS + "/install-requirements.txt", "agno/install-requirements.txt",
                        manifest["install-requirements.txt"])
    names = requirements.decode("ascii").splitlines()
    if (len(names) != 52 or len(set(names)) != 52 or "./" + AGNO_WHEEL not in names
            or any(not re.fullmatch(r"\./[A-Za-z0-9_][A-Za-z0-9_.-]*\.whl", name) for name in names)):
        raise ExportError("unsupported accepted wheel requirement list")
    copy(AGNO_INPUTS + "/runtime-constraints.txt", "agno/runtime-constraints.txt",
         manifest["runtime-constraints.txt"])
    for name in names:
        filename = name[2:]
        if filename == AGNO_WHEEL:
            copy(service["agno_artifact"], "agno/" + filename, service["agno_sha256"])
        elif target == TARGETS[0]:
            copy(AGNO_SEED + "/" + filename, "agno/" + filename, manifest[filename])
    if sha(files["agno/" + AGNO_WHEEL]) != service["agno_sha256"]:
        raise ExportError("selected Agno wheel mismatch")

    inputs = anchors[service["ap_acceptance"]]["consumer_inputs"]
    for name in ("package.json", "package-lock.json"):
        copy(service["ap_seed"] + "/" + name, "activepieces/" + name, inputs[name]["sha256"])
    copy(service["ap_artifact"], "activepieces/" + AP_ARCHIVE, service["ap_sha256"])
    if sha(files["activepieces/" + AP_ARCHIVE]) != service["ap_sha256"]:
        raise ExportError("selected Activepieces archive mismatch")

    service_inputs = anchors[service["acceptance"]]["bundle_sha256"]
    for name in (service["wheel"], "runtime-constraints.txt", "README.md"):
        output = "PROFILE-GUIDE.md" if name == "README.md" else name
        source = service["evidence"] if name == "runtime-constraints.txt" else service["seed"]
        copy(service["artifact"] if name == service["wheel"] else source + "/" + name,
             "services/" + output, service_inputs[name])
    if "install-requirements.txt" in service_inputs:
        copy(service["seed"] + "/install-requirements.txt", "services/install-requirements.txt",
             service_inputs["install-requirements.txt"])
    else:
        files["services/install-requirements.txt"] = ("./" + service["wheel"] + "\n").encode("ascii")
        origins["services/install-requirements.txt"] = {"kind": "selected_component_wheel_requirement",
            "acceptance": service["acceptance"]}
    if sha(files["services/" + service["wheel"]]) != service["sha256"]:
        raise ExportError("selected services wheel mismatch")
    collect_dependencies(root, files, origins, target)
    if "install-offline.txt" in service_inputs:
        accepted_requirements = read_file(root, service["seed"] + "/install-offline.txt",
                                          service_inputs["install-offline.txt"])
        if target == TARGETS[0]:
            # Preserve the accepted macOS order and hashes exactly.
            if sorted(files["services/install-offline.txt"].splitlines()) != sorted(accepted_requirements.splitlines()):
                raise ExportError("generated services requirements differ from accepted offline selection")
            files["services/install-offline.txt"] = accepted_requirements
            origins["services/install-offline.txt"] = {"path": service["seed"] + "/install-offline.txt",
                "sha256": service_inputs["install-offline.txt"]}
        else:
            # The component's supported dependency versions are platform-neutral.
            # Linux wheel hashes come from its independently pinned closure.
            graphs = []
            for data in (accepted_requirements, files["services/install-offline.txt"]):
                graph = {}
                for line in data.decode("ascii").splitlines():
                    match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9.!+_-]*) --hash=sha256:([0-9a-f]{64})", line)
                    if match is None:
                        raise ExportError("invalid accepted service requirement")
                    name = re.sub(r"[-_.]+", "-", match[1]).lower()
                    if name in graph:
                        raise ExportError("duplicate accepted service requirement")
                    graph[name] = match[2]
                graphs.append(graph)
            if graphs[0] != graphs[1]:
                raise ExportError("Linux service versions differ from accepted component graph")

    # Export the same generated metadata consumed by PackageCatalog. No second
    # inference engine selects a service or strategy for the consumer.
    copy(service["catalog"], "catalog.json", service["catalog_sha256"])

    # Read only named documentation members from already hash-verified archives.
    # No archive paths become filesystem paths and no package code is imported.
    def document(family: str, artifact: str, member: str, output: str, data: bytes) -> None:
        files[family + "/" + output] = data
        origins[family + "/" + output] = {
            "artifact": artifact, "artifact_sha256": sha(files[artifact]), "member": member,
        }

    artifact = "agno/" + AGNO_WHEEL
    with zipfile.ZipFile(io.BytesIO(files[artifact])) as archive:
        members = [("meerkat_toolkit_agno/PROFILE-GUIDE.md", "PROFILE-GUIDE.md")]
        members += [("meerkat_toolkit_agno-0.1.5.dist-info/licenses/" + name, name)
                    for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md")]
        for member, output in members:
            matches = [info for info in archive.infolist() if info.filename == member]
            if len(matches) != 1 or matches[0].is_dir() or matches[0].file_size > MAX_FILE:
                raise ExportError("invalid accepted wheel document")
            document("agno", artifact, member, output, archive.read(matches[0]))
    artifact = "activepieces/" + AP_ARCHIVE
    with tarfile.open(fileobj=io.BytesIO(files[artifact]), mode="r:gz") as archive:
        for source, output in (("README.md", "PROFILE-GUIDE.md"), ("LICENSE-MIT", "LICENSE-MIT"),
                               ("LICENSE-APACHE", "LICENSE-APACHE"), ("THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md")):
            member = "package/" + source
            matches = [info for info in archive.getmembers() if info.name == member]
            if len(matches) != 1 or not matches[0].isreg() or matches[0].size > MAX_FILE:
                raise ExportError("invalid accepted npm document")
            with archive.extractfile(matches[0]) as stream:
                document("activepieces", artifact, member, output, stream.read(MAX_FILE + 1))
    # Author-owned usage guidance is distinct from the immutable accepted inputs.
    guide = "LOCAL-CONSUMERS.md" if target == TARGETS[0] else "LINUX-CONSUMERS.md"
    source = "docs/adoption/" + guide
    files["README.md"] = read_file(ROOT, source)
    origins["README.md"] = {"path": source, "base": "exporter_checkout",
                            "sha256": sha(files["README.md"]), "kind": "consumer_instructions"}
    copy(service["installer"], "install-library-consumer.py", service["installer_sha256"])
    files["agno/hackernews.json"] = encoded({"profile": "hackernews", "operations": ["get_top_hackernews_stories"], "config": {"timeout": 10}})
    files["activepieces/tally.json"] = encoded({"profile": "tally.get-form", "token_env": "MY_TALLY_TOKEN"})
    slack_operations = ["auth_test", "list_conversations_page", "get_conversation", "read_interval", "get_permalink"]
    files["services/slack.json"] = encoded({"profile": "slack", "operations": slack_operations, "config": {
        "token_env": "HOST_SELECTED_SLACK_TOKEN", "timeout_seconds": 20,
        "max_response_bytes": 2097152, "max_interval_requests": 30, "max_interval_messages": 1000}})
    files["services/gmail.json"] = encoded({"profile": "gmail", "operations": ["list_messages_page", "get_message", "send_message"], "config": {
        "token_env": "HOST_SELECTED_GMAIL_TOKEN", "user_id": "me", "timeout_seconds": 20,
        "max_response_bytes": 2097152, "max_message_bytes": 1048576}})
    for name in ("agno/hackernews.json", "activepieces/tally.json", "services/slack.json", "services/gmail.json"):
        origins[name] = {"kind": "nonsecret_configuration_example"}
    # Route construction keeps its strict adopted-inventory meaning. Discovery
    # records are appended afterward and never imply an installable profile.
    routes = build_routes(root, files, target)
    adopted = files["catalog.json"]
    discovery = read_file(root, DISCOVERY_CATALOG, DISCOVERY_CATALOG_SHA)
    files["catalog.json"], files["routes.json"] = compose_catalog(
        adopted, discovery, routes)
    origins["catalog.json"] = {"kind": "composed_catalog_metadata", "sources": [
        {"path": service["catalog"], "sha256": service["catalog_sha256"],
         "scope": "adopted", "entries": len(json.loads(adopted))},
        {"path": DISCOVERY_CATALOG, "sha256": DISCOVERY_CATALOG_SHA,
         "scope": "discovery", "entries": len(json.loads(discovery))},
    ]}
    origins["routes.json"] = {"kind": "explicit_maintainer_catalog_routes", "sources": ROUTE_SOURCES}
    return files, origins


def compose_catalog(adopted, discovery, routes):
    """Compose pinned metadata, retaining exact records and existing route meaning."""
    adopted_rows, discovery_rows = json.loads(adopted), json.loads(discovery)
    if not isinstance(adopted_rows, list) or not isinstance(discovery_rows, list):
        raise ExportError("invalid catalog composition")
    rows = adopted_rows + discovery_rows
    entries = {}
    for row in rows:
        identity = row.get("identity") if isinstance(row, dict) else None
        if (not isinstance(identity, dict) or set(identity) != {"publisher", "name", "version"}
                or any(not isinstance(value, str) or not value for value in identity.values())):
            raise ExportError("invalid catalog composition identity")
        key = tuple(identity[field] for field in ("publisher", "name", "version"))
        if key in entries:
            raise ExportError("duplicate catalog composition identity")
        entries[key] = row
    index = json.loads(routes)
    if index["catalog_sha256"] != sha(adopted):
        raise ExportError("catalog route binding mismatch")
    adopted_entries = {tuple(row["identity"][field] for field in ("publisher", "name", "version")): row
                       for row in adopted_rows}
    for route in index["routes"]:
        key = tuple(route["identity"][field] for field in ("publisher", "name", "version"))
        if (key not in adopted_entries or entries.get(key) != adopted_entries[key]
                or sum(s.get("id") == route["strategy_id"] for s in entries[key]["strategies"]) != 1):
            raise ExportError("composed catalog changes route meaning")
    combined = encoded(rows)
    index["catalog_sha256"] = sha(combined)
    return combined, encoded(index)


def build_routes(root, files, target):
    """Compile explicit maintainer associations against this exact exported payload."""
    sources = {}
    for key, source in ROUTE_SOURCES.items():
        raw = read_file(root, source["path"])
        if sha(raw) != source["sha256"]:
            raise ExportError("route source changed")
        sources[key] = json.loads(raw)
    catalog = json.loads(files["catalog.json"])
    entries = {}
    for entry in catalog:
        identity = entry["identity"]
        if identity["publisher"] != "meerkat-toolkit" or identity["name"] in entries:
            raise ExportError("ambiguous catalog route identity")
        entries[identity["name"]] = entry
    paths = {"agno": "agno/" + AGNO_WHEEL, "activepieces": "activepieces/" + AP_ARCHIVE,
             "services": "services/" + service_selection(target)["wheel"]}
    components = {}
    for family, path in paths.items():
        profiles = [row["profile"] for row in sources[family]["profiles"]]
        if (not profiles or len(profiles) != len(set(profiles))
                or any(not isinstance(profile, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", profile) for profile in profiles)):
            raise ExportError("invalid route profile capture")
        components[family] = {"path": path, "sha256": sha(files[path]), "profiles": sorted(profiles)}
    routes, seen = [], set()
    def add(row, implementation, strategy, family):
        entry = entries.get(row["id"])
        if entry is None or sum(s["id"] == strategy for s in entry["strategies"]) != 1:
            raise ExportError("route strategy missing from catalog")
        key = (row["id"], strategy)
        if key in seen:
            raise ExportError("duplicate catalog route")
        seen.add(key)
        if (implementation.get("artifact_sha256") != components[family]["sha256"]
                or implementation.get("profile") not in components[family]["profiles"]):
            raise ExportError("route artifact or profile mismatch")
        routes.append({"identity": entry["identity"], "strategy_id": strategy,
                       "family": family, "profile": implementation["profile"]})
    family_by_route = {"agno-toolkit-adapter": "agno", "activepieces-action-adapter": "activepieces"}
    inventory_ids = set()
    for row in sources["inventory"]["integrations"]:
        if row["id"] in inventory_ids:
            raise ExportError("duplicate inventory identity")
        inventory_ids.add(row["id"])
        if row["status"] != "adopted":
            if row["id"] in entries:
                raise ExportError("catalog route lacks adopted source")
            continue
        family = family_by_route.get(row["adoption_route"])
        if family is not None:
            if row["status"] != "adopted":
                raise ExportError("route is not adopted")
            add(row, row["implementation"], "selected-local-mcp", family)
        elif row["adoption_route"] != "maintained-mcp":
            raise ExportError("unknown adoption route")
        for alternative in row.get("alternative_implementations", []):
            if alternative.get("receipt_kind") not in ("configured-services-consumer-v1", "configured-services-consumer-v2", "configured-services-consumer-v3"):
                raise ExportError("unknown alternative route")
            add(row, alternative, alternative["strategy_id"], "services")
    if set(entries) != {row["id"] for row in sources["inventory"]["integrations"] if row["status"] == "adopted"}:
        raise ExportError("catalog identities do not match adopted sources")
    return encoded({"schema_version": 1, "catalog_sha256": sha(files["catalog.json"]),
        "runtime_target": LINUX_RUNTIME_TARGET if target == TARGETS[1] else None,
        "provenance": {"inventory": ROUTE_SOURCES["inventory"],
                       "profile_captures": {key: ROUTE_SOURCES[key] for key in paths}},
        "components": components, "routes": routes})


def output_location(value: str, root: Path) -> tuple[Path, str]:
    path = Path(value)
    if not path.is_absolute() or any(part in (".", "..") for part in value.split("/")) or path == path.parent:
        raise ExportError("output must be an absolute new directory without dot components")
    parent = path.parent.resolve(strict=True)
    target = parent / path.name
    for protected in (root / Path(AGNO_ACCEPTANCE).parent, root / Path(AP_ACCEPTANCE).parent, root / AGNO, root / AP, root / CONFIG, root / AUTH, root / CREDENTIAL, root / SERVICES,
                      root / SERVICES_SEED, root / DEPENDENCIES, root / LINUX_DEPENDENCIES,
                      root / Path(DEPENDENCY_CLOSURE).parent, root / Path(LINUX_DEPENDENCY_CLOSURE).parent,
                      root / LINUX_SERVICES, root / LINUX_SERVICES_SEED, root / Path(LINUX_CATALOG).parent,
                      root / ACQUISITION, root / "docs/verification/20261003-linux-consumers"):
        if target == protected or protected in target.parents:
            raise ExportError("output cannot be inside accepted proof inputs")
    if os.path.lexists(target):
        raise ExportError("output already exists")
    return parent, path.name


def write_file(directory: int, name: str, data: bytes) -> None:
    handle = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=directory)
    with os.fdopen(handle, "wb") as output:
        output.write(data)


def export(root: Path, output: str, target: str = TARGETS[0]) -> dict:
    root = root.resolve(strict=True)
    parent, name = output_location(output, root)
    files, origins = collect(root, target)
    _, _, dependency_pins, _ = dependency_selection(target)
    contents = {"schema_version": 1, "scope": "consumer_export_only_no_installation_or_activation",
                "accepted_inputs": {**service_selection(target)["pins"], **dependency_pins}, "files": {
                    path: {"sha256": sha(data), "source": origins[path]} for path, data in sorted(files.items())}}
    if target == TARGETS[1]:
        contents.update(schema_version=2, runtime_target=LINUX_RUNTIME_TARGET)
    files["CONTENTS.json"] = encoded(contents)
    checksums = "".join(sha(data) + "  " + path + "\n" for path, data in sorted(files.items())).encode()
    parent_fd = open_directory(parent)
    try:
        os.mkdir(name, 0o700, dir_fd=parent_fd)  # Atomic refusal if the destination appeared meanwhile.
        directory = os.open(name, DIR_FLAGS, dir_fd=parent_fd)
        try:
            for family in ("agno", "activepieces", "services"):
                os.mkdir(family, 0o700, dir_fd=directory)
                family_fd = os.open(family, DIR_FLAGS, dir_fd=directory)
                try:
                    for path, data in sorted(files.items()):
                        if path.startswith(family + "/"):
                            write_file(family_fd, path.split("/", 1)[1], data)
                finally:
                    os.close(family_fd)
            for path in ("README.md", "install-library-consumer.py", "catalog.json", "routes.json", "CONTENTS.json"):
                write_file(directory, path, files[path])
            write_file(directory, "SHA256SUMS", checksums)  # Completion marker, written last.
        finally:
            os.close(directory)
    finally:
        os.close(parent_fd)
    return {"output": str(parent / name), "files": len(files) + 1,
            "contents_sha256": sha(files["CONTENTS.json"]), "checksums_sha256": sha(checksums),
            "scope": contents["scope"]}


class PinnedInputs(NamedTuple):
    """What the pinned mode reads besides the checkout: explicit paths, never inferred from the host."""
    pins: Path
    pinned_inputs: Path
    components: dict  # family -> (absolute path of the built component, the sha256 its build recorded)
    closure_record: Path
    closure_record_key: str | None = None
    profile_captures: dict = {}


def canonical(value: object) -> bytes:
    """The encoding the catalog generator digests for an entry's metadata revision."""
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()


def retained_component_digests() -> dict[str, str]:
    """The component digests the committed catalog and the Linux routes name."""
    return {"agno": AGNO_SHA, "activepieces": AP_SHA, "services": SERVICES_SHA}


def component_paths() -> dict[str, str]:
    return {"agno": "agno/" + AGNO_WHEEL, "activepieces": "activepieces/" + AP_ARCHIVE,
            "services": "services/" + SERVICES_WHEEL}


def normalized(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def parse_constraints(data: bytes) -> dict[str, str]:
    """Exact local pins only: one `name==version` per line, each normalized name once."""
    pairs = []
    for line in data.decode("ascii").splitlines():
        match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9.!+_-]*)", line)
        if match is None:
            raise ExportError("consumer constraints must be exact local package pins")
        pairs.append((normalized(match[1]), match[2]))
    result = dict(pairs)
    if not pairs or len(pairs) != len(result):
        raise ExportError("duplicate consumer dependency pin")
    return result


def wheel_supported_on_macos_arm64(filename: str) -> bool:
    """Whether CPython 3.11 on macOS arm64 (macOS 11 or later) installs a wheel of this file name."""
    if not filename.endswith(".whl"):
        return False
    parts = filename[:-4].split("-")
    if len(parts) not in (5, 6) or any(not part for part in parts):
        return False
    pythons, abis, platforms = (part.split(".") for part in parts[-3:])
    if "none" in abis:
        python = "py3" in pythons or "cp311" in pythons
    elif "cp311" in abis:
        python = "cp311" in pythons
    elif "abi3" in abis:
        python = any(match is not None and int(match[1]) <= 11
                     for match in (re.fullmatch(r"cp3([0-9]+)", tag) for tag in pythons))
    else:
        return False
    if not python:
        return False
    if "none" in abis and "any" in platforms:
        return True
    for tag in platforms:
        match = re.fullmatch(r"macosx_([0-9]+)_([0-9]+)_(arm64|universal2)", tag)
        if match is not None and int(match[1]) <= 11:
            return True
    return False


def wheel_metadata(data: bytes) -> tuple[str, str, list[str]]:
    """Name, version and requirements of a wheel, read from its one METADATA member."""
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        members = [info for info in archive.infolist() if info.filename.endswith(".dist-info/METADATA")]
        if len(members) != 1 or members[0].is_dir() or members[0].file_size > 1024 * 1024:
            raise ExportError("invalid wheel metadata")
        info = email.parser.BytesParser().parsebytes(archive.read(members[0]))
    name, version = info["Name"], info["Version"]
    if not isinstance(name, str) or not isinstance(version, str):
        raise ExportError("invalid wheel metadata")
    return normalized(name), version, [str(value) for value in info.get_all("Requires-Dist", [])]


def load_pins(path: Path) -> tuple[bytes, dict]:
    """Read the platform's pin file and index its rows by kind, name and version."""
    if not path.is_absolute():
        raise ExportError("the pin file needs an absolute path")
    raw = read_file(path.parent.resolve(strict=True), path.name)
    document = json.loads(raw)
    if (not isinstance(document, dict) or document.get("schema") != PINS_SCHEMA
            or document.get("target") != PINS_TARGET or not isinstance(document.get("pins"), list)):
        raise ExportError("unsupported pin file")
    index: dict[tuple[str, str, str], list[dict]] = {}
    for row in document["pins"]:
        if (not isinstance(row, dict)
                or any(not isinstance(row.get(key), str) or not row[key] for key in ("kind", "name", "version", "file"))):
            raise ExportError("invalid pin row")
        name = normalized(row["name"]) if row["kind"] == "pypi-wheel" else row["name"]
        index.setdefault((row["kind"], name, row["version"]), []).append(row)
    return raw, index


def pinned_artifact(directory: Path, index: dict, kind: str, name: str, version: str) -> tuple[dict, bytes]:
    """The one fetched file of a selected pin row, checked against the digests the row carries."""
    rows = index.get((kind, name, version), [])
    if len(rows) != 1:
        raise ExportError("selected dependency needs exactly one pin row")
    row = rows[0]
    if (not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*\.(whl|tgz)", row["file"])
            or not isinstance(row.get("url"), str) or not row["url"].startswith("https://")):
        raise ExportError("invalid pin row")
    data = read_file(directory, kind + "/" + row["file"])
    checked = False
    for key, pattern, digest in (("sha256", r"[0-9a-f]{64}", hashlib.sha256), ("sha512", r"[0-9a-f]{128}", hashlib.sha512)):
        expected = row.get(key)
        if expected is None:
            continue
        if not isinstance(expected, str) or not re.fullmatch(pattern, expected) or digest(data).hexdigest() != expected:
            raise ExportError("fetched input differs from its pin")
        checked = True
    if not checked:
        raise ExportError("pin row carries no digest")
    return row, data


def read_component(path: Path, digest: str, filename: str) -> bytes:
    """One component built in this run: a fixed file name and the digest its build recorded."""
    if (not isinstance(path, Path) or not path.is_absolute() or path.name != filename
            or not isinstance(digest, str) or not HEX_64.fullmatch(digest)):
        raise ExportError("component needs its fixed file name, an absolute path and a sha256")
    return read_file(path.parent.resolve(strict=True), path.name, digest)


def derive_catalog(adopted: bytes, digests: dict[str, str]) -> tuple[bytes, dict[str, tuple[str, str]]]:
    """The committed adopted catalog with this run's component digests.

    An entry's version is the digest of its own metadata, so every entry whose instructions name a replaced
    digest gets its version recomputed the way the catalog's generator computes it. Returns the catalog bytes
    and, per changed entry name, its committed and its new version. With the retained digests the result is
    the committed file.
    """
    rows = json.loads(adopted)
    if not isinstance(rows, list) or (json.dumps(rows, ensure_ascii=True, indent=2) + "\n").encode() != adopted:
        raise ExportError("adopted catalog is not in its generator's encoding")
    retained = retained_component_digests()
    if set(digests) != set(retained) or any(not isinstance(v, str) or not HEX_64.fullmatch(v) for v in digests.values()):
        raise ExportError("one sha256 per component family is required")
    replaced = {retained[family]: digests[family] for family in retained if retained[family] != digests[family]}

    def revision(row: dict) -> str:
        body = {**row, "identity": {key: value for key, value in row["identity"].items() if key != "version"}}
        return "metadata-sha256:" + sha(canonical(body))

    versions: dict[str, tuple[str, str]] = {}
    for row in rows:
        identity = row.get("identity") if isinstance(row, dict) else None
        if (not isinstance(identity, dict) or set(identity) != {"publisher", "name", "version"}
                or not isinstance(row.get("strategies"), list) or identity["version"] != revision(row)):
            raise ExportError("adopted catalog entry is not its generator's output")
        changed = False
        for strategy in row["strategies"]:
            text = strategy["instructions"]
            for old, new in replaced.items():
                if old in text:
                    text, changed = text.replace(old, new), True
            strategy["instructions"] = text
        if any(old in canonical(row).decode("ascii") for old in replaced):
            raise ExportError("component digest outside the strategy instructions")
        if changed:
            if identity["name"] in versions:
                raise ExportError("ambiguous catalog identity")
            committed = identity["version"]
            identity["version"] = revision(row)
            versions[identity["name"]] = (committed, identity["version"])
    return (json.dumps(rows, ensure_ascii=True, indent=2) + "\n").encode(), versions


def derive_routes(linux: bytes, catalog: bytes, digests: dict[str, str],
                  versions: dict[str, tuple[str, str]]) -> bytes:
    """The macOS routes from the public Linux routes file.

    Changed: the target field, each component digest, and the two kinds of value that are digests over text
    naming a component digest (the entry versions and the catalog digest). The maintainer associations, the
    profile lists and the provenance stay exactly the Linux file's.
    """
    index = json.loads(linux)
    retained, paths = retained_component_digests(), component_paths()
    if (not isinstance(index, dict) or encoded(index) != linux
            or set(index) != {"schema_version", "catalog_sha256", "runtime_target", "provenance", "components", "routes"}
            or type(index["schema_version"]) is not int or index["schema_version"] != 1
            or index["runtime_target"] != LINUX_RUNTIME_TARGET
            or not isinstance(index["components"], dict) or set(index["components"]) != set(retained)
            or not isinstance(index["routes"], list) or len(index["routes"]) > 512):
        raise ExportError("unsupported Linux routes file")
    for family, component in index["components"].items():
        profiles = component.get("profiles") if isinstance(component, dict) else None
        if (not isinstance(component, dict) or set(component) != {"path", "sha256", "profiles"}
                or component["path"] != paths[family] or component["sha256"] != retained[family]
                or not isinstance(profiles, list) or not profiles or profiles != sorted(set(profiles))
                or any(not isinstance(p, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", p) for p in profiles)):
            raise ExportError("Linux routes name another component selection")
        component["sha256"] = digests[family]
    entries = {}
    for row in json.loads(catalog):
        entries[tuple(row["identity"][field] for field in ("publisher", "name", "version"))] = row
    seen = set()
    for route in index["routes"]:
        identity = route.get("identity") if isinstance(route, dict) else None
        if (not isinstance(route, dict) or set(route) != {"identity", "strategy_id", "family", "profile"}
                or not isinstance(identity, dict) or set(identity) != {"publisher", "name", "version"}
                or any(not isinstance(value, str) or not value for value in identity.values())):
            raise ExportError("invalid Linux route")
        if identity["name"] in versions:
            committed, current = versions[identity["name"]]
            if identity["version"] != committed:
                raise ExportError("Linux route names another catalog revision")
            identity["version"] = current
        key = tuple(identity[field] for field in ("publisher", "name", "version"))
        entry = entries.get(key)
        if (entry is None or (key, route["strategy_id"]) in seen
                or sum(strategy.get("id") == route["strategy_id"] for strategy in entry["strategies"]) != 1
                or route["family"] not in index["components"]
                or route["profile"] not in index["components"][route["family"]]["profiles"]):
            raise ExportError("Linux route does not bind the committed catalog")
        seen.add((key, route["strategy_id"]))
    index["runtime_target"] = None
    index["catalog_sha256"] = sha(catalog)
    return encoded(index)


def pinned_locations(root: Path, output: str, inputs: PinnedInputs) -> tuple[Path, str, Path, str]:
    """Two new destinations, neither inside an input: the bundle directory and the closure record."""
    path, record = Path(output), inputs.closure_record
    if (not path.is_absolute() or any(part in (".", "..") for part in output.split("/")) or path == path.parent
            or not isinstance(record, Path) or not record.is_absolute()
            or any(part in (".", "..") for part in str(record).split("/")) or record == record.parent):
        raise ExportError("output and closure record must be absolute new paths without dot components")
    parent = path.parent.resolve(strict=True)
    target = parent / path.name
    record_parent = record.parent.resolve(strict=True)
    record_target = record_parent / record.name
    protected = (root / CARRIED, root / Path(CATALOG).parent, inputs.pinned_inputs.resolve(strict=True))
    for candidate in (target, record_target):
        if any(candidate == base or base in candidate.parents for base in protected):
            raise ExportError("output cannot be inside its inputs")
        if os.path.lexists(candidate):
            raise ExportError("output already exists")
    if record_target == target or target in record_target.parents:
        raise ExportError("the closure record is not a member of the bundle")
    return parent, path.name, record_parent, record.name


def collect_pinned(root: Path, inputs: PinnedInputs) -> tuple[dict[str, bytes], dict[str, dict], dict[str, str], bytes, dict]:
    """Assemble the macOS arm64 bundle in memory. Reads only; no input is imported, run or downloaded."""
    if PINNED_DISCOVERY_CATALOG_SHA is None:
        raise ExportError("no discovery catalog is selected for the pinned mode")
    files: dict[str, bytes] = {}
    origins: dict[str, dict] = {}
    paths, retained = component_paths(), retained_component_digests()
    if not isinstance(inputs.components, dict) or set(inputs.components) != set(paths):
        raise ExportError("exactly the three components are required")
    carried = {relative: read_file(root, relative, pin) for relative, pin in CARRIED_PINS.items()}

    def carry(name: str) -> None:
        relative = CARRIED + "/" + name
        files[name] = carried[relative]
        origins[name] = {"path": relative, "sha256": CARRIED_PINS[relative]}

    for name in ("install-library-consumer.py", "agno/runtime-constraints.txt",
                 "services/runtime-constraints.txt", "activepieces/package.json"):
        carry(name)
    digests = {}
    for family, bundle_path in paths.items():
        path, digest = inputs.components[family]
        files[bundle_path] = read_component(path, digest, bundle_path.split("/", 1)[1])
        digests[family] = digest
        origins[bundle_path] = {"kind": "component_built_in_this_run", "sha256": digest}

    # Catalog and routes: the committed adopted catalog and the public Linux routes with this run's digests.
    adopted = read_file(root, CATALOG, CATALOG_SHA)
    discovery = read_file(root, DISCOVERY_CATALOG, PINNED_DISCOVERY_CATALOG_SHA)
    derived, versions = derive_catalog(adopted, digests)
    routes = derive_routes(carried[LINUX_ROUTES], derived, digests, versions)
    route_profiles = {family: component["profiles"] for family, component in json.loads(routes)["components"].items()}
    files["catalog.json"], files["routes.json"] = compose_catalog(derived, discovery, routes)
    if any(digest in files["catalog.json"].decode("ascii") or digest in files["routes.json"].decode("ascii")
           for family, digest in retained.items() if digest != digests[family]):
        raise ExportError("a replaced component digest remains in the catalog or routes")
    origins["catalog.json"] = {"kind": "composed_catalog_metadata", "sources": [
        {"path": CATALOG, "sha256": CATALOG_SHA, "scope": "adopted", "entries": len(json.loads(adopted)),
         "derivation": {"kind": "component_digests_of_this_run", "entries_changed": len(versions)}},
        {"path": DISCOVERY_CATALOG, "sha256": PINNED_DISCOVERY_CATALOG_SHA,
         "scope": "discovery", "entries": len(json.loads(discovery))}]}
    origins["routes.json"] = {"kind": "derived_from_public_linux_routes", "path": LINUX_ROUTES,
                              "sha256": CARRIED_PINS[LINUX_ROUTES],
                              "substituted": ["runtime_target", "components.sha256",
                                              "routes.identity.version", "catalog_sha256"]}

    # Each Python component may require only what the pinned constraints of its family hold.
    requirements = {family: parse_constraints(files[family + "/runtime-constraints.txt"])
                    for family in ("agno", "services")}
    for family, distribution, version in (("agno", "meerkat-toolkit-agno", "0.1.5"),
                                          ("services", "meerkat-toolkit-services", "0.1.23")):
        name, found, requires = wheel_metadata(files[paths[family]])
        if (name, found) != (distribution, version):
            raise ExportError("component wheel is not the selected distribution")
        for requirement in requires:
            if re.search(r";\s*extra\s*==", requirement):
                continue  # Optional extras are outside the base bundle.
            match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9.!+_-]*)", requirement)
            if match is None or requirements[family].get(normalized(match[1])) != match[2]:
                raise ExportError("component requires a dependency outside its pinned constraints")

    # The consumer lock names the component archive by its sha512; npm checks it at install.
    lock_relative = CARRIED + "/activepieces/package-lock.json"
    lock = json.loads(carried[lock_relative])
    if encoded(lock) != carried[lock_relative]:
        raise ExportError("carried consumer lock is not in the exporter's encoding")
    packages = lock["packages"]
    component_entry = packages["node_modules/" + AP_COMPONENT]
    if component_entry.get("resolved") != "file:" + AP_ARCHIVE or component_entry.get("version") != "0.1.0":
        raise ExportError("carried consumer lock selects another component")
    with tarfile.open(fileobj=io.BytesIO(files[paths["activepieces"]]), mode="r:gz") as archive:
        members = [info for info in archive.getmembers() if info.name == "package/package.json"]
        if len(members) != 1 or not members[0].isreg() or members[0].size > 1024 * 1024:
            raise ExportError("invalid component archive")
        with archive.extractfile(members[0]) as stream:
            package = json.loads(stream.read(1024 * 1024 + 1))
    if (not isinstance(package, dict) or package.get("name") != AP_COMPONENT or package.get("version") != "0.1.0"
            or package.get("dependencies") != component_entry.get("dependencies")
            or package.get("engines") != component_entry.get("engines")):
        raise ExportError("component archive differs from the carried consumer lock")
    component_entry["integrity"] = "sha512-" + base64.b64encode(
        hashlib.sha512(files[paths["activepieces"]]).digest()).decode()
    files["activepieces/package-lock.json"] = encoded(lock)
    origins["activepieces/package-lock.json"] = {
        "kind": "consumer_lock_with_this_run_component_integrity", "path": lock_relative,
        "carried_sha256": CARRIED_PINS[lock_relative], "component_sha256": digests["activepieces"]}

    # The dependency closure: every file is the pinned public index artifact, fetched before this step.
    pins_raw, index = load_pins(inputs.pins)
    directory = inputs.pinned_inputs.resolve(strict=True)
    artifacts, selected, total = [], {family: {} for family in ("agno", "services", "activepieces")}, 0
    wanted: dict[tuple[str, str], list[str]] = {}
    for family in ("agno", "services"):
        for name, version in requirements[family].items():
            wanted.setdefault((name, version), []).append(family)
    for (name, version), families in sorted(wanted.items()):
        row, data = pinned_artifact(directory, index, "pypi-wheel", name, version)
        filename = row["file"]
        if not wheel_supported_on_macos_arm64(filename) or wheel_metadata(data)[:2] != (name, version):
            raise ExportError("pinned wheel is not this dependency for CPython 3.11 on macOS arm64")
        total += len(data)
        record = {"ecosystem": "python", "name": name, "version": version, "filename": filename,
                  "sha256": sha(data), "bytes": len(data), "families": families,
                  "path": "pypi-wheel/" + filename,
                  "source": {"kind": "pinned_public_index_artifact", "url": row["url"]}}
        artifacts.append(record)
        for family in families:
            output = family + "/" + filename
            if output in files:
                raise ExportError("dependency would replace selected consumer bytes")
            files[output] = data
            origins[output] = {"kind": "pinned_public_index_artifact", "pin": "pypi-wheel/" + filename,
                               "url": row["url"], "sha256": sha(data)}
            selected[family][name] = record
    npm_expected = {key.removeprefix("node_modules/"): value for key, value in packages.items()
                    if key and key != "node_modules/" + AP_COMPONENT}
    if set(npm_expected) != set(component_entry.get("dependencies") or {}) | {
            name for row in npm_expected.values() for name in (row.get("dependencies") or {})}:
        raise ExportError("carried consumer lock holds an unreferenced package")
    for name, expected in sorted(npm_expected.items()):
        if (not re.fullmatch(r"(?:@[a-z0-9_.-]+/)?[a-z0-9_.-]+", name) or not isinstance(expected, dict)
                or not isinstance(expected.get("version"), str) or not isinstance(expected.get("integrity"), str)
                or not isinstance(expected.get("resolved"), str)):
            raise ExportError("unsupported consumer lock dependency")
        row, data = pinned_artifact(directory, index, "npm-tarball", name, expected["version"])
        filename = name.removeprefix("@").replace("/", "-") + "-" + expected["version"] + ".tgz"
        if (not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*\.tgz", filename)
                or expected["integrity"] != "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode()
                or row["url"] != expected["resolved"]):
            raise ExportError("npm dependency differs from the carried consumer lock")
        total += len(data)
        record = {"ecosystem": "npm", "name": name, "version": expected["version"], "filename": filename,
                  "sha256": sha(data), "bytes": len(data), "families": ["activepieces"],
                  "integrity": expected["integrity"], "path": "npm-tarball/" + row["file"],
                  "source": {"kind": "pinned_public_index_artifact", "url": row["url"]}}
        artifacts.append(record)
        output = "activepieces/" + filename
        if output in files:
            raise ExportError("dependency would replace selected consumer bytes")
        files[output] = data
        origins[output] = {"kind": "pinned_public_index_artifact", "pin": "npm-tarball/" + row["file"],
                           "url": row["url"], "sha256": sha(data)}
        selected["activepieces"][name] = record
    if total > MAX_DEPENDENCY_BYTES:
        raise ExportError("dependency closure exceeds byte limit")
    bound = ("agno/runtime-constraints.txt", "services/runtime-constraints.txt", "activepieces/package.json",
             "activepieces/package-lock.json", *paths.values())
    closure = encoded({
        "schema_version": 1, "scope": "dependency_bytes_for_existing_consumer_selection",
        "python_platform": MACOS_PYTHON_PLATFORM,
        "pins": {"name": inputs.pins.name, "sha256": sha(pins_raw)},
        "accepted_inputs": {path: sha(files[path]) for path in bound},
        "artifacts": sorted(artifacts, key=lambda row: (row["ecosystem"], row["name"]))})
    closure_sha = sha(closure)

    # Requirement lists and the npm cache record are generated from that closure.
    for family, package, version in (("agno", "meerkat-toolkit-agno", "0.1.5"),
                                     ("services", "meerkat-toolkit-services", "0.1.23")):
        lines = {name: name + "==" + row["version"] + " --hash=sha256:" + row["sha256"]
                 for name, row in selected[family].items()}
        lines[package] = package + "==" + version + " --hash=sha256:" + digests[family]
        files[family + "/install-offline.txt"] = ("\n".join(line for _, line in sorted(lines.items())) + "\n").encode("ascii")
        origins[family + "/install-offline.txt"] = {"kind": "hash_locked_requirements_from_selected_closure",
                                                    "closure_sha256": closure_sha}
        files[family + "/install-requirements.txt"] = ("\n".join("./" + filename for filename in sorted(
            path.split("/", 1)[1] for path in files if path.startswith(family + "/") and path.endswith(".whl"))) + "\n").encode("ascii")
        origins[family + "/install-requirements.txt"] = {"kind": "wheel_list_from_selected_macos_closure",
                                                         "closure_sha256": closure_sha}
    files["activepieces/offline-cache.json"] = encoded({"schema_version": 1, "tarballs": [
        {"name": name, "version": row["version"], "path": row["filename"],
         "sha256": row["sha256"], "integrity": row["integrity"]}
        for name, row in sorted(selected["activepieces"].items())]})
    origins["activepieces/offline-cache.json"] = {"kind": "local_tarballs_for_ordinary_npm_cache_add",
                                                  "closure_sha256": closure_sha}

    # Guides, licences and notices are named members of the components built in this run.
    def document(family: str, member: str, output: str, data: bytes) -> None:
        files[family + "/" + output] = data
        origins[family + "/" + output] = {"artifact": paths[family], "artifact_sha256": digests[family],
                                          "member": member}

    def wheel_documents(family: str, members: list[tuple[str, str]]) -> zipfile.ZipFile:
        archive = zipfile.ZipFile(io.BytesIO(files[paths[family]]))
        for member, output in members:
            matches = [info for info in archive.infolist() if info.filename == member]
            if len(matches) != 1 or matches[0].is_dir() or matches[0].file_size > MAX_FILE:
                raise ExportError("component wheel lacks a document of the bundle")
            document(family, member, output, archive.read(matches[0]))
        return archive

    with wheel_documents("agno", [("meerkat_toolkit_agno/PROFILE-GUIDE.md", "PROFILE-GUIDE.md")] + [
            ("meerkat_toolkit_agno-0.1.5.dist-info/licenses/" + name, name)
            for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md")]) as archive:
        guides = [info for info in archive.infolist() if info.filename == "meerkat_toolkit_agno/CONFIGURATION-GUIDES.json"]
        if len(guides) != 1 or guides[0].file_size > MAX_FILE:
            raise ExportError("agno wheel lacks its profile list")
        listed = json.loads(archive.read(guides[0]))
        listed = listed.get("profiles") if isinstance(listed, dict) else None
        if not isinstance(listed, dict) or sorted(listed) != route_profiles["agno"]:
            raise ExportError("agno wheel profiles differ from the routes")
    wheel_documents("services", [("meerkat_toolkit_services/GUIDE.md", "PROFILE-GUIDE.md")]).close()
    with tarfile.open(fileobj=io.BytesIO(files[paths["activepieces"]]), mode="r:gz") as archive:
        for source, output in (("README.md", "PROFILE-GUIDE.md"), ("LICENSE-MIT", "LICENSE-MIT"),
                               ("LICENSE-APACHE", "LICENSE-APACHE"), ("THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md")):
            member = "package/" + source
            matches = [info for info in archive.getmembers() if info.name == member]
            if len(matches) != 1 or not matches[0].isreg() or matches[0].size > MAX_FILE:
                raise ExportError("component archive lacks a document of the bundle")
            with archive.extractfile(matches[0]) as stream:
                document("activepieces", member, output, stream.read(MAX_FILE + 1))
    for family, path in inputs.profile_captures.items():
        if family not in paths or not isinstance(path, Path) or not path.is_absolute():
            raise ExportError("profile capture needs a component family and an absolute path")
        capture = json.loads(read_file(path.parent.resolve(strict=True), path.name))
        rows = capture.get("profiles") if isinstance(capture, dict) else None
        names = [row.get("profile") if isinstance(row, dict) else None for row in rows] if isinstance(rows, list) else None
        if (names is None or any(not isinstance(name, str) for name in names)
                or len(names) != len(set(names)) or sorted(names) != route_profiles[family]):
            raise ExportError("built component profiles differ from the routes")

    files["README.md"] = read_file(root, MACOS_GUIDE)
    origins["README.md"] = {"path": MACOS_GUIDE, "base": "exporter_checkout",
                            "sha256": sha(files["README.md"]), "kind": "consumer_instructions"}
    files["agno/hackernews.json"] = encoded({"profile": "hackernews", "operations": ["get_top_hackernews_stories"], "config": {"timeout": 10}})
    files["activepieces/tally.json"] = encoded({"profile": "tally.get-form", "token_env": "MY_TALLY_TOKEN"})
    slack_operations = ["auth_test", "list_conversations_page", "get_conversation", "read_interval", "get_permalink"]
    files["services/slack.json"] = encoded({"profile": "slack", "operations": slack_operations, "config": {
        "token_env": "HOST_SELECTED_SLACK_TOKEN", "timeout_seconds": 20,
        "max_response_bytes": 2097152, "max_interval_requests": 30, "max_interval_messages": 1000}})
    files["services/gmail.json"] = encoded({"profile": "gmail", "operations": ["list_messages_page", "get_message", "send_message"], "config": {
        "token_env": "HOST_SELECTED_GMAIL_TOKEN", "user_id": "me", "timeout_seconds": 20,
        "max_response_bytes": 2097152, "max_message_bytes": 1048576}})
    for name in ("agno/hackernews.json", "activepieces/tally.json", "services/slack.json", "services/gmail.json"):
        origins[name] = {"kind": "nonsecret_configuration_example"}

    try:
        pins_key = str(inputs.pins.resolve(strict=True).relative_to(root))
    except ValueError:
        pins_key = inputs.pins.name
    closure_key = inputs.closure_record_key if inputs.closure_record_key is not None else inputs.closure_record.name
    if not isinstance(closure_key, str) or not closure_key or closure_key in CARRIED_PINS or closure_key in (
            pins_key, CATALOG, DISCOVERY_CATALOG):
        raise ExportError("invalid closure record key")
    accepted = {pins_key: sha(pins_raw), CATALOG: CATALOG_SHA, DISCOVERY_CATALOG: PINNED_DISCOVERY_CATALOG_SHA,
                **CARRIED_PINS, closure_key: closure_sha}
    facts = {"components": digests, "catalog_entries_changed": len(versions), "closure_key": closure_key}
    return files, origins, accepted, closure, facts


def export_pinned(root: Path, output: str, inputs: PinnedInputs) -> dict:
    """Write the macOS arm64 bundle and its dependency closure record; the checksums are written last."""
    root = root.resolve(strict=True)
    parent, name, record_parent, record_name = pinned_locations(root, output, inputs)
    files, origins, accepted, closure, facts = collect_pinned(root, inputs)
    # Schema 1 without a runtime target is the macOS contract of the delivered installers.
    contents = {"schema_version": 1, "scope": "consumer_export_only_no_installation_or_activation",
                "accepted_inputs": accepted, "files": {
                    path: {"sha256": sha(data), "source": origins[path]} for path, data in sorted(files.items())}}
    files["CONTENTS.json"] = encoded(contents)
    checksums = "".join(sha(data) + "  " + path + "\n" for path, data in sorted(files.items())).encode()
    record_fd = open_directory(record_parent)
    try:
        write_file(record_fd, record_name, closure)  # Refuses an existing record.
    finally:
        os.close(record_fd)
    parent_fd = open_directory(parent)
    try:
        os.mkdir(name, 0o700, dir_fd=parent_fd)  # Atomic refusal if the destination appeared meanwhile.
        directory = os.open(name, DIR_FLAGS, dir_fd=parent_fd)
        try:
            for family in ("agno", "activepieces", "services"):
                os.mkdir(family, 0o700, dir_fd=directory)
                family_fd = os.open(family, DIR_FLAGS, dir_fd=directory)
                try:
                    for path, data in sorted(files.items()):
                        if path.startswith(family + "/"):
                            write_file(family_fd, path.split("/", 1)[1], data)
                finally:
                    os.close(family_fd)
            for path in ("README.md", "install-library-consumer.py", "catalog.json", "routes.json", "CONTENTS.json"):
                write_file(directory, path, files[path])
            write_file(directory, "SHA256SUMS", checksums)  # Completion marker, written last.
        finally:
            os.close(directory)
    finally:
        os.close(parent_fd)
    return {"output": str(parent / name), "files": len(files) + 1, "mode": "pinned",
            "contents_sha256": sha(files["CONTENTS.json"]), "checksums_sha256": sha(checksums),
            "scope": contents["scope"], "components": facts["components"],
            "catalog_sha256": sha(files["catalog.json"]), "catalog_entries_changed": facts["catalog_entries_changed"],
            "routes_sha256": sha(files["routes.json"]),
            "closure_record": {"path": str(record_parent / record_name), "key": facts["closure_key"],
                               "sha256": sha(closure)}}


def pinned_arguments(args: argparse.Namespace) -> PinnedInputs | None:
    """The pinned mode is selected by --pins alone and then needs every one of its inputs."""
    required = ("pinned_inputs", "agno_wheel", "agno_sha256", "services_wheel", "services_sha256",
                "activepieces_archive", "activepieces_sha256", "closure_record")
    optional = ("closure_record_key",)
    if args.pins is None:
        if any(getattr(args, name) is not None for name in required + optional) or args.profile_capture:
            raise ExportError("pinned-mode options need --pins")
        return None
    if args.target != TARGETS[0] or any(getattr(args, name) is None for name in required):
        raise ExportError("the pinned mode needs every input and the macOS arm64 target")
    captures = {}
    for value in args.profile_capture:
        family, separator, path = value.partition("=")
        if separator != "=" or not path or family in captures:
            raise ExportError("profile captures are FAMILY=FILE, one per family")
        captures[family] = Path(path)
    return PinnedInputs(
        pins=Path(args.pins), pinned_inputs=Path(args.pinned_inputs),
        components={"agno": (Path(args.agno_wheel), args.agno_sha256),
                    "services": (Path(args.services_wheel), args.services_sha256),
                    "activepieces": (Path(args.activepieces_archive), args.activepieces_sha256)},
        closure_record=Path(args.closure_record), closure_record_key=args.closure_record_key,
        profile_captures=captures)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT, help="checkout containing the exact accepted proof inputs")
    parser.add_argument("--output", required=True, help="absolute new directory; its parent must exist")
    parser.add_argument("--target", choices=TARGETS, default=TARGETS[0],
                        help="explicit dependency/runtime profile, never inferred from the export host")
    pinned = parser.add_argument_group(
        "pinned mode", "macOS arm64 from a pin file, the checkout's carried files and components built in the "
        "same run; selected by --pins, which then needs every option of this group except the last two")
    pinned.add_argument("--pins", help="the platform's committed pin file")
    pinned.add_argument("--pinned-inputs", help="directory holding <kind>/<file> for every pin, already fetched")
    pinned.add_argument("--agno-wheel", help="absolute path of the agno wheel built in this run")
    pinned.add_argument("--agno-sha256", help="its sha256, as its build recorded it")
    pinned.add_argument("--services-wheel", help="absolute path of the services wheel built in this run")
    pinned.add_argument("--services-sha256", help="its sha256, as its build recorded it")
    pinned.add_argument("--activepieces-archive", help="absolute path of the Activepieces archive built in this run")
    pinned.add_argument("--activepieces-sha256", help="its sha256, as its build recorded it")
    pinned.add_argument("--closure-record", help="absolute new file for the generated dependency closure record")
    pinned.add_argument("--closure-record-key", help="name binding that record in CONTENTS.json (default: its file name)")
    pinned.add_argument("--profile-capture", action="append", default=[], metavar="FAMILY=FILE",
                        help="JSON output of a built component's profiles command, compared with the routes")
    args = parser.parse_args()
    try:
        inputs = pinned_arguments(args)
        if inputs is not None:
            result = export_pinned(args.root, args.output, inputs)
    except (OSError, ValueError, KeyError, TypeError, AttributeError, IndexError,
            zipfile.BadZipFile, tarfile.TarError):
        print("consumer export failed: check accepted inputs and new destination; any partial destination must not be used", file=sys.stderr)
        return 2
    if inputs is not None:
        print(json.dumps(result, sort_keys=True))
        return 0
    try:
        result = export(args.root, args.output, args.target)
    except (OSError, ValueError, KeyError, zipfile.BadZipFile, tarfile.TarError):
        print("consumer export failed: check accepted inputs and new destination; any partial destination must not be used", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
