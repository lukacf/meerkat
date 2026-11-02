"""Compiler-owned native teams and independently accepted application sources."""
from __future__ import annotations

import hashlib
import io
import json
import re
import uuid
import zipfile

from .artifact_binding import proposal_hash, report_brief_hash
from .blueprint import Blueprint
from .guide_models import review_readiness
from .projects import DraftConflict, fingerprint

NATIVE_SOURCE = "native-team-source.json"
NATIVE_KEYS = ("source_kind", "source_id", "source_digest")
# The native package has a closed compiler surface. Application extensions
# cannot become native source merely by adding them to a rewritten manifest.
_NATIVE_PATHS = frozenset({
    "app.py", "mcp_connections.py", "setup.py", "application_configuration.py", "console_configuration.py",
    "console_auth.py", "remote_operations.py", "connectors.py", "storage_setup.py", "install_gateway.py",
    "native_definition_continuity.py",
    "launchkit/__init__.py", "launchkit/connect_application_finalizer.py",
    "launchkit/connect_application_policy.py", "launchkit/connect_installer_caller.py",
    "launchkit/connect_policy_input.py", "launchkit/connect_application_startup.py",
    "launchkit/connect_application_callbacks.py", "launchkit/connect_member_class_requests.py",
    "console_connect.py", "console_binding.py", "console_presentation.py", "console_shell.py",
    "native_controller_review.py", "native_console_login.py",
    "console_setup.mjs", "console_setup.css", "console_shell.mjs", "console_shell.css",
    "controller_review.mjs", "controller_review.css",
    "toolkit-ui/LICENSE-APACHE", "toolkit-ui/LICENSE-MIT", "toolkit-ui/THIRD_PARTY_NOTICES.md",
    "toolkit-ui/package.json", "toolkit-ui/index.js", "toolkit-ui/index.d.ts",
    "toolkit-ui/model.js", "toolkit-ui/model.d.ts", "toolkit-ui/projection.js", "toolkit-ui/projection.d.ts",
    "toolkit-ui/setup-conversation.js", "toolkit-ui/setup-conversation.d.ts",
    "toolkit-ui/setup-markdown.js", "toolkit-ui/setup-markdown.d.ts",
    "toolkit-ui/setup-model.js", "toolkit-ui/setup-model.d.ts", "toolkit-ui/view.js", "toolkit-ui/view.d.ts",
    "toolkit-ui/style.css", "toolkit-ui/vendor/entities/LICENSE", "toolkit-ui/vendor/entities/package.json",
    "toolkit-ui/vendor/entities/decode-codepoint.js", "toolkit-ui/vendor/entities/decode.js",
    "toolkit-ui/vendor/entities/generated/decode-data-html.js",
    "toolkit-ui/vendor/entities/generated/decode-data-xml.js",
    "toolkit-ui/vendor/entities/internal/bin-trie-flags.js",
    "toolkit-ui/vendor/entities/internal/decode-shared.js", "toolkit-ui/vendor/marked/LICENSE",
    "toolkit-ui/vendor/marked/package.json", "toolkit-ui/vendor/marked/marked.esm.js",
    "start.sh", "install.sh", "blueprint.json", "deployment.json", "requirements.txt", "requirements.sdk.txt",
    "config/mob.toml",
    "config/console.toml", "config/connections.json", "README.md", ".gitignore", "workspace/START-HERE.txt",
    ".env.example", "deploy/deploy-gcp.sh", "deploy/install-service.sh", "deploy/tunnel-gcp.sh", "SHA256SUMS",
    "knowledge/settings.json", "knowledge/manage.py", "knowledge/README.md", "knowledge/.gitignore",
    "knowledge/compose.json", "knowledge/Dockerfile", "knowledge/Dockerfile.dockerignore", NATIVE_SOURCE,
})


def _unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError
        value[key] = item
    return value


def native_fields(value):
    return {key: value[key] for key in NATIVE_KEYS} if value.get("source_kind") == "native-team" else {}


def _inventory(files):
    return {name: {"sha256": hashlib.sha256(content).hexdigest(), "mode": mode}
            for name, (content, mode) in sorted(files.items()) if name != NATIVE_SOURCE}


