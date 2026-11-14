#!/usr/bin/env python3
"""Compose the Linux consumer bundle's catalog and first-party components from this checkout.

The base is an existing Linux consumer bundle directory. The output is a new bundle with the base's
file set and layout in which the catalog, the first-party components and every file derived from them
come from this checkout and from the components supplied in this run. Every other file is the base's,
byte for byte, checked against the base's own checksums first.

This reads files and writes two new destinations (the bundle directory and the dependency closure
record). It installs, downloads, imports and runs nothing; the Python standard library is enough.
"""
from __future__ import annotations

import argparse
import base64
import email.parser
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile
import zipfile
import zlib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
# The checkout's discovery catalog. The retained exporter path pins another file; this one is composed here.
DISCOVERY_CATALOG_SHA = "37e19a7600ba09858c16b4a0eecf9692c001038c8261c327d4020d758de09487"
AP_COMPONENT = "@rkat/toolkit-activepieces"
HEX_64 = re.compile(r"[0-9a-f]{64}")
PIN = re.compile(r"([A-Za-z0-9][A-Za-z0-9_.-]*)==([A-Za-z0-9][A-Za-z0-9.!+_-]*)")
GUIDE_COUNT = re.compile(r"(adds\s+)([0-9]+)( separate discovery records)")
REFUSAL = ("consumer projection failed: check the base bundle, the supplied components and the new destinations; "
           "any partial destination must not be used")
# What a component archive may decompress to while it is checked.
MAX_ARCHIVE_CONTENT = 1 << 30


def load_exporter():
    """The consumer exporter beside this script: its encodings, catalog composition and safe file access."""
    spec = importlib.util.spec_from_file_location("export_library_consumers", HERE / "export-library-consumers.py")
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


EXPORTER = load_exporter()
ProjectionError = EXPORTER.ExportError
sha = EXPORTER.sha
encoded = EXPORTER.encoded


def require(condition: object, message: str) -> None:
    if not condition:
        raise ProjectionError(message)


def canonical(value: object) -> bytes:
    """The encoding the catalog generator digests for an entry's metadata revision."""
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()


def generated(rows: list) -> bytes:
    """The catalog generator's file encoding."""
    return (json.dumps(rows, ensure_ascii=True, indent=2) + "\n").encode()


