"""Inert fixture bytes and authored documents for the installed MCP case.

The consumer owner supplies the actual canonical staging directory, the observed
host planning projection and the actual package-inspection reply. This helper
starts no process, imports no Connect implementation and grants no permission.
"""
from __future__ import annotations

import base64
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile


PACKAGE = "@toolkit-fixtures/source-built-mcp"
VERSION = "1.0.0"
COMPONENT = "source-built-mcp"
STAGED_ID = "source-built-mcp"
PROFILE_SHA256 = "7578190eabe0e22d911dc6174be334332b7d8d5299d03d3fd13b430734b2605b"
HEADERS = ("node_api.h", "node_api_types.h", "js_native_api.h", "js_native_api_types.h")
PACKAGE_FILES = ("package.json", "operation.schema.json", "server.cjs", "should-not-run.cjs", "src/add.c")
HEX = re.compile(r"[0-9a-f]{64}\Z")


def wire(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def require(value, reason):
    if not value:
        raise AssertionError(reason)


def package_document():
    return {
        "schema_version": 1,
        "identity": {"publisher": "toolkit-fixtures", "name": "source-built-mcp", "version": VERSION},
        "display_name": "Controlled source-built MCP fixture",
        "description": "One read-only MCP operation invokes a locally compiled C addon.",
        "provenance": {
            "source": "https://example.invalid/toolkit-fixtures/source-built-mcp",
            "source_revision": "fixture-source-v1", "license": "MIT",
            "notices": ["Synthetic local source fixture; public acquisition and autonomous discovery are not claimed."],
            "local_patches": []},
        "strategies": [{"id": "node-local", "kind": "local_mcp",
            "instructions": "Install the retained npm source archive with lifecycle scripts disabled, compile src/add.c with the explicit host compile plan, then use server.cjs through the ordinary MCP owner.",
            "sources": [], "known_requirements": [], "unresolved_details": []}],
        "known_exports": [{"id": "arithmetic", "role": "service_connector",
            "description": "source_add calls the compiled addon; no service readiness is inferred from installation."}],
        "assessments": []}


def package_files(root):
    root = Path(root)
    require(root.is_absolute() and root.resolve(strict=True) == root, "canonical fixture directory required")
    manifest = json.loads((root / "FIXTURE.json").read_bytes())
    require(manifest["schema_version"] == 1 and set(manifest["files"]) == set(PACKAGE_FILES),
            "exact source-only package inventory required")
    result = {}
    for name in PACKAGE_FILES:
        path = root / name
        require(path.is_file() and not path.is_symlink() and path.resolve(strict=True).is_relative_to(root),
                "package file must be an ordinary retained file")
        raw = path.read_bytes()
        require(len(raw) <= 65536 and manifest["files"][name] == {"bytes": len(raw), "sha256": sha(raw)},
                "fixture package file changed")
        result[name] = raw
    require(json.loads(result["package.json"])["name"] == PACKAGE, "package identity changed")
    return result


def source_archive(files):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
        for name, raw in sorted(files.items()):
            row = tarfile.TarInfo("package/" + name)
            row.size, row.mode, row.mtime = len(raw), 0o644, 0
            bundle.addfile(row, io.BytesIO(raw))
    return gzip.compress(output.getvalue(), mtime=0)


def build_plan(planning):
    # This case explicitly selects the already qualified C command and header
    # IDs. The host projection supplies the target and pins; none are inferred.
    require(planning["scope"] == "selected_node_build_planning" and planning["schema_version"] == 1,
            "actual host build planning projection required")
    require(planning["profile_sha256"] == PROFILE_SHA256, "fixture host profile differs from its selected bytes")
    require(set(planning["executables"]) == {"compile"}, "fixture requires its explicit compile executable ID")
    require(set(planning["files"]) == {name.replace(".", "-") for name in HEADERS},
            "fixture requires the selected four header IDs")
    return {"schema_version": 1, "target": planning["target"],
        "inputs": [{"source": {"npm": "node_modules/" + PACKAGE + "/src/add.c"}, "path": "src/add.c"},
                   *[{"source": {"toolchain": name.replace(".", "-")}, "path": "headers/" + name} for name in HEADERS]],
        "steps": [{"executable": "compile", "arguments": ["-shared", "-fPIC", "-Iheaders",
            "-DNODE_GYP_MODULE_NAME=source_addon", "src/add.c", "-o", "addon.node"]}],
        "outputs": [{"path": "addon.node", "destination": "node_modules/" + PACKAGE + "/build/Release/addon.node",
            "replaces_sha256": None}]}


def stage_and_check_request(fixture_root, staging_root, planning, inspection):
    """Materialize supplied inert source and return one exact helper CHECK.

    The returned request must be submitted by the actual setup helper. The
    caller does not invoke Controller/install/readback or attach MCP here.
    """
    files = package_files(fixture_root)
    package = package_document()
    package_raw = wire(package)
    require(inspection["status"] == "accepted" and inspection["source_digest"] == sha(package_raw),
            "actual package inspection must match these exact authored bytes")
    content_digest = inspection["summary"]["package_content_digest"]
    require(isinstance(content_digest, str) and HEX.fullmatch(content_digest), "owner-computed package digest required")
    staging = Path(staging_root)
    require(staging.is_absolute() and staging.resolve(strict=True) == staging and staging.is_dir(),
            "actual canonical helper staging root required")
    selected = staging / STAGED_ID
    selected.mkdir(mode=0o700)
    closure = selected / "closure"
    closure.mkdir(mode=0o700)
    archive = source_archive(files)
    plan = build_plan(planning)
    project = {"name": "source-built-mcp-case", "version": VERSION, "private": True,
        "dependencies": {PACKAGE: "file:./artifact.tgz"}}
    lock = {"name": project["name"], "version": VERSION, "lockfileVersion": 3, "requires": True,
        "packages": {"": {"name": project["name"], "version": VERSION, "dependencies": project["dependencies"]},
            "node_modules/" + PACKAGE: {"version": VERSION, "resolved": "file:artifact.tgz",
                "integrity": "sha512-" + base64.b64encode(hashlib.sha512(archive).digest()).decode(),
                "hasInstallScript": True}}}
    payloads = {"artifact.tgz": archive, "closure/package-json": wire(project), "closure/package-lock": wire(lock),
                "closure/node-build-plan": wire(plan)}
    for name, raw in payloads.items():
        with (selected / name).open("xb") as output:
            output.write(raw)
    schema = json.loads(files["operation.schema.json"])
    declaration = {"schema_version": 1, "document_kind": "local_software_declaration", "package": package["identity"],
        "package_content_digest": content_digest, "strategy": "node-local",
        "components": [{"id": COMPONENT, "version": VERSION, "runtime": "node",
            "source": {"kind": "artifact", "location": (selected / "artifact.tgz").as_uri(), "content_digest": sha(archive),
                "dependencies": {"kind": "pinned_closure", "artifacts": {
                    "package-json": sha(wire(project)), "package-lock": sha(wire(lock)), "node-build-plan": sha(wire(plan))}},
                "entrypoint": {"executable": "server.cjs", "arguments": []}}}],
        "exports": [{"role": "service_connector", "id": "arithmetic", "components": [COMPONENT],
            "authentication": {"kind": "none"}, "operations": [{"id": "add", "schema_digest": sha(wire(schema)),
                "declared_effect": "read", "required_authority": ["read"], "resources": {"kind": "connection_wide"},
                "result_limits": {"maximum_bytes": 1024, "maximum_items": 1}}]}],
        "portable_mcp_entries": [{"name": COMPONENT, "component": COMPONENT, "operation_tools": {"add": "source_add"}}]}
    return {"profile": "node", "staged_id": STAGED_ID, "component": COMPONENT,
        "package_json": package_raw.decode(), "declaration_json": wire(declaration).decode(), "build_plan_json": wire(plan).decode()}
