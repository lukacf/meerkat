"""Supplied no-build package bytes; actual selected owners validate and install.

Reuse the source-built fixture's deterministic archive and declaration shape.
This helper stages inputs only. It never runs a package or claims permission.
"""
import base64
import hashlib
import importlib.util
import json
from pathlib import Path

_path = Path(__file__).parent.parent / "source-built-mcp/setup_fixture.py"
_spec = importlib.util.spec_from_file_location("shared_selected_archive", _path)
_archive = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_archive)
wire, sha, require = _archive.wire, _archive.sha, _archive.require
PACKAGE = "@toolkit-fixtures/shared-selected-mcp"
COMPONENT = "shared-selected-mcp"
PACKAGE_FILES = ("package.json", "operation.schema.json", "server.cjs")


def package_document():
    value = _archive.package_document()
    value["identity"]["name"] = COMPONENT
    value["display_name"] = "Controlled shared preparation cleanup fixture"
    value["description"] = "No-build Node MCP arithmetic package for selected installation custody."
    value["provenance"]["source"] = "https://example.invalid/toolkit-fixtures/shared-selected-mcp"
    value["strategies"][0]["instructions"] = "Install the retained npm archive with lifecycle scripts disabled; no build step."
    value["known_exports"][0]["description"] = "selected_add has no external effects; installation alone does not grant access."
    return value


def stage_and_check_request(root, staging_root, inspection):
    root, staging = Path(root), Path(staging_root)
    manifest = json.loads((root / "FIXTURE.json").read_bytes())
    require(set(manifest["files"]) == set(PACKAGE_FILES), "exact no-build package inventory required")
    files = {}
    for name in PACKAGE_FILES:
        path = root / name
        require(path.is_file() and not path.is_symlink(), "retained package file required")
        raw = path.read_bytes()
        require(manifest["files"][name] == {"bytes": len(raw), "sha256": sha(raw)}, "package input changed")
        files[name] = raw
    require(json.loads(files["package.json"]) == {"name": PACKAGE, "version": "1.0.0", "private": True, "license": "MIT"},
            "no-build package gained a dependency or lifecycle script")
    package = package_document()
    require(inspection["status"] == "accepted" and inspection["source_digest"] == sha(wire(package)),
            "actual package inspection must select these exact inputs")
    content_digest = inspection["summary"]["package_content_digest"]
    require(isinstance(content_digest, str) and _archive.HEX.fullmatch(content_digest), "native package digest required")
    require(staging.is_absolute() and staging.resolve(strict=True) == staging and staging.is_dir(),
            "actual helper staging root required")
    selected = staging / COMPONENT
    selected.mkdir(mode=0o700)
    (selected / "closure").mkdir(mode=0o700)
    artifact = _archive.source_archive(files)
    project = {"name": COMPONENT, "version": "1.0.0", "private": True,
               "dependencies": {PACKAGE: "file:./artifact.tgz"}}
    lock = {"name": COMPONENT, "version": "1.0.0", "lockfileVersion": 3, "requires": True,
        "packages": {"": {"name": COMPONENT, "version": "1.0.0", "dependencies": project["dependencies"]},
            "node_modules/" + PACKAGE: {"version": "1.0.0", "resolved": "file:artifact.tgz",
            "integrity": "sha512-" + base64.b64encode(hashlib.sha512(artifact).digest()).decode()}}}
    payloads = {"artifact.tgz": artifact, "closure/package-json": wire(project), "closure/package-lock": wire(lock)}
    for name, raw in payloads.items():
        with (selected / name).open("xb") as output:
            output.write(raw)
    declaration = {"schema_version": 1, "document_kind": "local_software_declaration", "package": package["identity"],
        "package_content_digest": content_digest, "strategy": "node-local",
        "components": [{"id": COMPONENT, "version": "1.0.0", "runtime": "node",
            "source": {"kind": "artifact", "location": (selected / "artifact.tgz").as_uri(), "content_digest": sha(artifact),
                "dependencies": {"kind": "pinned_closure", "artifacts": {
                    name.removeprefix("closure/"): sha(raw) for name, raw in payloads.items() if name.startswith("closure/")}},
                "entrypoint": {"executable": "server.cjs", "arguments": []}}}],
        "exports": [{"role": "service_connector", "id": "arithmetic", "components": [COMPONENT],
            "authentication": {"kind": "none"}, "operations": [{"id": "add", "schema_digest": sha(wire(json.loads(files["operation.schema.json"]))),
                "declared_effect": "read", "required_authority": ["read"], "resources": {"kind": "connection_wide"},
                "result_limits": {"maximum_bytes": 1024, "maximum_items": 1}}]}],
        "portable_mcp_entries": [{"name": COMPONENT, "component": COMPONENT, "operation_tools": {"add": "selected_add"}}]}
    return {"profile": "node", "staged_id": COMPONENT, "component": COMPONENT,
            "package_json": wire(package).decode(), "declaration_json": wire(declaration).decode()}