def compile_native_source(state, version):
    if (type(version) is not int or state.get("version") != version or state.get("status") != "ready"
            or not state.get("blueprint") or state.get("proposal") is not None or state.get("question") is not None
            or (state.get("operation") or {}).get("status") == "running"):
        raise DraftConflict("Accept the current completed plan before installing its agent team.")
    if not review_readiness(state.get("brief") or {})["ready"]:
        raise DraftConflict("Resolve the plan's remaining decisions and implementation work before installing its agent team.")
    spec = Blueprint.model_validate(state["blueprint"])
    from .compiler import generate_package
    # A compiled agent team installs the public Meta release, not MobKit 0.8.
    files = {item.path: (item.content.encode("utf-8"), item.mode)
        for item in generate_package(spec, public_runtime=True)}
    inventory = _inventory(files)
    source = {"protocol": 1, "source_kind": "native-team", "guide_id": state["id"], "guide_version": version,
              "blueprint_hash": proposal_hash(spec.model_dump(mode="json")),
              "brief_hash": fingerprint(state["brief"]), "source_digest": fingerprint(inventory), "files": inventory}
    if state.get("organization") is not None:
        source["protocol"] = 2
        source["organization_hash"] = fingerprint(state["organization"])
    source["source_id"] = fingerprint(source)
    files[NATIVE_SOURCE] = (json.dumps(source, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode(), 0o644)
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, (content, mode) in sorted(files.items()):
            info = zipfile.ZipInfo(name)
            info.create_system = 3
            info.external_attr = (0o100000 | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, content)
    return output.getvalue(), read_package_source(files), spec.project.slug


def read_package_source(files, receipt=None):
    """Read exact package provenance, never manufacture application acceptance."""
    if NATIVE_SOURCE in files:
        try:
            content, mode = files[NATIVE_SOURCE]
            value = json.loads(content, object_pairs_hook=_unique)
            keys = {"protocol", "source_kind", "source_id", "guide_id", "guide_version", "blueprint_hash",
                    "brief_hash", "source_digest", "files"}
            hashes = {"source_id", "source_digest", "blueprint_hash", "brief_hash"}
            if value.get("protocol") == 2:
                keys.add("organization_hash")
                hashes.add("organization_hash")
            if (len(content) > 512_000 or mode != 0o644 or set(value) != keys or value["protocol"] not in (1, 2)
                    or type(value["protocol"]) is not int or value["source_kind"] != "native-team"
                    or type(value["guide_version"]) is not int or value["guide_version"] < 1
                    or str(uuid.UUID(value["guide_id"])) != value["guide_id"]
                    or any(not isinstance(value[key], str) or not re.fullmatch(r"[a-f0-9]{64}", value[key])
                           for key in hashes)
                    or set(files) - _NATIVE_PATHS
                    or value["files"] != _inventory(files) or value["source_digest"] != fingerprint(value["files"])
                    or value["source_id"] != fingerprint({key: item for key, item in value.items() if key != "source_id"})
                    or value["blueprint_hash"] != proposal_hash(json.loads(files["blueprint.json"][0]))):
                raise ValueError
            source = {key: value[key] for key in keys - {"files", "protocol"}}
            source["build_id"] = None
        except (ValueError, KeyError, TypeError, UnicodeError, AttributeError):
            raise ValueError("The compiled agent-team source manifest could not be verified.") from None
    else:
        try:
            report = json.loads(files["implementation-report.json"][0])
            source = {key: report.get(key) for key in ("guide_version", "blueprint_hash", "build_id")}
            if "organization_hash" in report:
                source["organization_hash"] = report["organization_hash"]
            source.update(source_kind="application", source_id=report.get("build_id"),
                          source_digest=report.get("verification", {}).get("digest"), brief_hash=report_brief_hash(report, strict=True))
        except (ValueError, KeyError, TypeError, AttributeError, UnicodeError):
            raise ValueError("The application source report could not be verified.") from None
    if receipt is not None:
        expected_kind = receipt.get("source_kind", "application")
        keys = ("build_id", "guide_version", "blueprint_hash") + (NATIVE_KEYS + ("guide_id", "brief_hash")
                if source["source_kind"] == "native-team" else ())
        if (expected_kind != source["source_kind"] or any(source.get(key) != receipt.get(key) for key in keys)
                or ("organization_hash" in receipt and (
                    "organization_hash" not in source or source["organization_hash"] != receipt["organization_hash"]))
                or (source["source_kind"] == "native-team"
                    and ("organization_hash" in source) != ("organization_hash" in receipt))
                or type(source.get("guide_version")) is not int
                or not isinstance(source.get("source_digest"), str)
                or not re.fullmatch(r"[a-f0-9]{64}", source["source_digest"])):
            raise ValueError("The installed source binding could not be verified.")
    return source