def normalized(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def component_paths() -> dict[str, str]:
    return {"agno": "agno/" + EXPORTER.AGNO_WHEEL, "activepieces": "activepieces/" + EXPORTER.AP_ARCHIVE,
            "services": "services/" + EXPORTER.SERVICES_WHEEL}


def retained_component_digests() -> dict[str, str]:
    """The component digests the committed adopted catalog names."""
    return {"agno": EXPORTER.AGNO_SHA, "activepieces": EXPORTER.AP_SHA, "services": EXPORTER.SERVICES_SHA}


def wheel_identity(path: str) -> tuple[str, str]:
    """Distribution name and version a first-party wheel must declare, from its fixed file name."""
    name, version = PurePosixPath(path).name.split("-")[:2]
    return normalized(name), version


def derive_catalog(adopted: bytes, digests: dict[str, str]) -> tuple[bytes, dict[str, tuple[str, str]]]:
    """The committed adopted catalog with the given component digests in place of the retained ones.

    An entry's version is the digest of its own metadata, so each entry whose instructions name a
    replaced digest gets its version recomputed the way the catalog generator computes it. Returns the
    catalog bytes and, per changed entry name, its committed and its derived version. With the retained
    digests the result is the committed file.
    """
    rows = json.loads(adopted)
    require(isinstance(rows, list) and generated(rows) == adopted, "adopted catalog is not in its generator's encoding")
    retained = retained_component_digests()
    require(set(digests) == set(retained) and all(isinstance(v, str) and HEX_64.fullmatch(v) for v in digests.values()),
            "one sha256 per component family is required")
    replaced = {retained[family]: digests[family] for family in retained if retained[family] != digests[family]}

    def revision(row: dict) -> str:
        body = {**row, "identity": {key: value for key, value in row["identity"].items() if key != "version"}}
        return "metadata-sha256:" + sha(canonical(body))

    versions: dict[str, tuple[str, str]] = {}
    names = set()
    for row in rows:
        identity = row.get("identity") if isinstance(row, dict) else None
        require(isinstance(identity, dict) and set(identity) == {"publisher", "name", "version"}
                and isinstance(row.get("strategies"), list) and identity["version"] == revision(row)
                and identity["name"] not in names, "adopted catalog entry is not its generator's output")
        names.add(identity["name"])
        changed = False
        for strategy in row["strategies"]:
            text = strategy["instructions"]
            for old, new in replaced.items():
                if old in text:
                    text, changed = text.replace(old, new), True
            strategy["instructions"] = text
        require(not any(old in canonical(row).decode("ascii") for old in replaced),
                "component digest outside the strategy instructions")
        if changed:
            committed = identity["version"]
            identity["version"] = revision(row)
            versions[identity["name"]] = (committed, identity["version"])
    return generated(rows), versions


def derive_routes(base: bytes, runtime_target: object, base_catalog_sha: str, base_digests: dict[str, str],
                  base_versions: dict, catalog: bytes, digests: dict[str, str], versions: dict) -> bytes:
    """The base routes with this run's component digests and the entry versions that follow them.

    Only those values change here; the catalog digest follows in the composition. The target, the
    provenance, the profile lists and every association stay the base's.
    """
    index = json.loads(base)
    paths = component_paths()
    require(isinstance(index, dict) and encoded(index) == base
            and set(index) == {"schema_version", "catalog_sha256", "runtime_target", "provenance", "components", "routes"}
            and type(index["schema_version"]) is int and index["schema_version"] == 1
            and isinstance(index["runtime_target"], dict) and index["runtime_target"] == runtime_target
            and index["catalog_sha256"] == base_catalog_sha
            and isinstance(index["components"], dict) and set(index["components"]) == set(paths)
            and isinstance(index["routes"], list) and len(index["routes"]) <= 512, "unsupported base routes")
    for family, component in index["components"].items():
        profiles = component.get("profiles") if isinstance(component, dict) else None
        require(isinstance(component, dict) and set(component) == {"path", "sha256", "profiles"}
                and component["path"] == paths[family] and component["sha256"] == base_digests[family]
                and isinstance(profiles, list) and profiles and profiles == sorted(set(profiles))
                and all(isinstance(p, str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", p) for p in profiles),
                "base routes name another component selection")
        component["sha256"] = digests[family]
    entries = {tuple(row["identity"][field] for field in ("publisher", "name", "version")): row
               for row in json.loads(catalog)}
    seen = set()
    for route in index["routes"]:
        identity = route.get("identity") if isinstance(route, dict) else None
        require(isinstance(route, dict) and set(route) == {"identity", "strategy_id", "family", "profile"}
                and isinstance(identity, dict) and set(identity) == {"publisher", "name", "version"}
                and all(isinstance(value, str) and value for value in identity.values()), "invalid base route")
        name = identity["name"]
        if name in base_versions or name in versions:
            committed = (base_versions.get(name) or versions[name])[0]
            require(identity["version"] == base_versions.get(name, (committed, committed))[1],
                    "base route names another catalog revision")
            identity["version"] = versions.get(name, (committed, committed))[1]
        key = tuple(identity[field] for field in ("publisher", "name", "version"))
        entry = entries.get(key)
        require(entry is not None and (key, route["strategy_id"]) not in seen
                and sum(strategy.get("id") == route["strategy_id"] for strategy in entry["strategies"]) == 1
                and route["family"] in index["components"]
                and route["profile"] in index["components"][route["family"]]["profiles"],
                "base route does not bind the adopted catalog")
        seen.add((key, route["strategy_id"]))
    index["catalog_sha256"] = sha(catalog)  # The binding the composition requires before it adds discovery rows.
    return encoded(index)


def parse_constraints(data: bytes) -> dict[str, str]:
    pairs = []
    for line in data.decode("ascii").splitlines():
        match = PIN.fullmatch(line)
        require(match is not None, "consumer constraints must be exact local package pins")
        pairs.append((normalized(match[1]), match[2]))
    result = dict(pairs)
    require(pairs and len(pairs) == len(result), "duplicate consumer dependency pin")
    return result


def wheel_member(data: bytes, member: str) -> bytes:
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        matches = [info for info in archive.infolist() if info.filename == member]
        require(len(matches) == 1 and not matches[0].is_dir() and matches[0].file_size <= EXPORTER.MAX_FILE,
                "component wheel lacks a document of the bundle")
        return archive.read(matches[0])


def archive_member(data: bytes, member: str) -> bytes:
    # The whole stream first: one that is cut, damaged or followed by other bytes is no component archive
    # (the tar reader alone stops at the last member it needs).
    total = 0
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
        while block := stream.read(1 << 20):
            total += len(block)
            require(total <= MAX_ARCHIVE_CONTENT, "component archive content is too large")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        matches = [info for info in archive.getmembers() if info.name == member]
        require(len(matches) == 1 and matches[0].isreg() and matches[0].size <= EXPORTER.MAX_FILE,
                "component archive lacks a document of the bundle")
        with archive.extractfile(matches[0]) as stream:
            return stream.read(EXPORTER.MAX_FILE + 1)


def check_wheel(family: str, path: str, data: bytes, constraints: bytes) -> None:
    """Static checks of a supplied first-party wheel; nothing of it is imported or run."""
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        members = [info for info in archive.infolist() if info.filename.endswith(".dist-info/METADATA")]
        require(len(members) == 1 and not members[0].is_dir() and members[0].file_size <= 1024 * 1024,
                "invalid wheel metadata")
        info = email.parser.BytesParser().parsebytes(archive.read(members[0]))
    name, version = info["Name"], info["Version"]
    require(isinstance(name, str) and isinstance(version, str)
            and (normalized(name), version) == wheel_identity(path), "component wheel is not the selected distribution")
    pins = parse_constraints(constraints)
    for requirement in (str(value) for value in info.get_all("Requires-Dist", [])):
        if re.search(r";\s*extra\s*==", requirement):
            continue  # Optional extras are outside the base bundle.
        match = PIN.fullmatch(requirement)
        require(match is not None and pins.get(normalized(match[1])) == match[2],
                "component requires a dependency outside its pinned constraints")


def read_base(directory: Path, contents_sha: str) -> tuple[dict[str, bytes], dict, bytes]:
    """Every file of the base bundle, proven against its own manifests before anything is used."""
    names, stack = [], [""]
    while stack:
        prefix = stack.pop()
        with os.scandir(directory / prefix if prefix else directory) as entries:
            for item in entries:
                relative = prefix + item.name
                if item.is_dir(follow_symlinks=False):
                    require(not prefix, "base bundle nests deeper than one directory")
                    stack.append(relative + "/")
                else:
                    require(item.is_file(follow_symlinks=False), "base bundle holds a link or special file")
                    names.append(relative)
    require(0 < len(names) <= 4096, "unsupported base bundle size")
    files = {name: EXPORTER.read_file(directory, name) for name in names}
    raw, sums = files.pop("CONTENTS.json", None), files.pop("SHA256SUMS", None)
    require(raw is not None and sums is not None and sha(raw) == contents_sha, "base manifest differs from its digest")
    contents = json.loads(raw)
    rows = contents.get("files") if isinstance(contents, dict) else None
    require(isinstance(rows, dict) and encoded(contents) == raw
            and set(contents) == {"schema_version", "scope", "runtime_target", "accepted_inputs", "files"}
            and type(contents["schema_version"]) is int and contents["schema_version"] == 2
            and contents["scope"] == "consumer_export_only_no_installation_or_activation"
            and isinstance(contents["runtime_target"], dict) and isinstance(contents["accepted_inputs"], dict)
            and all(isinstance(key, str) and isinstance(value, str) and HEX_64.fullmatch(value)
                    for key, value in contents["accepted_inputs"].items()), "unsupported base manifest")
    require(set(rows) == set(files) and all(
        isinstance(row, dict) and set(row) == {"sha256", "source"} and isinstance(row["source"], dict)
        and row["sha256"] == sha(files[name]) for name, row in rows.items()), "base manifest does not bind the base files")
    require(checksums({**files, "CONTENTS.json": raw}) == sums, "base checksums do not bind the base files")
    return files, contents, sums


def checksums(files: dict[str, bytes]) -> bytes:
    return "".join(sha(data) + "  " + path + "\n" for path, data in sorted(files.items())).encode()


def new_location(value: str, protected: tuple[Path, ...]) -> tuple[Path, str]:
    """An absolute destination that does not exist yet, under an existing parent and outside every input."""
    path = Path(value)
    require(path.is_absolute() and not any(part in (".", "..") for part in value.split("/")) and path != path.parent,
            "destinations must be absolute new paths without dot components")
    parent = path.parent.resolve(strict=True)
    target = parent / path.name
    require(not any(target == base or base in target.parents for base in protected), "destination inside an input")
    require(not os.path.lexists(target), "destination already exists")
    return parent, path.name


def read_supplied(value: str | None, digest: str | None, filename: str) -> bytes:
    path = Path(value or "")
    require(value and path.is_absolute() and path.name == filename and isinstance(digest, str)
            and HEX_64.fullmatch(digest), "component needs its fixed file name, an absolute path and a sha256")
    return EXPORTER.read_file(path.parent.resolve(strict=True), path.name, digest)


def project(args: argparse.Namespace) -> dict:
    root = Path(args.root).resolve(strict=True)
    base_value = Path(args.base)
    require(base_value.is_absolute() and HEX_64.fullmatch(args.base_contents_sha256 or ""), "base needs an absolute path and a sha256")
    base_dir = base_value.resolve(strict=True)
    record_in = Path(args.closure_record_in)
    require(record_in.is_absolute(), "the closure record needs an absolute path")
    protected = (base_dir, root / Path(EXPORTER.CATALOG).parent)
    parent, name = new_location(args.output, protected)
    record_parent, record_name = new_location(args.closure_record_out, protected + (parent / name,))
    require((args.activepieces_archive is None) == (args.activepieces_sha256 is None),
            "the component archive needs its path and its sha256")

    base, contents, _ = read_base(base_dir, args.base_contents_sha256)
    files, sources = dict(base), {path: row["source"] for path, row in contents["files"].items()}
    paths = component_paths()
    require(all(path in base for path in paths.values()), "base bundle lacks a first-party component")
    base_digests = {family: sha(base[path]) for family, path in paths.items()}

    # The first-party components of this run. The archive stays the base's unless one is supplied.
    supplied = {"agno": read_supplied(args.agno_wheel, args.agno_sha256, EXPORTER.AGNO_WHEEL),
                "services": read_supplied(args.services_wheel, args.services_sha256, EXPORTER.SERVICES_WHEEL)}
    if args.activepieces_archive is not None:
        supplied["activepieces"] = read_supplied(args.activepieces_archive, args.activepieces_sha256, EXPORTER.AP_ARCHIVE)
    for family in ("agno", "services"):
        check_wheel(family, paths[family], base[paths[family]], base[family + "/runtime-constraints.txt"])
        check_wheel(family, paths[family], supplied[family], base[family + "/runtime-constraints.txt"])
    for family, data in supplied.items():
        if data != base[paths[family]]:
            files[paths[family]] = data
            sources[paths[family]] = {"kind": "component_built_in_this_run", "sha256": sha(data)}
    digests = {family: sha(files[path]) for family, path in paths.items()}

    # Documents the exporter copies out of a component are read again from this run's component.
    documents = {"agno": [("meerkat_toolkit_agno/PROFILE-GUIDE.md", "PROFILE-GUIDE.md")] + [
        (PurePosixPath(paths["agno"]).name.rsplit("-", 3)[0] + ".dist-info/licenses/" + notice, notice)
        for notice in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-Agno", "THIRD_PARTY_NOTICES.md")],
                 "services": [("meerkat_toolkit_services/GUIDE.md", "PROFILE-GUIDE.md")],
                 "activepieces": [("package/" + source, output) for source, output in (
                     ("README.md", "PROFILE-GUIDE.md"), ("LICENSE-MIT", "LICENSE-MIT"),
                     ("LICENSE-APACHE", "LICENSE-APACHE"), ("THIRD_PARTY_NOTICES.md", "THIRD_PARTY_NOTICES.md"))]}
    derived_names = set()
    for family, members in documents.items():
        reader = archive_member if family == "activepieces" else wheel_member
        for member, output in members:
            target = family + "/" + output
            derived_names.add(target)
            require(target in base and reader(base[paths[family]], member) == base[target],
                    "base document is not its component's member")
            data = reader(files[paths[family]], member)
            if family in supplied and files[paths[family]] != base[paths[family]]:
                files[target] = data
                sources[target] = {"artifact": paths[family], "artifact_sha256": digests[family], "member": member}
    for target, source in sources.items():
        require(target in derived_names or source.get("artifact") not in paths.values(),
                "base names a component document this projection does not derive")

    routes_base = json.loads(base["routes.json"])
    listed = json.loads(wheel_member(files[paths["agno"]], "meerkat_toolkit_agno/CONFIGURATION-GUIDES.json"))
    listed = listed.get("profiles") if isinstance(listed, dict) else None
    require(isinstance(listed, dict) and isinstance(routes_base, dict) and isinstance(routes_base.get("components"), dict)
            and isinstance(routes_base["components"].get("agno"), dict)
            and sorted(listed) == routes_base["components"]["agno"].get("profiles"), "agno wheel profiles differ from the routes")

    # Hash-locked requirement lists name the component by digest; the wheel lists name files only.
    for family in ("agno", "services"):
        package, version = wheel_identity(paths[family])
        target, lines = family + "/install-offline.txt", {}
        for line in base[target].decode("ascii").splitlines():
            match = re.fullmatch(PIN.pattern + r" --hash=sha256:([0-9a-f]{64})", line)
            require(match is not None and normalized(match[1]) not in lines, "invalid base requirement list")
            lines[normalized(match[1])] = line
        require(lines.get(package) == package + "==" + version + " --hash=sha256:" + base_digests[family]
                and ("\n".join(line for _, line in sorted(lines.items())) + "\n").encode("ascii") == base[target],
                "base requirement list is not the exporter's form")
        lines[package] = package + "==" + version + " --hash=sha256:" + digests[family]
        files[target] = ("\n".join(line for _, line in sorted(lines.items())) + "\n").encode("ascii")
        wheels = ("\n".join("./" + filename for filename in sorted(
            path.split("/", 1)[1] for path in files if path.startswith(family + "/") and path.endswith(".whl"))) + "\n")
        require(base[family + "/install-requirements.txt"] == wheels.encode("ascii"), "base wheel list is not the exporter's form")

    # A supplied archive is the package the consumer lock selects; the lock names it by its sha512.
    if "activepieces" in supplied and supplied["activepieces"] != base[paths["activepieces"]]:
        lock = json.loads(base["activepieces/package-lock.json"])
        require(isinstance(lock, dict) and encoded(lock) == base["activepieces/package-lock.json"]
                and isinstance(lock.get("packages"), dict), "base consumer lock is not in the exporter's encoding")
        component = lock["packages"].get("node_modules/" + AP_COMPONENT)
        package = json.loads(archive_member(supplied["activepieces"], "package/package.json"))
        version = PurePosixPath(paths["activepieces"]).name.removesuffix(".tgz").rsplit("-", 1)[1]
        require(isinstance(component, dict) and component.get("resolved") == "file:" + EXPORTER.AP_ARCHIVE
                and component.get("version") == version and isinstance(package, dict)
                and package.get("name") == AP_COMPONENT and package.get("version") == version
                and package.get("dependencies") == component.get("dependencies")
                and package.get("engines") == component.get("engines"), "component archive differs from the consumer lock")
        component["integrity"] = "sha512-" + base64.b64encode(hashlib.sha512(supplied["activepieces"]).digest()).decode()
        files["activepieces/package-lock.json"] = encoded(lock)
        sources["activepieces/package-lock.json"] = {
            "kind": "consumer_lock_with_this_run_component_integrity", "base": sources["activepieces/package-lock.json"],
            "component_sha256": digests["activepieces"]}

    # Catalog and routes. The base catalog is only compared, by digest; no row of it is carried over.
    adopted = EXPORTER.read_file(root, EXPORTER.CATALOG, EXPORTER.CATALOG_SHA)
    discovery = EXPORTER.read_file(root, EXPORTER.DISCOVERY_CATALOG, DISCOVERY_CATALOG_SHA)
    expected_base, base_versions = derive_catalog(adopted, base_digests)
    expected_rows = json.loads(expected_base)
    adopted_count = len(expected_rows)
    base_rows = json.loads(base["catalog.json"])
    require(isinstance(base_rows, list) and len(base_rows) >= adopted_count
            and sha(canonical(base_rows[:adopted_count])) == sha(canonical(expected_rows)),
            "the base catalog's adopted part is not this checkout's adopted catalog")
    base_discovery_count = len(base_rows) - adopted_count
    del base_rows, expected_rows
    derived, versions = derive_catalog(adopted, digests)
    routes = derive_routes(base["routes.json"], contents["runtime_target"], sha(base["catalog.json"]), base_digests,
                           base_versions, derived, digests, versions)
    files["catalog.json"], files["routes.json"] = EXPORTER.compose_catalog(derived, discovery, routes)
    discovery_count = len(json.loads(discovery))
    require(len(json.loads(files["catalog.json"])) == adopted_count + discovery_count, "composed catalog size")
    discovery_source = {"path": EXPORTER.DISCOVERY_CATALOG, "sha256": DISCOVERY_CATALOG_SHA,
                        "scope": "discovery", "entries": discovery_count}
    base_catalog_source = sources["catalog.json"]
    sources["catalog.json"] = {"kind": "composed_catalog_metadata", "sources": [
        {"path": EXPORTER.CATALOG, "sha256": EXPORTER.CATALOG_SHA, "scope": "adopted", "entries": adopted_count,
         "derivation": {"kind": "component_digests_of_this_run", "entries_changed": len(versions)}}, discovery_source]}
    if files["routes.json"] != base["routes.json"]:
        sources["routes.json"] = {"kind": "derived_from_base_bundle_routes", "sha256": sha(base["routes.json"]),
                                  "base": sources["routes.json"],
                                  "substituted": ["components.sha256", "routes.identity.version", "catalog_sha256"]}

    # The consumer guide states the number of discovery records; it follows the composed catalog.
    guide = base["README.md"].decode("utf-8")
    stated = GUIDE_COUNT.findall(guide)
    require(len(stated) == 1 and stated[0][1] == str(base_discovery_count),
            "the consumer guide does not state the base number of discovery records exactly once")
    if discovery_count != base_discovery_count:
        files["README.md"] = GUIDE_COUNT.sub(lambda match: match[1] + str(discovery_count) + match[3], guide).encode("utf-8")
        sources["README.md"] = {"kind": "consumer_instructions_with_composed_catalog_counts",
                                "base": sources["README.md"], "discovery_records": discovery_count}

    # The dependency closure record: the accepted record, rebound to this bundle's files.
    closure_base = EXPORTER.read_file(record_in.parent.resolve(strict=True), record_in.name)
    closure_base_sha = sha(closure_base)
    accepted = dict(contents["accepted_inputs"])
    base_keys = [key for key, value in accepted.items() if value == closure_base_sha]
    record = json.loads(closure_base)
    platform = record.get("python_platform") if isinstance(record, dict) else None
    require(len(base_keys) == 1 and isinstance(record, dict) and encoded(record) == closure_base
            and type(record.get("schema_version")) is int and record["schema_version"] == 1
            and record.get("scope") == "dependency_bytes_for_existing_consumer_selection"
            and isinstance(platform, dict) and platform.get("system") == "Linux"
            and isinstance(record.get("accepted_inputs"), dict)
            and set(paths.values()) <= set(record["accepted_inputs"])
            and record["accepted_inputs"] == {path: sha(base[path]) for path in record["accepted_inputs"] if path in base},
            "the base bundle does not accept this closure record")
    record["accepted_inputs"] = {path: sha(files[path]) for path in record["accepted_inputs"]}
    closure = encoded(record)
    closure_sha = sha(closure)
    key = args.closure_record_key if args.closure_record_key is not None else record_name
    del accepted[base_keys[0]]
    EXPORTER.relative_parts(key)
    require(key not in accepted and key not in (EXPORTER.CATALOG, EXPORTER.DISCOVERY_CATALOG), "invalid closure record key")
    accepted.update({EXPORTER.CATALOG: EXPORTER.CATALOG_SHA, EXPORTER.DISCOVERY_CATALOG: DISCOVERY_CATALOG_SHA,
                     key: closure_sha})
    for target, source in sources.items():
        if source.get("closure_sha256") == closure_base_sha:
            sources[target] = {**source, "closure_sha256": closure_sha}

    # Manifests, in the exporter's own encoding.
    require(set(files) == set(base), "the projection keeps the base file set")
    manifest = encoded({"schema_version": 2, "scope": contents["scope"], "runtime_target": contents["runtime_target"],
                        "accepted_inputs": accepted, "files": {
                            path: {"sha256": sha(data), "source": sources[path]} for path, data in sorted(files.items())}})
    stale = {base_digests[family] for family in paths if base_digests[family] != digests[family]}
    if closure_sha != closure_base_sha:
        stale.add(closure_base_sha)
    for row in base_catalog_source.get("sources", []) if isinstance(base_catalog_source.get("sources"), list) else []:
        if isinstance(row, dict) and row.get("scope") == "discovery" and row.get("sha256") != DISCOVERY_CATALOG_SHA:
            stale.add(row.get("sha256"))
    texts = [manifest, closure, files["catalog.json"], files["routes.json"],
             files["agno/install-offline.txt"], files["services/install-offline.txt"]]
    require(not any(isinstance(value, str) and value.encode("ascii") in text for value in stale for text in texts),
            "a replaced digest remains in the projected bundle")
    sums = checksums({**files, "CONTENTS.json": manifest})
    changed = sorted(path for path in files if files[path] != base[path]) + ["CONTENTS.json", "SHA256SUMS"]

    # Every check passed. The closure record first, the bundle's checksums last. A write that fails
    # takes the closure record away again: a record without its complete bundle binds nothing.
    record_fd = EXPORTER.open_directory(record_parent)
    try:
        EXPORTER.write_file(record_fd, record_name, closure)
        try:
            write_bundle(parent, name, files, manifest, sums)
        except BaseException:
            os.unlink(record_name, dir_fd=record_fd)
            raise
    finally:
        os.close(record_fd)
    return {"output": str(parent / name), "files": len(files) + 2, "contents_sha256": sha(manifest),
            "checksums_sha256": sha(sums), "catalog_sha256": sha(files["catalog.json"]),
            "catalog_rows": adopted_count + discovery_count, "discovery_rows": discovery_count,
            "catalog_entries_changed": len(versions), "routes_sha256": sha(files["routes.json"]),
            "components": digests, "changed": changed,
            "base": {"contents_sha256": args.base_contents_sha256, "components": base_digests},
            "closure_record": {"path": str(record_parent / record_name), "key": key, "sha256": closure_sha}}


def write_bundle(parent: Path, name: str, files: dict[str, bytes], manifest: bytes, sums: bytes) -> None:
    """The new bundle directory under parent; its checksums are the completion marker, written last."""
    parent_fd = EXPORTER.open_directory(parent)
    try:
        os.mkdir(name, 0o700, dir_fd=parent_fd)
        directory = os.open(name, EXPORTER.DIR_FLAGS, dir_fd=parent_fd)
        try:
            for family in sorted({path.split("/", 1)[0] for path in files if "/" in path}):
                os.mkdir(family, 0o700, dir_fd=directory)
                family_fd = os.open(family, EXPORTER.DIR_FLAGS, dir_fd=directory)
                try:
                    for path, data in sorted(files.items()):
                        if path.startswith(family + "/"):
                            EXPORTER.write_file(family_fd, path.split("/", 1)[1], data)
                finally:
                    os.close(family_fd)
            for path, data in sorted(files.items()):
                if "/" not in path:
                    EXPORTER.write_file(directory, path, data)
            EXPORTER.write_file(directory, "CONTENTS.json", manifest)
            EXPORTER.write_file(directory, "SHA256SUMS", sums)  # Completion marker, written last.
        finally:
            os.close(directory)
    finally:
        os.close(parent_fd)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="absolute directory of the base Linux consumer bundle")
    parser.add_argument("--base-contents-sha256", required=True, help="sha256 of the base bundle's CONTENTS.json")
    parser.add_argument("--agno-wheel", required=True, help="absolute path of the agno wheel built in this run")
    parser.add_argument("--agno-sha256", required=True)
    parser.add_argument("--services-wheel", required=True, help="absolute path of the services wheel built in this run")
    parser.add_argument("--services-sha256", required=True)
    parser.add_argument("--activepieces-archive", help="absolute path of an archive built in this run; default: the base's")
    parser.add_argument("--activepieces-sha256")
    parser.add_argument("--closure-record-in", required=True, help="absolute path of the dependency closure record the base accepts")
    parser.add_argument("--closure-record-out", required=True, help="absolute new file for the rebound record")
    parser.add_argument("--closure-record-key", help="the name CONTENTS.json accepts the new record under; default: its file name")
    parser.add_argument("--output", required=True, help="absolute new directory; its parent must exist")
    parser.add_argument("--root", default=str(ROOT), help="the connect directory whose catalogs are composed")
    args = parser.parse_args(argv)
    try:
        result = project(args)
    except (OSError, ValueError, KeyError, IndexError, TypeError, AttributeError, EOFError, RecursionError,
            zipfile.BadZipFile, tarfile.TarError, zlib.error):
        print(REFUSAL, file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
