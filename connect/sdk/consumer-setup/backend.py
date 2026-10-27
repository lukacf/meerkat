#!/usr/bin/env python3
"""Ephemeral, trusted-local configuration UI for an operator-pinned consumer bundle.

The installed package parser owns configuration semantics. The unchanged bundle
installer owns installation and its receipts. This controller only holds UI
observations and validation correlations. Immutable nonsecret install requests
can be recovered for explicit readback; they never authorize replay.
"""
from __future__ import annotations

import argparse
import copy
from contextlib import contextmanager
import hashlib
import hmac
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


FAMILIES = ("agno", "services", "activepieces")
OFFICIAL_FAMILY = "official-github"
PLAYWRIGHT_FAMILY = "official-playwright"
OFFICIAL_FAMILIES = (OFFICIAL_FAMILY, PLAYWRIGHT_FAMILY)
SCOPE = "Configuration and local software installation only; account access, host attachment, credentials and MCP launch are not performed."
MAX_BODY = 64 * 1024
MAX_JSON = 2 * 1024 * 1024
INSPECTION_TIMEOUT = 30.0
SELECTED_GUIDE_BYTES = 32 * 1024
MCPB_INSPECTION_TIMEOUT = 120.0
STOP_CHECK_INTERVAL = 0.1
# The selected Python/Node installer allows its child 2 s after TERM. Leave
# additional time for that existing owner to join and unwind before escalation.
SELECTED_INSTALLER_TERMINATION_GRACE = 10
REQUEST_ID = re.compile(r"[0-9a-f]{32}\Z")
HEX = re.compile(r"[0-9a-f]{64}\Z")
REFERENCE = re.compile(r"[A-Za-z_][A-Za-z0-9_]{0,127}\Z")
READBACK_SCOPE = "retained_payload_and_launch_binding_only"
CATALOG_SCOPE = "catalog_metadata_only"
LOCAL_RECIPES_SCOPE = "disclose_nonsecret_local_recipe_metadata"
LOCAL_RECIPES_RETAIN_SCOPE = "retain_and_disclose_nonsecret_local_recipe_metadata"
LOCAL_RECIPES_OUTPUT_SCOPE = "retained_recipe_metadata"
LOCAL_RECIPES_TOOL_BYTES = 64 * 1024 * 1024
LOCAL_RECIPE_PAGE_BYTES = 256 * 1024
LOCAL_RECIPE_CURSOR = re.compile(r"[0-9a-f]{2,4096}\Z")
REMOTE_RECIPE_DOCUMENT = 32 * 1024
REMOTE_EXPORT_FILE = 64 * 1024
REMOTE_EXPORT_RESULT = 256 * 1024
REMOTE_EXPORT_REQUEST = "toolkit.consumer_setup.remote_export_request"
REMOTE_EXPORT_READBACK = "current_portable_export_files_only"
REMOTE_EXPORT_EXCLUSIONS = [
    "Matching files are a current observation, not proof that the historical export succeeded.",
    "No endpoint connection, authentication, operation permission or native attachment is established.",
    "Exclusive host custody is required; this is not an atomic snapshot against concurrent writers.",
]
ACQUISITION_REQUEST = "toolkit.consumer_setup.acquisition_request"
ACQUISITION_READBACK = "retained_acquisition_files_only"
ACQUISITION_ASSETS = {
    "python": {"profile.json", "pip.pyz", "installer.py", "worker.py"},
    "node": {"profile.json", "npm.tgz", "installer.py", "worker.py"},
    "compose": {"profile.json", "runtime.py", "planner", "host-profile.json"},
    "mcpb": {"profile.json", "worker.py"},
    "node-prebuilt": {"profile.json", "installer.py", "fetch.py", "worker.py"},
}
ACQUISITION_SCOPES = {"python": "trusted_pypi_wheel_acquisition", "node": "trusted_npm_registry_acquisition",
                      "compose": "acquire_agent_selected_container_images", "mcpb": "trusted_public_mcpb_acquisition",
                      "node-prebuilt": "trusted_public_node_prebuilt_acquisition"}
ACQUISITION_EXCLUSIONS = [
    "Matching retained files do not establish that the historical acquisition completed successfully.",
    "No network, package code, installer, runtime, engine observation, image pull or graph operation is executed.",
    "Current registry provenance, runtime compatibility, image availability, installation, authentication and native attachment are not established.",
    "Later package.json and the selected activation.json or declaration.json are not acquired-byte proof and must pass normal selected installation validation.",
    "Exclusive quiescent host custody is required; this is not an atomic snapshot or evidence that an earlier worker has stopped.",
]
READBACK_EXCLUSIONS = [
    "Runtime executable integrity and compatibility are not verified.",
    "Python bootstrap, generated metadata, wrappers and bytecode integrity are not verified.",
    "npm-generated hidden lock metadata is not verified.",
    "No installed configuration validator, connector, authentication or native attachment is run or established.",
    "This is a point-in-time observation under exclusive host custody, not permission or a guarantee of safe execution.",
]
MCPB_READBACK_EXCLUSIONS = [
    "The pinned installer checks the retained archive tree, launch binding and current Node executable hash/version; it does not launch the selected server.",
    "No MCP readiness, credential lookup, authentication, account access or native attachment is established.",
    "This is a point-in-time observation under exclusive host custody, not permission or a guarantee of safe execution.",
]


class SetupError(Exception):
    def __init__(self, code: str, status: int = 400):
        super().__init__(code)
        self.code, self.status = code, status


RECIPE_REJECTION_HELP = {
    "mcp_schema": 'Set mcp_json.$schema to https://agent-plugins.org/schemas/1.0.0/mcp.schema.json.',
    "mcp_document": "Check mcp_json against the Agent Plugins MCP document format, including field types and duplicate or unknown fields.",
    "plugin_document": "Check plugin_json against the Agent Plugins manifest format.",
    "recipe_document": "Check recipes_json against the Toolkit LibraryRecipeDocument format.",
    "bundle_contract": "Check that the plugin, recipe, endpoint, authentication and strategy declarations agree with the Toolkit bundle contract.",
}

SELECTED_REJECTION_HELP = {
    "invalid_package": "Correct package.json to match the Toolkit IntegrationPackage format, then inspect it or submit the corrected metadata to setup_selected_check.",
    "invalid_selection": "Correct the explicitly selected activation.json or declaration.json to match its Toolkit format and selected package, then submit the corrected metadata to setup_selected_check.",
    "unsupported_python_selection": "Choose a supported Python component and explicit software declaration or full activation in the package metadata, then submit the corrected metadata to setup_selected_check.",
    "unsupported_node_selection": "Choose a supported Node component and explicit software declaration or full activation in the package metadata, then submit the corrected metadata to setup_selected_check.",
}
# Exact canonical MCPB check refusals only. These are presentation hints, not
# independent validation or claims that the check left no prepared files.
MCPB_REJECTION_HELP = {
    "invalid_document": "Check that package.json, activation.json and configuration.json are valid supported JSON documents without duplicate fields.",
    "invalid_package": "Correct package.json to match the Toolkit IntegrationPackage format, then inspect it or submit the corrected metadata to setup_selected_check.",
    "invalid_selection": "Correct activation.json to match the Toolkit ActivationManifest format and selected package, then submit the corrected metadata to setup_selected_check.",
    "configuration_selection_mismatch": "Bind activation.json configuration.content_digest to the SHA-256 of the exact configuration.json bytes submitted for this check.",
    "unsupported_selection": "Choose a supported independent local Node MCPB component and service export. Check the MCPB installer contract before submitting corrected metadata.",
    "unsupported_mcpb_launch": "Compare the selected script, arguments and explicit nonsecret string configuration with the inspected MCPB manifest and the supported installer contract.",
}
MCPB_REJECTION_SUFFIX = ": no readiness claim; inspect the selected destination before any new attempt\n"
SELECTED_DIAGNOSTIC_BYTES = 128

# Presentation vocabulary only. The installed service validator owns the rules
# and chooses a code; this layer never infers a cause from submitted values.
CONFIGURATION_DIAGNOSTICS = {
    "invalid_document": (((),), "Use an object containing profile, operations and config."),
    "invalid_config_file": (((),), "The selected configuration file could not be read as valid JSON."),
    "invalid_profile": ((("profile",),), "Choose a supported service profile."),
    "invalid_operations": ((("operations",),), "Choose a nonempty set of supported operations without duplicates."),
    "invalid_config_fields": ((("config",),), "Supply the required fields for the selected operations and remove unsupported fields."),
    "invalid_channel_ids": ((("config", "allowed_channel_ids"),), "Use at most 256 unique channel IDs containing only letters and digits, each 1-128 characters."),
    "channel_list_conflict": ((("operations",),), "Channel restrictions cannot be combined with list_conversations_page. Keep the restriction and deselect that operation before validating again."),
    "invalid_recipient_addresses": ((("config", "allowed_recipient_addresses"),), "Use at most 256 unique bare mailbox addresses, without display names or wildcards."),
    "recipient_operation_conflict": ((("operations",),), "Recipient restrictions cannot be combined with send_raw or create_draft_raw. Keep the restriction and choose structured send_message for sending."),
    "invalid_credential_reference": ((("config", "token_env"),), "Enter an environment variable name, not a credential value. Use letters, digits and underscores, beginning with a letter or underscore, at most 128 characters."),
    "integer_out_of_range": (tuple(("config", field) for field in (
        "timeout_seconds", "max_response_bytes", "max_message_bytes", "max_content_bytes",
        "max_interval_requests", "max_interval_messages", "max_interval_result_bytes")), "Use a whole number within the bounds in the configuration reference."),
    "invalid_mailbox": ((("config", "user_id"),), "Use me or a supported bare ASCII mailbox address for user_id."),
    "invalid_calendar_id": ((("config", "calendar_id"),), "Choose a nonempty calendar ID without control characters or dot path segments. This fixes request routing; it does not grant calendar access."),
}


def configuration_diagnostic(raw, family, profile, configuration_sha, contents_sha):
    """Accept only the installed owner's bounded, value-free failure protocol."""
    if family != "services" or profile not in ("slack", "gmail", "google-calendar", "google-drive") or len(raw) > 1024:
        return None
    try:
        value = decode(raw)
        if (type(value) is not dict or set(value) != {"valid", "scope", "diagnostic"}
                or value["valid"] is not False or value["scope"] != "configuration_only"):
            return None
        diagnostic = value["diagnostic"]
        if (type(diagnostic) is not dict or set(diagnostic) != {"version", "code", "field_path"}
                or type(diagnostic["version"]) is not int or diagnostic["version"] != 1
                or type(diagnostic["code"]) is not str or diagnostic["code"] not in CONFIGURATION_DIAGNOSTICS
                or type(diagnostic["field_path"]) is not list
                or any(type(item) is not str for item in diagnostic["field_path"])):
            return None
        code, path = diagnostic["code"], tuple(diagnostic["field_path"])
        paths, message = CONFIGURATION_DIAGNOSTICS[code]
        if path not in paths:
            return None
        if (profile != "slack" and (code in {"invalid_channel_ids", "channel_list_conflict"}
                or path in {("config", "max_interval_requests"), ("config", "max_interval_messages"),
                            ("config", "max_interval_result_bytes")})):
            return None
        if (profile != "gmail" and (code in {"invalid_recipient_addresses", "recipient_operation_conflict", "invalid_mailbox"}
                or path == ("config", "max_message_bytes"))):
            return None
        if profile != "google-calendar" and code == "invalid_calendar_id":
            return None
        if profile != "google-drive" and path == ("config", "max_content_bytes"):
            return None
        return {"version": 1, "phase": "configuration_validation", "code": code,
                "field_path": list(path), "message": message, "family": family, "profile": profile,
                "configuration_sha256": configuration_sha, "contents_sha256": contents_sha}
    except (SetupError, TypeError, ValueError):
        return None


def setup_error_payload(error):
    """Public help is fixed host text; never serialize child diagnostic text."""
    value = {"error": error.code}
    if error.code == "plugin_metadata_rejected":
        value["diagnostic"] = {"phase": "before_plugin_retention", "message":
            "Check the explicitly selected public plugin files, supported metadata versions and required contracts. No package code was executed."}
    if error.code == "invalid_configuration" and getattr(error, "configuration_diagnostic", None):
        value["diagnostic"] = copy.deepcopy(error.configuration_diagnostic)
    for code, message in RECIPE_REJECTION_HELP.items():
        if error.code == "recipe_" + code + "_rejected":
            value["diagnostic"] = {"phase": "before_store_open", "code": code, "message": message}
            break
    for code, message in SELECTED_REJECTION_HELP.items():
        if error.code == "selected_" + code:
            value["diagnostic"] = {"phase": "before_installation", "code": code, "message": message}
            if code == "invalid_package" and getattr(error, "package_diagnostic", None):
                value["diagnostic"].update(copy.deepcopy(error.package_diagnostic))
                value["diagnostic"]["message"] = (
                    "Correct the authored field at field_path using the selected-metadata guide, then inspect again. "
                    "The path may stop at the nearest known container. Preserve valid notices, unresolved details "
                    "and setup facts; move prose to the appropriate text field instead of discarding it. "
                    "This diagnostic establishes no installation or runtime readiness.")
            break
    for code, message in MCPB_REJECTION_HELP.items():
        if error.code == "selected_mcpb_" + code:
            value["diagnostic"] = {"phase": "before_installation", "profile": "mcpb",
                "code": code, "message": message +
                " The failed check may retain prepared files. A corrected check creates a new proposal; no installation has been requested."}
            break
    return value


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encoded(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False) + "\n").encode("ascii")


def decode(data: bytes):
    def pairs(rows):
        result = {}
        for key, value in rows:
            if key in result:
                raise ValueError("duplicate key")
            result[key] = value
        return result
    def constant(_):
        raise ValueError("nonfinite value")
    try:
        return json.loads(data, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise SetupError("invalid_json") from error


def keys(value, expected):
    if not isinstance(value, dict) or set(value) != set(expected):
        raise SetupError("invalid_request")


def catalog_records(data: bytes):
    """Bound presentation of already pinned metadata, not package semantics.

    IntegrationPackage remains the canonical Rust metadata owner. This reader
    retains complete records and checks only the supported envelope, identity
    uniqueness and rendering bounds. It never derives installed profile IDs.
    """
    try:
        rows = decode(data)
        if not isinstance(rows, list) or not 1 <= len(rows) <= 512 or len(encoded(rows)) > MAX_JSON:
            raise ValueError()
        identities = set()
        for row in rows:
            if (not isinstance(row, dict) or type(row.get("schema_version")) is not int
                    or row["schema_version"] != 1 or len(encoded(row)) > MAX_BODY):
                raise ValueError()
            identity = row.get("identity")
            if not isinstance(identity, dict) or set(identity) != {"publisher", "name", "version"}:
                raise ValueError()
            if any(not isinstance(v, str) or not 1 <= len(v) <= 256 for v in identity.values()):
                raise ValueError()
            key = tuple(identity[k] for k in ("publisher", "name", "version"))
            if key in identities:
                raise ValueError()
            identities.add(key)
            if any(not isinstance(row.get(k), str) or not row[k] for k in ("display_name", "description")):
                raise ValueError()
            if not isinstance(row.get("provenance"), dict) or not isinstance(row.get("strategies"), list):
                raise ValueError()
            pending, count = [(row, 0)], 0
            while pending:
                item, depth = pending.pop()
                count += 1
                if count > 4096 or depth > 12:
                    raise ValueError()
                if isinstance(item, dict):
                    if any(len(k) > 256 for k in item):
                        raise ValueError()
                    pending.extend((v, depth + 1) for v in item.values())
                elif isinstance(item, list):
                    pending.extend((v, depth + 1) for v in item)
        return rows
    except (SetupError, ValueError, TypeError, RecursionError) as error:
        raise SetupError("invalid_catalog") from error


def relative(name: str) -> tuple[str, ...]:
    if not isinstance(name, str) or len(name) > 512 or "\\" in name:
        raise SetupError("invalid_bundle")
    path = PurePosixPath(name)
    if path.is_absolute() or not path.parts or path.as_posix() != name:
        raise SetupError("invalid_bundle")
    if any(part in (".", "..") or not re.fullmatch(r"[A-Za-z0-9_.+@-]+", part) for part in path.parts):
        raise SetupError("invalid_bundle")
    return path.parts


def read_file(root: Path, name: str, limit: int = MAX_JSON) -> bytes:
    """Bounded regular-file read without following descendants' symlinks."""
    parts = relative(name)
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            next_fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
            os.close(directory)
            directory = next_fd
        fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
                raise SetupError("invalid_file")
            chunks, size = [], 0
            while True:
                chunk = os.read(fd, min(1024 * 1024, limit + 1 - size))
                if not chunk:
                    break
                chunks.append(chunk)
                size += len(chunk)
                if size > limit:
                    raise SetupError("invalid_file")
            return b"".join(chunks)
        finally:
            os.close(fd)
    finally:
        os.close(directory)


def write_new(path: Path, data: bytes):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
    except BaseException:
        # Preserve partial files; a new workspace is required after interruption.
        raise


def canonical_directory(value: Path) -> Path:
    """Recovery accepts canonical absolute paths, never symlink aliases."""
    if not value.is_absolute() or value.resolve(strict=True) != value:
        raise SetupError("invalid_recovery_path")
    fd = os.open(value, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    os.close(fd)
    return value


def local_recipe_database(value):
    """Check the host's existing path without opening or interpreting SQLite."""
    if not isinstance(value, str) or not 1 <= len(value) <= 4096 or "\0" in value:
        raise ValueError()
    path = Path(value)
    if not path.is_absolute():
        raise ValueError()
    canonical_directory(path.parent)
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError()


def load_local_recipes_profile(path, expected):
    """The host explicitly permits disclosure; this does not sanitize records."""
    try:
        if (not isinstance(path, Path) or not path.is_absolute()
                or not isinstance(expected, str) or not HEX.fullmatch(expected)):
            raise ValueError()
        raw = read_file(canonical_directory(path.parent), path.name, MAX_BODY)
        if digest(raw) != expected:
            raise ValueError()
        value = decode(raw)
        keys(value, ("schema_version", "scope", "database", "realm", "owner", "tool"))
        if (type(value["schema_version"]) is not int or value["schema_version"] != 1
                or value["scope"] not in (LOCAL_RECIPES_SCOPE, LOCAL_RECIPES_RETAIN_SCOPE)
                or any(not isinstance(value[k], str) or not 1 <= len(value[k]) <= 256
                       or "\0" in value[k] for k in ("realm", "owner"))):
            raise ValueError()
        local_recipe_database(value["database"])
        tool = value["tool"]
        keys(tool, ("path", "sha256"))
        if (not isinstance(tool["path"], str) or not 1 <= len(tool["path"]) <= 4096
                or "\0" in tool["path"] or not Path(tool["path"]).is_absolute()
                or not isinstance(tool["sha256"], str) or not HEX.fullmatch(tool["sha256"])):
            raise ValueError()
        source = Path(tool["path"])
        payload = read_file(canonical_directory(source.parent), source.name, LOCAL_RECIPES_TOOL_BYTES)
        if digest(payload) != tool["sha256"] or not os.access(source, os.X_OK):
            raise ValueError()
        return value, raw, payload
    except (SetupError, OSError, TypeError, ValueError, RecursionError):
        raise SetupError("invalid_local_recipes_profile") from None


def recipe_page_arguments(value):
    if not isinstance(value, dict) or set(value) - {"limit", "cursor"} or "limit" not in value:
        raise SetupError("invalid_request")
    if type(value["limit"]) is not int or not 1 <= value["limit"] <= 50:
        raise SetupError("invalid_request")
    if "cursor" in value and (not isinstance(value["cursor"], str)
            or not REQUEST_ID.fullmatch(value["cursor"])):
        raise SetupError("invalid_request")
    return copy.deepcopy(value)


def local_recipe_output(raw, request, page=None):
    """Validate the observation envelope; the retained-recipe owner owns records."""
    if not isinstance(raw, bytes) or len(raw) > MAX_JSON:
        raise ValueError()
    value = decode(raw)
    if (not isinstance(value, dict) or type(value.get("schema_version")) is not int
            or value["schema_version"] != 1 or value.get("scope") != LOCAL_RECIPES_OUTPUT_SCOPE
            or len(encoded(value)) > MAX_JSON):
        raise ValueError()
    envelope = ("schema_version", "scope", "status")
    if page is not None:
        keys(value, (*envelope, "recipes", "unverified", "oversized", "next_cursor", "scanned_records", "request_cursor", "limit"))
        if (value["status"] != "listed_page" or len(raw) > LOCAL_RECIPE_PAGE_BYTES
                or value["request_cursor"] != page.get("cursor")
                or type(value["limit"]) is not int or value["limit"] != page["limit"]
                or type(value["scanned_records"]) is not int or not 0 <= value["scanned_records"] <= 200
                or any(not isinstance(value[k], list) or any(not isinstance(row, dict) for row in value[k])
                       for k in ("recipes", "unverified", "oversized"))):
            raise ValueError()
        count = sum(len(value[k]) for k in ("recipes", "unverified", "oversized"))
        if count > page["limit"] or count > value["scanned_records"]:
            raise ValueError()
        cursor = value["next_cursor"]
        if cursor is not None and (not isinstance(cursor, str) or not LOCAL_RECIPE_CURSOR.fullmatch(cursor)
                or len(cursor) % 2 or cursor == page.get("cursor") or value["scanned_records"] == 0):
            raise ValueError()
        for row in value["unverified"]:
            keys(row, ("error",))
            if row["error"] != "stored_recipe_unverified":
                raise ValueError()
        for row in value["oversized"]:
            keys(row, ("identity", "digest", "error"))
            keys(row["identity"], ("publisher", "name", "version"))
            if (row["error"] != "summary_too_large" or not selected_hash(row["digest"])
                    or any(not isinstance(item, str) or not 1 <= len(item) <= 256
                           or "\0" in item for item in row["identity"].values())):
                raise ValueError()
    elif request is None:
        keys(value, (*envelope, "recipes", "unverified"))
        if (value["status"] != "listed"
                or any(not isinstance(value[k], list) or any(not isinstance(row, dict) for row in value[k])
                       for k in ("recipes", "unverified"))):
            raise ValueError()
    elif value.get("status") == "not_found":
        keys(value, envelope)
    else:
        keys(value, (*envelope, "recipe"))
        recipe = value["recipe"]
        if (value["status"] != "found" or not isinstance(recipe, dict)
                or recipe.get("identity") != {key: request[key] for key in ("publisher", "name", "version")}
                or recipe.get("digest") != request["digest"]):
            raise ValueError()
    return value


def retention_arguments(value):
    """Bound caller-provided historical references, without attesting to them."""
    keys(value, ("job_id", "decision", "verification"))
    if not isinstance(value["job_id"], str) or not REQUEST_ID.fullmatch(value["job_id"]):
        raise SetupError("invalid_request")
    if not isinstance(value["verification"], list) or not 1 <= len(value["verification"]) <= 32:
        raise SetupError("invalid_request")
    for reference in [value["decision"], *value["verification"]]:
        keys(reference, ("owner", "record", "revision"))
        for key in ("owner", "record"):
            item = reference[key]
            if (not isinstance(item, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/:@-]{0,191}", item)
                    or any(part in ("", ".", "..") for part in item.split("/"))):
                raise SetupError("invalid_request")
        revision = reference["revision"]
        if (not isinstance(revision, str) or not revision.strip()
                or any(ord(char) < 32 or 127 <= ord(char) <= 159 or 0xD800 <= ord(char) <= 0xDFFF for char in revision)
                or len(revision.encode("utf-8")) > 256):
            raise SetupError("invalid_request")
    if len({encoded(item) for item in value["verification"]}) != len(value["verification"]):
        raise SetupError("invalid_request")
    return copy.deepcopy(value)


def remote_recipe_arguments(value, exporting=False):
    """Bound inline documents; Rust remains their schema and semantic owner."""
    if not isinstance(value, dict):
        raise SetupError("invalid_request")
    document_key = "declaration_json" if "declaration_json" in value else "activation_json"
    expected = (("publisher", "name", "version", "digest", document_key) if exporting else
                ("plugin_json", "mcp_json", "recipes_json", *(("decision",) if "decision" in value else ())))
    value = copy.deepcopy(value)
    if not exporting:
        value.setdefault("mcp_json", None)
    keys(value, expected)
    documents = (document_key,) if exporting else ("plugin_json", "mcp_json", "recipes_json")
    try:
        for name in documents:
            text = value[name]
            if name == "mcp_json" and text is None:
                continue
            if not isinstance(text, str) or not 1 <= len(text.encode("utf-8")) <= REMOTE_RECIPE_DOCUMENT:
                raise ValueError()
        if len(encoded(value)) > MAX_BODY:
            raise ValueError()
        if exporting:
            for name in ("publisher", "name", "version"):
                if not isinstance(value[name], str) or not 1 <= len(value[name]) <= 256 or "\0" in value[name]:
                    raise ValueError()
            if not isinstance(value["digest"], str) or not HEX.fullmatch(value["digest"]):
                raise ValueError()
        elif "decision" in value:
            remote_recipe_reference(value["decision"])
    except (ValueError, UnicodeError, RecursionError):
        raise SetupError("invalid_request") from None
    return value


def remote_export_document(request):
    """Fixed owner-selected names for the two explicit export input forms."""
    return (("declaration_json", "declaration.json", "--declaration") if "declaration_json" in request else
            ("activation_json", "activation.json", "--selection"))


def remote_export_kind(value, request):
    """Bind the canonical output kind to this exact retained request form."""
    if "declaration_json" in request:
        if value.get("selection_kind") != "remote_declaration":
            raise ValueError()
        return {"selection_kind": "remote_declaration"}
    if "selection_kind" in value:
        raise ValueError()
    return {}


def remote_recipe_reference(value):
    """Bound a reference projection; only its owner can verify its provenance."""
    keys(value, ("owner", "record", "revision"))
    for name, text in value.items():
        if not isinstance(text, str) or not 1 <= len(text.encode("utf-8")) <= (256 if name == "revision" else 192) or "\0" in text:
            raise ValueError()


def remote_recipe_summary(value, identity=None, decision=None):
    keys(value, ("identity", "digest", "package_content_digest", "origin", "display_name", "strategies", "exports"))
    keys(value["identity"], ("publisher", "name", "version"))
    if (any(not isinstance(text, str) or not 1 <= len(text) <= 256 or "\0" in text for text in value["identity"].values())
            or any(not isinstance(value[k], str) or not HEX.fullmatch(value[k]) for k in ("digest", "package_content_digest"))
            or not isinstance(value["display_name"], str) or not value["display_name"]
            or any(not isinstance(value[k], list) for k in ("strategies", "exports"))):
        raise ValueError()
    if identity is not None and (value["identity"] != {k: identity[k] for k in ("publisher", "name", "version")} or value["digest"] != identity["digest"]):
        raise ValueError()
    keys(value["origin"], ("kind", "decision"))
    if value["origin"]["kind"] != "local_investigation":
        raise ValueError()
    remote_recipe_reference(value["origin"]["decision"])
    if decision is not None and value["origin"]["decision"] != decision:
        raise ValueError()


def remote_recipe_envelope(raw, status, extra, optional=()):
    if not isinstance(raw, bytes) or len(raw) > REMOTE_EXPORT_RESULT:
        raise ValueError()
    value = decode(raw)
    keys(value, ("schema_version", "scope", "status", *extra, *(k for k in optional if k in value)))
    if type(value["schema_version"]) is not int or value["schema_version"] != 1 or value["scope"] != LOCAL_RECIPES_OUTPUT_SCOPE or value["status"] != status:
        raise ValueError()
    return value


def remote_recipe_save_failure(raw):
    """Only an exact, pre-store owner diagnostic supports an input correction."""
    if not isinstance(raw, bytes) or len(raw) > REMOTE_EXPORT_RESULT:
        raise ValueError()
    value = decode(raw)
    if isinstance(value, dict) and "diagnostic" in value:
        value = remote_recipe_envelope(raw, "error", ("error", "diagnostic"))
        keys(value["diagnostic"], ("phase", "code"))
        code = value["diagnostic"]["code"]
        if (value["error"] != "invalid_bundle" or value["diagnostic"]["phase"] != "before_store_open"
                or not isinstance(code, str) or code not in RECIPE_REJECTION_HELP):
            raise ValueError()
        return "recipe_" + code + "_rejected"
    value = remote_recipe_envelope(raw, "error", ("error",))
    return "retention_conflict" if value["error"] == "immutable_conflict" else "retention_unconfirmed"


def retention_output(raw, code, request, effect, plan, display_name):
    """Accept only the canonical owner's exact bound retention summary."""
    if not isinstance(raw, bytes) or len(raw) > MAX_JSON:
        raise ValueError()
    value = decode(raw)
    if (not isinstance(value, dict) or type(value.get("schema_version")) is not int
            or value["schema_version"] != 1 or value.get("scope") != LOCAL_RECIPES_OUTPUT_SCOPE
            or len(encoded(value)) > MAX_JSON):
        raise ValueError()
    if code != 0:
        if (set(value) == {"schema_version", "scope", "status", "error"}
                and value["status"] == "error" and value["error"] == "immutable_conflict"):
            return None
        raise ValueError()
    keys(value, ("schema_version", "scope", "status", "recipe"))
    if value["status"] != "retained":
        raise ValueError()
    identity = selected_identity(plan, effect["request"])
    expected = {"kind": "local_software_selection" if "document_kind" in identity else "local_selection",
                "identity": plan["package"], "package_content_digest": plan["package_content_digest"],
                **identity, "profile": effect["request"]["profile"],
                "component": plan["component"], "display_name": display_name,
                "origin": {"kind": "local_investigation", "decision": request["decision"]},
                "verification": request["verification"],
                "reuse": "requires_current_host_selection_and_artifact_validation"}
    if effect["request"]["profile"] == "mcpb":
        expected["configuration_digest"] = effect["request"]["configuration_sha256"]
    if "plugin_source" in effect["request"]:
        expected["plugin_source_digest"] = effect["request"]["plugin_source"]["inspection_sha256"]
    recipe = value["recipe"]
    keys(recipe, (*expected, "digest"))
    if not selected_hash(recipe["digest"]) or encoded({k: recipe[k] for k in expected}) != encoded(expected):
        raise ValueError()
    return value


def sync_directory(path: Path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def sync_file(root: Path, name: str):
    # All callers select fixed, direct child names in an owned directory.
    if len(relative(name)) != 1:
        raise SetupError("invalid_file")
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        try:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                raise SetupError("invalid_file")
            os.fsync(fd)
        finally:
            os.close(fd)
        os.fsync(directory)
    finally:
        os.close(directory)


def load_export_recovery(root, identifier, expected, profile_sha256):
    """Load a host-selected export request, never paths or authority from a tool."""
    if not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier) or not selected_hash(expected):
        raise SetupError("invalid_recovery_startup")
    root = canonical_directory(root)
    raw = read_file(root, "requests/" + identifier + ".json", MAX_BODY * 2)
    if digest(raw) != expected:
        raise SetupError("recovery_request_changed")
    record = decode(raw)
    if not isinstance(record, dict) or record.get("kind") != REMOTE_EXPORT_REQUEST:
        return None
    keys(record, ("schema_version", "kind", "job_id", "workspace", "profile_sha256", "request_sha256", "request"))
    request = remote_recipe_arguments(record["request"], exporting=True)
    if (type(record["schema_version"]) is not int or record["schema_version"] != 1
            or record["job_id"] != identifier or record["workspace"] != str(root)
            or record["profile_sha256"] != profile_sha256
            or record["request_sha256"] != digest(encoded(request))):
        raise SetupError("invalid_recovery_request")
    directory = root / "remote-recipes" / ("export-" + identifier)
    canonical_directory(directory)
    document_key, document_name, _ = remote_export_document(request)
    for name, data in {"request.json": encoded(request), document_name: request[document_key].encode("utf-8")}.items():
        if read_file(root, str(directory.relative_to(root) / name), MAX_BODY) != data:
            raise SetupError("recovery_request_changed")
    return record, {"kind": "export", "root": root, "directory": directory,
                    "destination": directory / "output", "request": request,
                    "recovery_anchor": {"root": root, "id": identifier, "sha256": expected}}


def configuration_bytes(selection):
    """Existing family byte projection only; installed parsers own meaning."""
    values, profile = selection["values"], selection["profile"]
    configuration = {"profile": profile, **values} if selection["family"] == "activepieces" else {
        "profile": profile, "operations": selection["operations"], "config": values}
    data = encoded(configuration)
    if len(data) > MAX_BODY:
        raise SetupError("invalid_configuration")
    return data


def string_list_value(value):
    """Bound a generic JSON string list, without assigning provider meaning."""
    return (type(value) is list and len(value) <= 256
            and all(type(item) is str and len(item) <= 2048
                    and not any(ord(char) < 32 or ord(char) == 127 for char in item)
                    for item in value))


def prior_request(data):
    """Parse host-designated historical input; do not dereference its old paths.

    Self-consistency is not proof of old validation, execution or nonsecrecy.
    The host must explicitly approve this file for the setup audience.
    """
    try:
        record = decode(data)
        schema = record.get("schema_version") if isinstance(record, dict) else None
        official = type(schema) is int and schema in (2, 3)
        playwright = official and schema == 3
        keys(record, ("schema_version", "kind", "job_id", "validation_id", "workspace", "selection",
                      "configuration_sha256", "server_name", "contents_sha256", "installer_sha256", "runtimes")
                     + (("official_component",) if official else ())
                     + (("playwright_host_profile_sha256",) if playwright else ()))
        selection = record["selection"]
        keys(selection, ("family", "profile", "operations", "values"))
        keys(record["runtimes"], ("python", "node", "npm_cli"))
        if (type(schema) is not int or schema not in (1, 2, 3)
                or record["kind"] != "toolkit.consumer_setup.configured_install_request"
                or (selection["family"] != (PLAYWRIGHT_FAMILY if playwright else OFFICIAL_FAMILY)
                    if official else selection["family"] not in FAMILIES)
                or official and record["official_component"] != ("playwright" if playwright else "github")
                or playwright and not selected_hash(record["playwright_host_profile_sha256"])
                or any(not isinstance(record[key], str) or not REQUEST_ID.fullmatch(record[key]) for key in ("job_id", "validation_id"))
                or any(not isinstance(record[key], str) or not HEX.fullmatch(record[key]) for key in ("configuration_sha256", "contents_sha256", "installer_sha256"))
                or not isinstance(record["workspace"], str) or not 1 <= len(record["workspace"]) <= 4096
                or not Path(record["workspace"]).is_absolute()
                or record["server_name"] != "toolkit-" + selection["family"] + "-" + record["validation_id"][:12]
                or any(value is not None and (not isinstance(value, str) or not 1 <= len(value) <= 4096) for value in record["runtimes"].values())
                or not isinstance(selection["profile"], str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", selection["profile"])):
            raise ValueError()
        operations, values = selection["operations"], selection["values"]
        if (not isinstance(operations, list) or not 1 <= len(operations) <= 128
                or any(not isinstance(op, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", op) for op in operations)
                or len(set(operations)) != len(operations)
                or selection["family"] == "activepieces" and len(operations) != 1
                or not isinstance(values, dict) or len(values) > 256
                or any(not isinstance(key, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", key)
                       or key in {"profile", "operations", "config", "__proto__", "prototype", "constructor"}
                       or type(value) not in (str, int, bool, list)
                       or type(value) is list and not string_list_value(value)
                       or type(value) is str and len(value) > 2048
                       or type(value) is int and abs(value) > 9007199254740991 for key, value in values.items())
                or digest(configuration_bytes(selection)) != record["configuration_sha256"]):
            raise ValueError()
        return record
    except (SetupError, KeyError, TypeError, ValueError):
        pass
    raise SetupError("invalid_prior_request")


def prior_projection(selection, help_value):
    """Current structural projection only; the installed parser owns meaning."""
    fields = {field["name"]: field for field in help_value["fields"]}
    current_operations = {row["id"] for row in help_value["operations"]}
    values, unapplied = {}, []
    for operation in selection["operations"]:
        if operation not in current_operations:
            unapplied.append({"kind": "operation", "name": operation, "reason": "not_in_current_profile"})
    for name, value in selection["values"].items():
        field = fields.get(name)
        kind = {str: "string", int: "integer", bool: "boolean", list: "string_list"}[type(value)]
        reason = None
        if field is None:
            reason = "not_in_current_profile"
        elif kind != field["kind"]:
            reason = "current_type_differs"
        elif kind == "string" and (not value or any(ord(char) < 32 or ord(char) == 127 for char in value)):
            reason = "not_representable_in_current_form"
        elif field["credential_reference"] and not REFERENCE.fullmatch(value):
            reason = "current_reference_name_required"
        elif kind == "integer" and ("minimum" in field and value < field["minimum"] or "maximum" in field and value > field["maximum"]):
            reason = "outside_current_bounds"
        if reason:
            unapplied.append({"kind": "field", "name": name, "prior_type": kind, "reason": reason})
        else:
            values[name] = copy.deepcopy(value)
    return {"values": values, "prior_operations": copy.deepcopy(selection["operations"]), "unapplied": unapplied}


def load_recovery(root, identifier, expected_sha, *, contents_sha, installer_sha, runtimes, families, official_selection=None, playwright_profile_sha=None):
    """Read host-pinned intent only. No receipt or historical outcome is trusted."""
    root = canonical_directory(root)
    if (not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier)
            or not isinstance(expected_sha, str) or not HEX.fullmatch(expected_sha)):
        raise SetupError("invalid_recovery_request")
    raw = read_file(root, "requests/" + identifier + ".json", MAX_BODY * 2)
    if digest(raw) != expected_sha:
        raise SetupError("recovery_digest_mismatch")
    record = decode(raw)
    playwright_record = record.get("schema_version") == 3 if isinstance(record, dict) else False
    official_record = record.get("schema_version") in (2, 3) if isinstance(record, dict) else False
    keys(record, ("schema_version", "kind", "job_id", "validation_id", "workspace", "selection",
                  "configuration_sha256", "server_name", "contents_sha256", "installer_sha256", "runtimes") + (("official_component",) if official_record else ()) + (("playwright_host_profile_sha256",) if playwright_record else ()))
    selection = record["selection"]
    keys(selection, ("family", "profile", "operations", "values"))
    validation_id = record["validation_id"]
    family = selection["family"]
    if official_record:
        if (official_selection is None or record["official_component"] != ("playwright" if playwright_record else "github")
                or family != (PLAYWRIGHT_FAMILY if playwright_record else OFFICIAL_FAMILY)
                or playwright_record and (playwright_profile_sha is None or record["playwright_host_profile_sha256"] != playwright_profile_sha)):
            raise SetupError("recovery_selection_mismatch")
        contents_sha, installer_sha = official_selection
    elif family in OFFICIAL_FAMILIES:
        raise SetupError("recovery_selection_mismatch")
    if (type(record["schema_version"]) is not int or record["schema_version"] != (3 if playwright_record else 2 if official_record else 1)
            or record["kind"] != "toolkit.consumer_setup.configured_install_request"
            or record["job_id"] != identifier or record["workspace"] != str(root)
            or not isinstance(validation_id, str) or not REQUEST_ID.fullmatch(validation_id)
            or not isinstance(family, str) or family not in families
            or not isinstance(selection["profile"], str)
            or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", selection["profile"])
            or not isinstance(selection["operations"], list) or not 1 <= len(selection["operations"]) <= 128
            or any(not isinstance(v, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", v) for v in selection["operations"])
            or len(set(selection["operations"])) != len(selection["operations"])
            or (family == "activepieces" and len(selection["operations"]) != 1)
            or not isinstance(selection["values"], dict) or len(selection["values"]) > 256
            or set(selection["values"]) & {"profile", "operations", "config"}
            or any(not isinstance(k, str) or len(k) > 128 or type(v) not in (str, int, bool, list)
                   or (type(v) is list and not string_list_value(v))
                   or (isinstance(v, str) and len(v) > 2048) for k, v in selection["values"].items())
            or record["contents_sha256"] != contents_sha or record["installer_sha256"] != installer_sha
            or record["runtimes"] != runtimes
            or not isinstance(record["configuration_sha256"], str) or not HEX.fullmatch(record["configuration_sha256"])
            or record["server_name"] != "toolkit-" + family + "-" + validation_id[:12]):
        raise SetupError("recovery_selection_mismatch")
    path = root / "configurations" / (validation_id + ".json")
    canonical_directory(path.parent)
    config_bytes = read_file(root, "configurations/" + path.name, MAX_BODY)
    if digest(config_bytes) != record["configuration_sha256"] or configuration_bytes(selection) != config_bytes:
        raise SetupError("recovery_configuration_mismatch")
    # Destination may be absent or incomplete. Never infer an outcome from it.
    canonical_directory(root / "installations")
    destination = root / "installations" / validation_id
    if destination.is_symlink():
        raise SetupError("invalid_recovery_path")
    return record, {"family": family, "profile": selection["profile"], "destination": destination,
        "configuration": {"path": path, "sha256": record["configuration_sha256"], "server_name": record["server_name"]},
        "recovery_anchor": {"root": root, "id": identifier, "sha256": expected_sha}}


def executable(value: str) -> str:
    path = Path(value)
    if not path.is_absolute():
        raise SetupError("invalid_runtime")
    resolved = path.resolve(strict=True)
    if not resolved.is_file():
        raise SetupError("invalid_runtime")
    return str(resolved)


class Runner:
    """Only trusted argv, minimal environment, and bounded JSON capture."""
    def __init__(self, workspace: Path):
        self.workspace = workspace
        # The Controller binds only its current worker thread to one job.
        # Request handlers never hold a process handle or signal a PID.
        self._job_context = threading.local()
        self._diagnostic_context = threading.local()
        self._compose_context = threading.local()
        self.env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
                    "TMPDIR": str(workspace / "tmp"), "PYTHONNOUSERSITE": "1",
                    "PYTHONDONTWRITEBYTECODE": "1"}

    def run_selected_validator(self, argv: list[str], *, inspection_timeout=None) -> tuple[int, bytes, bytes]:
        """Opt in to bounded diagnostics without changing the run contract."""
        if hasattr(self._diagnostic_context, "stderr"):
            raise SetupError("inspection_unavailable", 500)
        diagnostic = bytearray()
        self._diagnostic_context.stderr = diagnostic
        try:
            # Dispatch through run so a custom override is never bypassed.
            kwargs = {} if inspection_timeout is None else {"inspection_timeout": inspection_timeout}
            code, raw = self.run(argv, capture=True, **kwargs)
            return code, raw, bytes(diagnostic)
        finally:
            del self._diagnostic_context.stderr

    def run_compose(self, argv, *, input_file=None, input_sha256=None,
                    output_limit=8 * 1024 * 1024 + 1, timeout, termination_grace=0):
        """Opt-in regular-file stdin and bounded output under the same runner."""
        if (hasattr(self._compose_context, "settings") or type(timeout) not in (int, float)
                or not 0 < timeout <= 3600 or type(output_limit) is not int
                or not 1 <= output_limit <= 8 * 1024 * 1024 + 1
                or type(termination_grace) not in (int, float) or not 0 <= termination_grace <= 10):
            raise SetupError("compose_runner_unavailable", 500)
        fd = None
        def check_input():
            if fd is None:
                return
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_size > 2 * 1024 * 1024:
                raise SetupError("compose_inputs_changed", 409)
            os.lseek(fd, 0, os.SEEK_SET)
            data = bytearray()
            while True:
                chunk = os.read(fd, min(65536, 2 * 1024 * 1024 + 1 - len(data)))
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > 2 * 1024 * 1024:
                    raise SetupError("compose_inputs_changed", 409)
            if digest(bytes(data)) != input_sha256:
                raise SetupError("compose_inputs_changed", 409)
            os.lseek(fd, 0, os.SEEK_SET)
        try:
            if input_file is not None:
                canonical_directory(input_file.parent)
                fd = os.open(input_file, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                check_input()
            elif input_sha256 is not None:
                raise SetupError("compose_inputs_changed", 409)
            self._compose_context.settings = {"stdin": fd, "output_limit": output_limit,
                                              "termination_grace": termination_grace}
            # Preserve custom run overrides and the existing process/cancel owner.
            result = self.run(argv, capture=True, inspection_timeout=timeout)
            check_input()
            return result
        finally:
            if hasattr(self._compose_context, "settings"):
                del self._compose_context.settings
            if fd is not None:
                os.close(fd)

    def run_compose_diagnostic(self, argv, **kwargs):
        """Capture bounded owner codes without bypassing custom run methods."""
        if hasattr(self._diagnostic_context, "stderr"):
            raise SetupError("compose_runner_unavailable", 500)
        diagnostic = bytearray()
        self._diagnostic_context.stderr = diagnostic
        try:
            code, output = self.run_compose(argv, **kwargs)
            return code, output, bytes(diagnostic)
        finally:
            del self._diagnostic_context.stderr

    def run(self, argv: list[str], *, capture: bool = False,
            inspection_timeout: float | None = None) -> tuple[int, bytes]:
        selected_timeout = getattr(self._job_context, "installer_timeout", None)
        timeout = ((selected_timeout if selected_timeout is not None else INSPECTION_TIMEOUT)
                   if inspection_timeout is None else inspection_timeout)
        compose = getattr(self._compose_context, "settings", None)
        if capture and (type(timeout) not in (int, float) or not 0 < timeout <= (3600 if compose or selected_timeout is not None else MCPB_INSPECTION_TIMEOUT)):
            raise SetupError("inspection_unavailable", 500)
        control = getattr(self._job_context, "control", None)
        diagnostic = getattr(self._diagnostic_context, "stderr", None) if capture else None
        def check_stop():
            if control is not None and control["requested"].is_set():
                raise SetupError("operation_stopped", 409)
        def spawn():
            check_stop()
            return subprocess.Popen(argv, cwd=self.workspace, env=self.env, shell=False,
                                    stdin=compose["stdin"] if compose and compose["stdin"] is not None else subprocess.DEVNULL,
                                    stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
                                    stderr=subprocess.PIPE if diagnostic is not None else subprocess.DEVNULL,
                                    start_new_session=True)
        if control is None:
            process = spawn()
        else:
            # Linearize spawn against explicit stop acceptance. Once stop is
            # accepted, this worker cannot launch another pipeline stage.
            with control["spawn_lock"]:
                process = spawn()
        result = bytearray()
        selector = None
        try:
            if not capture:
                # Browser/MCP disconnect never sets the explicit stop event.
                if control is None:
                    return process.wait(), b""
                while True:
                    check_stop()
                    try:
                        return process.wait(timeout=STOP_CHECK_INTERVAL), b""
                    except subprocess.TimeoutExpired:
                        pass
            deadline = time.monotonic() + timeout
            selector = selectors.DefaultSelector()
            os.set_blocking(process.stdout.fileno(), False)
            selector.register(process.stdout, selectors.EVENT_READ, (result, compose["output_limit"] if compose else MAX_JSON))
            if diagnostic is not None:
                os.set_blocking(process.stderr.fileno(), False)
                selector.register(process.stderr, selectors.EVENT_READ, (diagnostic, SELECTED_DIAGNOSTIC_BYTES))
            while selector.get_map():
                check_stop()
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SetupError("inspection_unavailable", 500)
                for key, _ in selector.select(min(remaining, STOP_CHECK_INTERVAL) if control is not None else remaining):
                    buffer, limit = key.data
                    chunk = os.read(key.fd, min(65536, limit + 1 - len(buffer)))
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    if len(buffer) + len(chunk) > limit:
                        raise SetupError("inspection_unavailable", 500)
                    buffer.extend(chunk)
            while True:
                check_stop()
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SetupError("inspection_unavailable", 500)
                try:
                    code = process.wait(timeout=min(remaining, STOP_CHECK_INTERVAL) if control is not None else remaining)
                    return code, bytes(result)
                except subprocess.TimeoutExpired:
                    continue
        except BaseException as error:
            # Captured commands include metadata observation and explicit
            # recipe retention. Killing their owned session cannot prove an
            # entered mutation did not commit. Callers preserve uncertainty.
            # Only this worker signals its owned, not-yet-reaped process
            # session. A joined child's PID might already identify new work.
            # Direct-child settlement does not attest every descendant exit.
            try:
                if process.returncode is None:
                    grace = (compose.get("termination_grace", 0) if compose else
                             getattr(self._job_context, "termination_grace", 0))
                    if grace:
                        # Pinned Compose and selected installer processes own
                        # separately-sessioned children. Give that owner a
                        # chance to join them. Grace exhaustion leaves inner
                        # settlement unconfirmed; even a graceful exit does
                        # not establish effect state or descendant closure.
                        try:
                            process.send_signal(signal.SIGTERM)
                            process.wait(timeout=grace)
                        except (ProcessLookupError, subprocess.TimeoutExpired):
                            pass
                    try:
                        if process.returncode is None:
                            os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            finally:
                # Even a signal failure cannot release this job's ownership
                # while its direct child remains live.
                process.wait()
            if isinstance(error, SetupError) and error.code == "operation_stopped":
                raise
            raise SetupError("inspection_unavailable", 500)
        finally:
            if selector is not None:
                selector.close()
            if capture:
                process.stdout.close()
            if diagnostic is not None:
                process.stderr.close()


def operation_rows(family: str, source: dict) -> list[dict]:
    raw = source["operations"]
    if not isinstance(raw, list) or not 1 <= len(raw) <= 128:
        raise SetupError("inspection_unavailable", 500)
    published_tools = source.get("operation_tools")
    if "operation_tools" in source:
        if (family == "services" or not isinstance(published_tools, dict)
                or any(not isinstance(operation, str) for operation in raw)
                or set(published_tools) != set(raw)
                or any(not isinstance(tool, str)
                       or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", tool)
                       for tool in published_tools.values())
                or len(set(published_tools.values())) != len(published_tools)):
            raise SetupError("inspection_unavailable", 500)
    rows = []
    for operation in raw:
        if family == "services":
            row = {"id": operation["operation"], "effect": operation["effect"]}
            if "tool" in operation:
                tool = operation["tool"]
                if not isinstance(tool, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", tool):
                    raise SetupError("inspection_unavailable", 500)
                # The publisher owns the callable name; selection still uses id.
                row["tool"] = tool
        else:
            row = {"id": operation, "effect": source.get("effect", "unspecified")}
            if published_tools is not None:
                row["tool"] = published_tools[operation]
        if not isinstance(row["id"], str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", row["id"]):
            raise SetupError("inspection_unavailable", 500)
        if row["effect"] not in ("read", "write", "unspecified"):
            raise SetupError("inspection_unavailable", 500)
        rows.append(row)
    if len({row["id"] for row in rows}) != len(rows):
        raise SetupError("inspection_unavailable", 500)
    tools = [row["tool"] for row in rows if "tool" in row]
    if len(set(tools)) != len(tools):
        raise SetupError("inspection_unavailable", 500)
    return rows


def target_projection(family, source):
    """Carry an installer-owned target without selecting or inferring one."""
    if family != OFFICIAL_FAMILY or "target" not in source:
        return {}
    target = source["target"]
    if not isinstance(target, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", target):
        raise SetupError("invalid_target_projection")
    # Historical macOS help/receipts already carried this value while setup
    # omitted it. Keep those setup response shapes backward compatible.
    return {} if target == "macos-arm64" else {"target": target}


def normalize_help(family: str, profile: str, source: dict, guide: str) -> dict:
    if source["profile"] != profile:
        raise SetupError("inspection_unavailable", 500)
    selectable = source.get("selectable_operations", True)
    if type(selectable) is not bool:
        raise SetupError("inspection_unavailable", 500)
    if family == "services":
        fields = [dict(value, name=name, required=value.get("required", True)) for name, value in source["config_fields"].items()]
    else:
        fields = source["config_fields"] if family == "agno" else source["fields"]
    result = []
    for field in fields:
        if field["name"] in ("profile", "operations", "config"):
            continue
        if field["kind"] not in ("string", "integer", "boolean", "string_list"):
            raise SetupError("inspection_unavailable", 500)
        row = {key: copy.deepcopy(field[key]) for key in (
            "name", "kind", "required", "credential_reference", "minimum", "maximum",
            "only_when_operation_selected") if key in field}
        row.setdefault("credential_reference", False)
        row.setdefault("required", True)
        if (type(row["required"]) is not bool or type(row["credential_reference"]) is not bool
                or row["credential_reference"] and row["kind"] != "string"):
            raise SetupError("inspection_unavailable", 500)
        result.append(row)
    authentication = {}
    if "authentication" in source:
        if (source["authentication"] != {"kind": "none"}
                or any(field["credential_reference"] for field in result)):
            raise SetupError("inspection_unavailable", 500)
        authentication = {"authentication": {"kind": "none"}}
    return {"family": family, "profile": profile, "operations": operation_rows(family, source),
            "selectable_operations": selectable, "fields": result,
            "prerequisites": copy.deepcopy(source["prerequisites"]),
            "unresolved_details": copy.deepcopy(source["unresolved_details"]),
            "guide_text": guide, "scope": source["scope"], **target_projection(family, source), **authentication}


def package_help(family, profile, source, full_manual, full_guide=False):
    """Present package-owned guidance, retaining the complete pinned manual.

    A present selected guide must bind this profile and exact manual. Older
    packages without it keep their full-manual default. No relevance inference
    or provider-specific documentation parsing happens in this owner.
    """
    if type(full_guide) is not bool:
        raise SetupError("invalid_request")
    manual_bytes = full_manual.encode("utf-8")
    manual_digest = digest(manual_bytes)
    selected = source.get("selected_guide")
    if "selected_guide" in source:
        if (type(selected) is not dict or set(selected) != {"profile", "full_guide_sha256", "text"}
                or selected["profile"] != profile or selected["full_guide_sha256"] != manual_digest
                or type(selected["text"]) is not str or not selected["text"].strip()
                  or any(ord(ch) < 32 and ch not in "\n\t\r" for ch in selected["text"])):
            raise SetupError("inspection_unavailable", 500)
        try:
            selected_bytes = selected["text"].encode("utf-8")
        except UnicodeError:
            raise SetupError("inspection_unavailable", 500) from None
        if len(selected_bytes) > SELECTED_GUIDE_BYTES:
            raise SetupError("inspection_unavailable", 500)
    text = full_manual if full_guide or selected is None else selected["text"]
    result = normalize_help(family, profile, source, text)
    result["guide"] = {
        "scope": "full_family_manual" if full_guide or selected is None else "selected_profile",
        "sha256": digest(text.encode("utf-8")), "utf8_bytes": len(text.encode("utf-8")),
        "full_manual_sha256": manual_digest, "full_manual_utf8_bytes": len(manual_bytes),
    }
    return result


def credential_projection(source, selection, help_value):
    """Validate the installed owner's projection; never discover credentials."""
    if (not isinstance(source, dict) or source.get("valid") is not True
            or source.get("profile") != selection["profile"]
            or source.get("scope") != "configuration_only"):
        raise SetupError("invalid_credential_projection")
    if "credential_environment_references" not in source:
        # Older supported validators do not report this field. That is unknown,
        # not a declaration that the configuration needs no credentials.
        return {}
    rows = source["credential_environment_references"]
    if not isinstance(rows, list) or len(rows) > 256:
        raise SetupError("invalid_credential_projection")
    declared = {}
    names = set()
    for field in help_value["fields"]:
        name = field.get("name")
        if (not isinstance(name, str) or not REFERENCE.fullmatch(name) or name in names
                or type(field.get("credential_reference")) is not bool):
            raise SetupError("invalid_credential_projection")
        names.add(name)
        if field["credential_reference"]:
            if field["kind"] != "string":
                raise SetupError("invalid_credential_projection")
            if name in selection["values"]:
                path = (name,) if selection["family"] == "activepieces" else ("config", name)
                declared[path] = selection["values"][name]
    observed = {}
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"field_path", "environment_variable"}:
            raise SetupError("invalid_credential_projection")
        path, variable = row["field_path"], row["environment_variable"]
        if (not isinstance(path, list) or not 1 <= len(path) <= 2
                or any(not isinstance(key, str) or not REFERENCE.fullmatch(key) for key in path)
                or not isinstance(variable, str) or not REFERENCE.fullmatch(variable)):
            raise SetupError("invalid_credential_projection")
        path = tuple(path)
        if path in observed or path not in declared or declared[path] != variable:
            raise SetupError("invalid_credential_projection")
        observed[path] = variable
    if observed != declared:
        raise SetupError("invalid_credential_projection")
    return {"credential_environment_references": copy.deepcopy(rows)}


# Selected software setup is an explicit host-enabled transport to the existing
# canonical validators and installer. No manifest declaration grants authority.
SELECTED_SCOPE = "standing_local_software_installation_only"
SELECTED_PROPOSAL = "selected_software_inputs_only"
SELECTED_REQUEST = "toolkit.consumer_setup.selected_install_request"
SELECTED_DOCUMENT = 1024 * 1024
SELECTED_INLINE_DOCUMENT = 32 * 1024
PACKAGE_INSPECTION_OUTPUT = 256 * 1024
SELECTED_FILE = 64 * 1024 * 1024
SELECTED_TOTAL = 512 * 1024 * 1024
SELECTED_ARGUMENTS = 256 * 1024
SELECTED_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\Z")


def selected_hash(value):
    return isinstance(value, str) and HEX.fullmatch(value) is not None


def selected_name(value):
    return isinstance(value, str) and SELECTED_ID.fullmatch(value) is not None and value not in (".", "..")


def selected_profile(path, expected, *, recovering=False):
    """Read a host-only standing software scope; never accept it in tool JSON."""
    try:
        if not isinstance(path, Path) or not path.is_absolute() or not selected_hash(expected):
            raise ValueError()
        raw = read_file(canonical_directory(path.parent), path.name, MAX_BODY)
        if digest(raw) != expected:
            raise ValueError()
        value = decode(raw)
        if not isinstance(value, dict) or set(value) - {"python_target", "package_inspector", "plugin_inspection", "mcpb_inspection", "node_build"} not in (
                {"schema_version", "scope", "staging_root", "installer", "validators"},
                {"schema_version", "scope", "staging_root", "validators"}):
            raise ValueError()
        if (type(value["schema_version"]) is not int or value["schema_version"] != 1
                or value["scope"] != SELECTED_SCOPE or not isinstance(value["staging_root"], str)
                or not Path(value["staging_root"]).is_absolute()
                or not isinstance(value["validators"], dict) or not value["validators"]
                or not set(value["validators"]) <= {"python", "node", "mcpb"}
                or ("python_target" in value and ("python" not in value["validators"]
                    or value["python_target"] not in ("macos-arm64", "linux-x86_64")))
                or ("plugin_inspection" in value and (type(value["plugin_inspection"]) is not bool
                    or value["plugin_inspection"] and "package_inspector" not in value))
                or ("mcpb_inspection" in value and (type(value["mcpb_inspection"]) is not bool
                    or value["mcpb_inspection"] and ("package_inspector" not in value
                        or "mcpb" not in value["validators"])))
                or ("node_build" in value and ("node" not in value["validators"] or "installer" not in value))
                or ("installer" not in value and set(value["validators"]) != {"mcpb"})):
            raise ValueError()
        if not recovering:
            canonical_directory(Path(value["staging_root"]))
        tools = {**({"installer": value["installer"]} if "installer" in value else {}), **value["validators"],
                 **({"package_inspector": value["package_inspector"]} if "package_inspector" in value else {}),
                 **({"node_build": value["node_build"]} if "node_build" in value else {})}
        payloads = {}
        for name, item in tools.items():
            keys(item, ("path", "sha256"))
            if not isinstance(item["path"], str) or not Path(item["path"]).is_absolute() or not selected_hash(item["sha256"]):
                raise ValueError()
            source = Path(item["path"])
            data = read_file(canonical_directory(source.parent), source.name, 65536 if name == "node_build" else SELECTED_FILE)
            if digest(data) != item["sha256"]:
                raise ValueError()
            if name not in ("installer", "node_build") and not os.access(source, os.X_OK):
                raise ValueError()
            payloads[name] = data
        return value, raw, payloads
    except (SetupError, OSError, TypeError, ValueError, RecursionError):
        pass
    raise SetupError("invalid_selected_host_profile")


LOCAL_SOFTWARE_DOCUMENT = "local_software_declaration"
SELECTED_IDENTITY_FIELDS = {"document_kind", "declaration_digest", "manifest_digest"}


def selected_document(request):
    """Select a fixed document from the explicit request, never its contents."""
    if "document_kind" not in request:
        return "--selection", "activation.json", "manifest_digest"
    if request["document_kind"] != LOCAL_SOFTWARE_DOCUMENT or request.get("profile") not in ("python", "node"):
        raise SetupError("invalid_selected_request")
    return "--declaration", "declaration.json", "declaration_digest"


def selected_identity(value, request):
    """Require the canonical owner's exclusive identity for this input form."""
    _, _, pin = selected_document(request)
    expected = {pin: value.get(pin)} if isinstance(value, dict) else {}
    if pin == "declaration_digest":
        expected["document_kind"] = LOCAL_SOFTWARE_DOCUMENT
    if (not isinstance(value, dict) or not selected_hash(value.get(pin))
            or {key: value[key] for key in SELECTED_IDENTITY_FIELDS if key in value} != expected):
        raise SetupError("selected_validation_unavailable")
    return expected


def selected_source_request(value):
    fields = ("profile", "staged_id", "package_sha256", "selection_sha256", "component")
    keys(value, (*fields, *(("configuration_sha256",) if isinstance(value, dict) and value.get("profile") == "mcpb" else ()),
                 *(("plugin_source",) if isinstance(value, dict) and "plugin_source" in value else ()),
                 *(("acquired_source",) if isinstance(value, dict) and "acquired_source" in value else ()),
                 *(("document_kind",) if isinstance(value, dict) and "document_kind" in value else ()),
                 *(("build_plan_sha256",) if isinstance(value, dict) and "build_plan_sha256" in value else ())))
    selected_document(value)
    if (value["profile"] not in ("python", "node", "mcpb") or not selected_name(value["staged_id"])
            or not selected_name(value["component"]) or not selected_hash(value["package_sha256"])
            or not selected_hash(value["selection_sha256"])
            or (value["profile"] == "mcpb" and not selected_hash(value["configuration_sha256"]))):
        raise SetupError("invalid_selected_request")
    if "build_plan_sha256" in value and (value["profile"] != "node" or not selected_hash(value["build_plan_sha256"])):
        raise SetupError("invalid_selected_request")
    if "plugin_source" in value:
        plugin_source_reference(value["plugin_source"])
    if "acquired_source" in value:
        acquired_source_reference(value["acquired_source"])
        if value["profile"] not in ("python", "node") or value["staged_id"] != value["acquired_source"]["job_id"]:
            raise SetupError("invalid_selected_request")


def acquired_source_reference(value):
    keys(value, ("job_id", "receipt_sha256"))
    if (type(value["job_id"]) is not str or not REQUEST_ID.fullmatch(value["job_id"])
            or not selected_hash(value["receipt_sha256"])):
        raise SetupError("invalid_selected_request")


def plugin_source_reference(value):
    keys(value, ("inspection_id", "inspection_sha256"))
    if (not isinstance(value["inspection_id"], str) or not REQUEST_ID.fullmatch(value["inspection_id"])
            or not selected_hash(value["inspection_sha256"])):
        raise SetupError("invalid_plugin_source")


def selected_inline_document(value):
    """Bound exact UTF-8 bytes; the canonical owner parses their meaning."""
    try:
        if type(value) is str:
            raw = value.encode("utf-8")
            if 1 <= len(raw) <= SELECTED_INLINE_DOCUMENT:
                return raw
    except UnicodeError:
        pass
    raise SetupError("invalid_selected_request")


def selected_check_request(value):
    """Normalize inline input to the unchanged retained hash-only request."""
    if not isinstance(value, dict) or not {"package_json", "activation_json", "declaration_json", "configuration_json"}.intersection(value):
        if isinstance(value, dict) and "acquired_source" in value:
            raise SetupError("invalid_selected_request")
        selected_source_request(value)
        return copy.deepcopy(value), None
    software = "declaration_json" in value
    if software and value.get("profile") not in ("python", "node"):
        raise SetupError("invalid_selected_request")
    fields = {"package_json": ("package.json", "package_sha256"),
              ("declaration_json" if software else "activation_json"):
                  ("declaration.json" if software else "activation.json", "selection_sha256")}
    if value.get("profile") == "mcpb":
        fields["configuration_json"] = ("configuration.json", "configuration_sha256")
    if "build_plan_json" in value:
        if value.get("profile") != "node":
            raise SetupError("invalid_selected_request")
        fields["build_plan_json"] = ("closure/node-build-plan", "build_plan_sha256")
    keys(value, ("profile", "staged_id", "component", *fields,
                 *(("plugin_source",) if "plugin_source" in value else ()),
                 *(("acquired_source",) if "acquired_source" in value else ())))
    documents = {filename: selected_inline_document(value[key]) for key, (filename, _) in fields.items()}
    request = {key: value[key] for key in ("profile", "staged_id", "component")}
    if software:
        request["document_kind"] = LOCAL_SOFTWARE_DOCUMENT
    request.update({hash_key: digest(documents[filename]) for filename, hash_key in fields.values()})
    if "plugin_source" in value:
        request["plugin_source"] = copy.deepcopy(value["plugin_source"])
    if "acquired_source" in value:
        request["acquired_source"] = copy.deepcopy(value["acquired_source"])
    selected_source_request(request)
    if len(encoded(value)) > MAX_BODY:
        raise SetupError("invalid_selected_request")
    return request, documents


def materialize_acquired_source(raw, component_id, result, build_plan_sha256=None):
    """Copy only retained artifact facts; the pinned validator owns semantics."""
    value = decode(raw)
    if not isinstance(value, dict) or type(value.get("components")) is not list:
        raise SetupError("invalid_selected_request")
    matches = [component for component in value["components"]
               if isinstance(component, dict) and component.get("id") == component_id]
    if len(matches) != 1:
        raise SetupError("invalid_selected_request")
    source = matches[0].get("source")
    keys(source, ("kind", "entrypoint"))
    if source["kind"] != "artifact":
        raise SetupError("invalid_selected_request")
    source.update(location=result["primary"]["url"], content_digest=result["primary"]["sha256"],
                  dependencies={"kind": "pinned_closure", "artifacts": {
                      name.removeprefix("closure/"): sha for name, sha in result["files"].items()
                      if name.startswith("closure/")}})
    if build_plan_sha256 is not None:
        if not selected_hash(build_plan_sha256) or "closure/node-build-plan" in result["files"]:
            raise SetupError("invalid_selected_request")
        # The authored plan is separate from the unmodified acquisition result.
        source["dependencies"]["artifacts"]["node-build-plan"] = build_plan_sha256
    expanded = encoded(value)
    if len(expanded) > SELECTED_DOCUMENT:
        raise SetupError("invalid_selected_request")
    return expanded


PLUGIN_SOURCE_SCOPE = "observed_public_plugin_files"
PLUGIN_FILE_COUNT = 16
PLUGIN_FILES_TOTAL = 128 * 1024
PLUGIN_INVENTORY_COUNT = 256
PLUGIN_INVENTORY_PATH_BYTES = 1024
PLUGIN_INVENTORY_TOTAL_PATH_BYTES = 64 * 1024


def plugin_files_request(value):
    keys(value, ("staged_id", "files"))
    if (not selected_name(value["staged_id"]) or type(value["files"]) is not list
            or not 1 <= len(value["files"]) <= PLUGIN_FILE_COUNT
            or any(type(path) is not str or not 3 <= len(path) <= 1024 for path in value["files"])
            or len(set(value["files"])) != len(value["files"])
            or value["files"].count("./plugin.json") != 1):
        raise SetupError("invalid_plugin_request")
    # This only bounds the request. The canonical anchored reader owns paths,
    # file selection, portable metadata parsing and required contract versions.
    if len(encoded(value)) > MAX_BODY:
        raise SetupError("invalid_plugin_request")


def plugin_inventory_path(value, max_depth):
    """Bound portable names in the envelope; the canonical reader owns traversal."""
    if (type(value) is not str or not value.startswith("./") or ":" in value or "\\" in value
            or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value)):
        return False
    try:
        if len(value.encode("utf-8")) > PLUGIN_INVENTORY_PATH_BYTES:
            return False
    except UnicodeError:
        return False
    parts = [] if value == "./" else value[2:].split("/")
    return len(parts) <= max_depth and all(part not in ("", ".", "..") for part in parts)


def plugin_inventory_request(value):
    keys(value, ("staged_id", "inventory"))
    if (not selected_name(value["staged_id"]) or not plugin_inventory_path(value["inventory"], 15)
            or len(encoded(value)) > MAX_BODY):
        raise SetupError("invalid_plugin_request")


def plugin_files_output(raw, code, files):
    """Check the pinned reader's evidence envelope, without parsing plugin meaning."""
    try:
        if type(code) is not int or type(raw) is not bytes or len(raw) > PACKAGE_INSPECTION_OUTPUT:
            raise ValueError()
        value = decode(raw)
        if code == 2:
            keys(value, ("schema_version", "scope", "status", "error"))
            error = value["error"]
            keys(error, ("kind", "path", "message"))
            if (type(value["schema_version"]) is int and value["schema_version"] == 1
                    and value["scope"] == "metadata_only" and value["status"] == "invalid"
                    and error["kind"] == "invalid_metadata"
                    and all(type(error[key]) is str for key in ("path", "message"))):
                raise SetupError("plugin_metadata_rejected")
            raise ValueError()
        keys(value, ("schema_version", "scope", "format", "status", "source_digest", "files"))
        rows = value["files"]
        if (code != 0 or type(value["schema_version"]) is not int or value["schema_version"] != 1
                or value["scope"] != "metadata_only" or value["format"] != "agent-plugin-files"
                or value["status"] not in ("accepted", "accepted_with_diagnostics")
                or not selected_hash(value["source_digest"]) or type(rows) is not list
                or len(rows) != len(files)):
            raise ValueError()
        total, paths, diagnostics = 0, [], False
        for row in rows:
            keys(row, ("path", "text", "byte_length", "source_digest", "inspection"))
            if (type(row["path"]) is not str or row["path"] not in files or type(row["text"]) is not str
                    or type(row["byte_length"]) is not int):
                raise ValueError()
            data = row["text"].encode("utf-8")
            total += len(data)
            paths.append(row["path"])
            if (len(data) > SELECTED_INLINE_DOCUMENT or row["byte_length"] != len(data)
                    or row["source_digest"] != digest(data)
                    or any((ord(c) < 32 and c not in "\t\r\n") or 127 <= ord(c) <= 159 for c in row["text"])):
                raise ValueError()
            report = row["inspection"]
            expected = {"./plugin.json": "agent-plugin", "./mcp.json": "mcp-servers"}.get(row["path"])
            if expected is None:
                if report is not None:
                    raise ValueError()
            else:
                keys(report, ("schema_version", "scope", "format", "status", "source_digest", "summary", "diagnostics"))
                if (type(report["schema_version"]) is not int or report["schema_version"] != 1
                        or report["scope"] != "metadata_only" or report["format"] != expected
                        or report["source_digest"] != row["source_digest"] or type(report["summary"]) is not dict
                        or type(report["diagnostics"]) is not list):
                    raise ValueError()
                for item in report["diagnostics"]:
                    keys(item, ("scope", "message"))
                    if any(type(item[key]) is not str for key in item):
                        raise ValueError()
                observed = bool(report["diagnostics"])
                if report["status"] != ("accepted_with_diagnostics" if observed else "accepted"):
                    raise ValueError()
                diagnostics |= observed
        canonical = json.dumps(rows, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")
        if (len(set(paths)) != len(files) or total > PLUGIN_FILES_TOTAL
                or value["source_digest"] != digest(canonical)
                or value["status"] != ("accepted_with_diagnostics" if diagnostics else "accepted")):
            raise ValueError()
        return value
    except SetupError as error:
        if error.code == "plugin_metadata_rejected":
            raise
    except (TypeError, ValueError, KeyError, RecursionError, UnicodeError):
        pass
    raise SetupError("plugin_inspection_unavailable")


def plugin_inventory_output(raw, code, directory):
    """Accept only a complete, content-free inventory of the requested directory."""
    try:
        if type(code) is not int or type(raw) is not bytes or len(raw) > PACKAGE_INSPECTION_OUTPUT:
            raise ValueError()
        # Inventory never parses file contents. A rejection, including an older
        # inspector without this variant, says nothing about package validity.
        if code != 0:
            raise ValueError()
        value = decode(raw)
        keys(value, ("schema_version", "scope", "format", "status", "directory", "inventory", "complete"))
        rows = value["inventory"]
        if (code != 0 or type(value["schema_version"]) is not int or value["schema_version"] != 1
                or value["scope"] != "metadata_only" or value["format"] != "agent-plugin-files"
                or value["status"] != "inventoried" or value["directory"] != directory
                or value["complete"] is not True or type(rows) is not list
                or len(rows) > PLUGIN_INVENTORY_COUNT):
            raise ValueError()
        prefix = directory if directory == "./" else directory + "/"
        paths = []
        for row in rows:
            if type(row) is not dict or row.get("kind") not in ("file", "directory", "symlink", "special"):
                raise ValueError()
            keys(row, ("path", "kind", "byte_length") if row["kind"] == "file" else ("path", "kind"))
            path = row["path"]
            if (not plugin_inventory_path(path, 16) or not path.startswith(prefix)
                    or not path[len(prefix):] or "/" in path[len(prefix):]):
                raise ValueError()
            if row["kind"] == "file" and (type(row["byte_length"]) is not int
                    or not 0 <= row["byte_length"] <= 2**64 - 1):
                raise ValueError()
            paths.append(path.encode("utf-8"))
        if (paths != sorted(set(paths)) or sum(map(len, paths)) > PLUGIN_INVENTORY_TOTAL_PATH_BYTES):
            raise ValueError()
        return value
    except SetupError:
        pass
    except (TypeError, ValueError, KeyError, RecursionError, UnicodeError):
        pass
    raise SetupError("plugin_inspection_unavailable")


def mcpb_files_request(request):
    keys(request, ("staged_id", "artifact_sha256", "files"))
    files = request["files"]
    if (not selected_name(request["staged_id"]) or not selected_hash(request["artifact_sha256"])
            or type(files) is not list or len(files) > 16):
        raise SetupError("invalid_mcpb_inspection_request")
    for path in files:
        if (type(path) is not str or not path.startswith("./") or not 3 <= len(path) <= 1024
                or any(part in ("", ".", "..") for part in path[2:].split("/"))
                or "\\" in path or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in path)):
            raise SetupError("invalid_mcpb_inspection_request")
    if len(set(files)) != len(files):
        raise SetupError("invalid_mcpb_inspection_request")
    normalized = ["./manifest.json", *(path for path in files if path != "./manifest.json")]
    if len(normalized) > 16:
        raise SetupError("invalid_mcpb_inspection_request")
    return normalized


def mcpb_files_output(raw, code, artifact_sha256, files):
    """Validate the canonical reader's envelope, not the MCPB's semantics."""
    try:
        if type(code) is not int or type(raw) is not bytes or len(raw) > PACKAGE_INSPECTION_OUTPUT:
            raise ValueError()
        value = decode(raw)
        if code == 2:
            keys(value, ("schema_version", "scope", "status", "error"))
            error = value["error"]
            keys(error, ("kind", "path", "message"))
            if (type(value["schema_version"]) is int and value["schema_version"] == 1
                    and value["scope"] == "metadata_only" and value["status"] == "invalid"
                    and error["kind"] == "invalid_metadata"
                    and all(type(error[key]) is str for key in ("path", "message"))):
                raise SetupError("mcpb_metadata_rejected")
            raise ValueError()
        keys(value, ("schema_version", "scope", "format", "status", "artifact_sha256",
            "artifact_byte_length", "source_digest", "archive_entry_count", "inventory_entry_count", "inventory",
            "inventory_truncated", "files"))
        rows, inventory, count = value["files"], value["inventory"], value["inventory_entry_count"]
        if (code != 0 or type(value["schema_version"]) is not int or value["schema_version"] != 1
                or value["scope"] != "metadata_only" or value["format"] != "mcpb-archive"
                or value["artifact_sha256"] != artifact_sha256
                or type(value["artifact_byte_length"]) is not int
                or not 1 <= value["artifact_byte_length"] <= SELECTED_FILE
                or type(count) is not int or not 1 <= count <= 10000
                or type(value["archive_entry_count"]) is not int
                or not 1 <= value["archive_entry_count"] <= count
                or type(inventory) is not list or len(inventory) != min(count, 128)
                or type(value["inventory_truncated"]) is not bool
                or value["inventory_truncated"] != (count > 128)
                or type(rows) is not list or len(rows) != len(files)):
            raise ValueError()
        names = []
        for item in inventory:
            keys(item, ("path", "byte_length", "directory"))
            if (type(item["path"]) is not str or not item["path"].startswith("./")
                    or not 3 <= len(item["path"]) <= 1026
                    or any(part in ("", ".", "..") for part in item["path"][2:].split("/"))
                    or "\\" in item["path"]
                    or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in item["path"])
                    or type(item["byte_length"]) is not int
                    or not 0 <= item["byte_length"] <= SELECTED_FILE
                    or type(item["directory"]) is not bool
                    or item["directory"] and item["byte_length"] != 0):
                raise ValueError()
            names.append(item["path"])
        if names != sorted(set(names)):
            raise ValueError()
        total, diagnostics = 0, False
        for path, row in zip(files, rows):
            keys(row, ("path", "text", "byte_length", "source_digest", "inspection"))
            if row["path"] != path or type(row["text"]) is not str or type(row["byte_length"]) is not int:
                raise ValueError()
            data = row["text"].encode("utf-8")
            total += len(data)
            if (len(data) > SELECTED_INLINE_DOCUMENT or row["byte_length"] != len(data)
                    or row["source_digest"] != digest(data)
                    or any((ord(c) < 32 and c not in "\t\r\n") or 127 <= ord(c) <= 159 for c in row["text"])):
                raise ValueError()
            report = row["inspection"]
            if path != "./manifest.json":
                if report is not None:
                    raise ValueError()
                continue
            keys(report, ("schema_version", "scope", "format", "status", "source_digest", "summary", "diagnostics"))
            if (type(report["schema_version"]) is not int or report["schema_version"] != 1
                    or report["scope"] != "metadata_only" or report["format"] != "mcpb"
                    or report["source_digest"] != row["source_digest"] or type(report["summary"]) is not dict
                    or type(report["diagnostics"]) is not list):
                raise ValueError()
            declarations = report["summary"].get("declarations")
            keys(declarations, ("runtime", "entry_point", "command", "args"))
            if (any(type(declarations[key]) is not str for key in ("runtime", "entry_point", "command"))
                    or type(declarations["args"]) is not list
                    or any(type(argument) is not str for argument in declarations["args"])):
                raise ValueError()
            for diagnostic in report["diagnostics"]:
                keys(diagnostic, ("scope", "message"))
                if any(type(diagnostic[key]) is not str for key in diagnostic):
                    raise ValueError()
            diagnostics = bool(report["diagnostics"])
            if report["status"] != ("accepted_with_diagnostics" if diagnostics else "accepted"):
                raise ValueError()
        canonical = json.dumps(rows, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")
        if (total > PLUGIN_FILES_TOTAL or value["source_digest"] != digest(canonical)
                or value["status"] != ("accepted_with_diagnostics" if diagnostics else "accepted")):
            raise ValueError()
        return value
    except SetupError as error:
        if error.code == "mcpb_metadata_rejected":
            raise
    except (TypeError, ValueError, KeyError, RecursionError, UnicodeError):
        pass
    raise SetupError("mcpb_inspection_unavailable")


# Output vocabulary only. This filters paths reported by the canonical owner;
# it never reads submitted metadata or independently validates package fields.
PACKAGE_DIAGNOSTIC_PATH = {
    "schema_version": None, "display_name": None, "description": None,
    "identity": {"publisher": None, "name": None, "version": None},
    "provenance": {"source": None, "source_revision": None, "license": None,
                   "notices": [None], "local_patches": [None]},
    "strategies": [{"id": None, "kind": None, "instructions": None,
                    "sources": [None], "unresolved_details": [None],
                    "known_requirements": [{"name": None,
                        "version": {"major": None, "minimum_minor": None},
                        "authority": [None], "availability": {"kind": None, "degraded_behavior": None}}]}],
    "known_exports": [{"id": None, "role": None, "description": None}],
    "assessments": [{"status": None, "scope": None, "source_revision": None,
                     "assessed_at_unix_ms": None,
                     "evidence": [{"owner": None, "record": None, "revision": None}]}],
}


def package_diagnostic(value, source_digest):
    keys(value, ("version", "category", "field_path", "source_digest"))
    if (type(value["version"]) is not int or value["version"] != 1
            or type(value["category"]) is not str or value["category"] not in ("data", "syntax", "eof")
            or not selected_hash(value["source_digest"]) or value["source_digest"] != source_digest
            or type(value["field_path"]) is not list or len(value["field_path"]) > 16):
        raise ValueError()
    node = PACKAGE_DIAGNOSTIC_PATH
    for segment in value["field_path"]:
        if type(node) is dict and type(segment) is str and segment in node:
            node = node[segment]
        elif type(node) is list and type(segment) is int and 0 <= segment <= 1024 * 1024:
            node = node[0]
        else:
            raise ValueError()
    return copy.deepcopy(value)


def package_inspection(raw, source_digest, code):
    """Project only the pinned inspector's current metadata report contract."""
    try:
        if type(code) is not int or type(raw) is not bytes or len(raw) > PACKAGE_INSPECTION_OUTPUT:
            raise ValueError()
        value = decode(raw)
        if code == 2:
            keys(value, ("schema_version", "scope", "status", "error"))
            error = value["error"]
            detailed = type(error) is dict and "diagnostic" in error
            keys(error, ("kind", "path", "message", "diagnostic") if detailed else ("kind", "path", "message"))
            if (type(value["schema_version"]) is int and value["schema_version"] == 1
                    and value["scope"] == "metadata_only" and value["status"] == "invalid"
                    and error["kind"] == "invalid_metadata"
                    and all(type(error[key]) is str for key in ("path", "message"))):
                refusal = SetupError("selected_invalid_package")
                if detailed:
                    if error["path"] != "package" or error["message"] != "invalid integration package":
                        raise ValueError()
                    refusal.package_diagnostic = package_diagnostic(error["diagnostic"], source_digest)
                raise refusal
            raise ValueError()
        keys(value, ("schema_version", "scope", "format", "status", "source_digest", "summary", "diagnostics"))
        summary = value["summary"]
        keys(summary, ("identity", "package_content_digest", "strategies", "exports"))
        keys(summary["identity"], ("publisher", "name", "version"))
        def text_value(item):
            return type(item) is str and 1 <= len(item) <= 256 and not any(ord(c) < 32 or ord(c) == 127 for c in item)
        if (code != 0 or type(value["schema_version"]) is not int or value["schema_version"] != 1
                or value["scope"] != "metadata_only" or value["format"] != "integration-package"
                or value["status"] != "accepted" or value["source_digest"] != source_digest
                or value["diagnostics"] != [] or not selected_hash(summary["package_content_digest"])
                or not all(text_value(item) for item in summary["identity"].values())):
            raise ValueError()
        for field, fields in (("strategies", ("id", "kind")), ("exports", ("id", "role"))):
            if type(summary[field]) is not list:
                raise ValueError()
            for row in summary[field]:
                keys(row, fields)
                if not all(text_value(item) for item in row.values()):
                    raise ValueError()
        return value
    except SetupError as error:
        if error.code == "selected_invalid_package":
            raise
    except (TypeError, ValueError, KeyError, RecursionError):
        pass
    raise SetupError("package_inspection_unavailable")


def acquired_inspection_output(raw, code, request):
    if type(code) is not int or type(raw) is not bytes or len(raw) > PACKAGE_INSPECTION_OUTPUT:
        raise ValueError()
    value = decode(raw)
    if code == 2 and value == {"error": "acquired_member_unreadable"}:
        raise SetupError("acquired_member_unreadable")
    scope = ("acquired_primary_archive_only" if request.get("archive_role", "primary") == "primary"
             else "acquired_dependency_archive_only")
    common = {"schema_version": 1, "scope": scope, "profile": request["profile"],
              "filename": request["filename"], "archive_sha256": request["archive_sha256"]}
    listing = "after" in request["selection"]
    fields = ("members", "next_after") if listing else ("member", "member_bytes", "member_sha256", "offset", "text", "next_offset")
    keys(value, (*common, *fields))
    if code or type(value["schema_version"]) is not int or any(value[k] != v for k, v in common.items()):
        raise ValueError()
    if listing:
        rows = value["members"]
        if type(rows) is not list or len(rows) > 64:
            raise ValueError()
        names = []
        for row in rows:
            keys(row, ("path", "bytes", "sha256"))
            if (type(row["path"]) is not str or not 1 <= len(row["path"]) <= 1024
                    or type(row["bytes"]) is not int or not 0 <= row["bytes"] <= SELECTED_FILE
                    or not selected_hash(row["sha256"])):
                raise ValueError()
            names.append(row["path"])
        after = request["selection"]["after"]
        if (names != sorted(set(names)) or after is not None and any(n <= after for n in names)
                or value["next_after"] is not None and (not names or value["next_after"] != names[-1])):
            raise ValueError()
    else:
        if (value["member"] != request["selection"]["member"] or type(value["offset"]) is not int
                or value["offset"] != request["selection"]["offset"] or type(value["text"]) is not str
                or len(value["text"].encode("utf-8")) > SELECTED_INLINE_DOCUMENT
                or not selected_hash(value["member_sha256"]) or type(value["member_bytes"]) is not int
                or not 0 <= value["member_bytes"] <= SELECTED_DOCUMENT):
            raise ValueError()
        end = value["offset"] + len(value["text"].encode("utf-8"))
        if (end > value["member_bytes"] or (value["next_offset"] is not None and type(value["next_offset"]) is not int)
                or value["next_offset"] != (end if end < value["member_bytes"] else None)
                or any((ord(c) < 32 and c not in "\t\r\n") or 127 <= ord(c) <= 159 for c in value["text"])):
            raise ValueError()
    return value


def selected_service_declaration(value):
    """Bound one canonical manifest projection; this is not runtime evidence."""
    optional = ("environment_secret_slots",) if "environment_secret_slots" in value else ()
    keys(value, ("authentication", "operation_tools", *optional))
    authentication = value["authentication"]
    if authentication != {"kind": "none"}:
        keys(authentication, ("kind", "owner", "requirement", "credential_audience"))
        if authentication["kind"] != "binding" or any(type(authentication[k]) is not str
                or not 1 <= len(authentication[k]) <= 192 or any(ord(c) < 32 for c in authentication[k])
                for k in ("owner", "requirement", "credential_audience")):
            raise ValueError()
    if optional:
        slots = value["environment_secret_slots"]
        if (authentication.get("kind") != "binding" or type(slots) is not list
                or not 1 <= len(slots) <= 256
                or any(type(name) is not str or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", name) for name in slots)
                or slots != sorted(set(slots))):
            raise ValueError()
    names = value["operation_tools"]
    if (type(names) is not dict or not 1 <= len(names) <= 128
            or any(type(k) is not str or not 1 <= len(k) <= 192 or any(ord(c) < 32 for c in k)
                   or type(v) is not str or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", v) for k, v in names.items())
            or len(set(names.values())) != len(names)):
        raise ValueError()


def remote_service_declaration(value, request, selection_digest, mcp=None):
    """Check a pinned owner's projection against its exact retained input.

    Rust owns recipe, declaration and activation semantics. These equality checks neither
    infer raw names nor establish authentication, observed tools or readiness.
    Legacy omission remains exportable without public activation evidence.
    """
    selection_kind = remote_export_kind(value, request)
    if "declared_remote_service" not in value:
        return selection_kind
    declared = value["declared_remote_service"]
    pin = "declaration_digest" if selection_kind else "manifest_digest"
    keys(declared, (pin, "component", "export", "mcp_name", "authentication", "operation_tools", *selection_kind))
    if (any(declared[k] != v for k, v in selection_kind.items())
            or declared[pin] != selection_digest or not selected_hash(selection_digest)
            or any(type(declared[k]) is not str or not 1 <= len(declared[k]) <= 192
                   or any(ord(c) < 32 or ord(c) == 127 for c in declared[k])
                   for k in ("component", "export", "mcp_name"))):
        raise ValueError()
    selected_service_declaration({k: declared[k] for k in ("authentication", "operation_tools")})
    document_key, _, _ = remote_export_document(request)
    activation = decode(request[document_key].encode("utf-8"))
    if (not isinstance(activation, dict)
            or (selection_kind and activation.get("selection_kind") != "remote_declaration")
            or type(activation.get("components")) is not list
            or len(activation["components"]) != 1 or type(activation.get("exports")) is not list
            or len(activation["exports"]) != 1 or type(activation.get("portable_mcp_entries")) is not list
            or len(activation["portable_mcp_entries"]) != 1):
        raise ValueError()
    component, service, portable = (activation["components"][0], activation["exports"][0],
                                    activation["portable_mcp_entries"][0])
    if (not all(isinstance(item, dict) for item in (component, service, portable))
            or component.get("id") != declared["component"]
            or service.get("id") != declared["export"]
            or service.get("authentication") != declared["authentication"]
            or portable.get("name") != declared["mcp_name"]
            or portable.get("component") != declared["component"]
            or portable.get("operation_tools") != declared["operation_tools"]
            or mcp is not None and set(mcp["mcpServers"]) != {declared["mcp_name"]}):
        raise ValueError()
    return {**selection_kind, "declared_remote_service": copy.deepcopy(declared)}


def selected_plan(raw, request):
    """Check a pinned owner's bounded output shape, not package semantics."""
    try:
        plan = decode(raw)
        mcpb = request["profile"] == "mcpb"
        identity = selected_identity(plan, request)
        common = ("schema_version", "package", "package_content_digest", *identity, "component",
                  "artifact_digest", "entrypoint", "mcp_name")
        fields = (("mcpb_manifest_digest", "configuration_digest", "prepared_evidence_digest", "minimum_node_version")
                  if mcpb else ("strategy", "export", "closure"))
        optional = ("declared_service",) if not mcpb and "declared_service" in plan else ()
        keys(plan, (*common, *fields, *(("artifact_filename", "module") if request["profile"] == "python" else ()), *optional))
        if optional:
            selected_service_declaration(plan["declared_service"])
        keys(plan["package"], ("publisher", "name", "version"))
        console_fields = (("python_console_script",) if request["profile"] == "python"
                          and "python_console_script" in plan["entrypoint"] else ())
        keys(plan["entrypoint"], ("executable", "arguments", *console_fields))
        if console_fields:
            script = plan["entrypoint"]["python_console_script"]
            if (type(script) is not str or len(script) > SELECTED_DOCUMENT
                    or not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", script)):
                raise ValueError()
        if (type(plan["schema_version"]) is not int or plan["schema_version"] != 1
                or plan["component"] != request["component"]
                or any(not isinstance(x, str) or not 1 <= len(x) <= 256 or any(ord(c) < 32 for c in x)
                       for x in [*plan["package"].values(), plan["component"], plan["mcp_name"],
                                 *([] if mcpb else [plan["strategy"], plan["export"]])])
                or any(not selected_hash(plan[k]) for k in ("package_content_digest", "artifact_digest"))
                or not isinstance(plan["entrypoint"]["executable"], str)
                or len(plan["entrypoint"]["executable"]) > 8192
                or not isinstance(plan["entrypoint"]["arguments"], list)
                or len(plan["entrypoint"]["arguments"]) > 256
                or len(encoded(plan["entrypoint"]["arguments"])) > SELECTED_ARGUMENTS
                or any(not isinstance(arg, str) or len(arg) > 8192 for arg in plan["entrypoint"]["arguments"])):
            raise ValueError()
        if mcpb:
            if (any(not selected_hash(plan[k]) for k in ("mcpb_manifest_digest", "configuration_digest", "prepared_evidence_digest"))
                    or plan["configuration_digest"] != request["configuration_sha256"]
                    or not isinstance(plan["minimum_node_version"], list) or len(plan["minimum_node_version"]) != 3
                    or any(type(v) is not int or not 0 <= v <= 4294967295 for v in plan["minimum_node_version"])):
                raise ValueError()
            # Only bound the path used in the launch projection. The canonical
            # MCPB owner validates archive contents, script type and substitutions.
            relative(plan["entrypoint"]["executable"])
        elif (not isinstance(plan["closure"], dict) or not 1 <= len(plan["closure"]) <= 256
                or any(len(relative(k)) != 1 or not selected_hash(v) for k, v in plan["closure"].items())):
            raise ValueError()
        if request["profile"] == "python" and (not isinstance(plan["module"], str) or not isinstance(plan["artifact_filename"], str)):
            raise ValueError()
        return plan
    except (SetupError, KeyError, TypeError, ValueError, RecursionError):
        pass
    raise SetupError("selected_validation_unavailable")


def selected_files(plan, request):
    if request["profile"] == "node" and plan["closure"].get("node-build-plan") != request.get("build_plan_sha256"):
        raise SetupError("selected_validation_unavailable")
    if request["profile"] == "mcpb":
        return {"package.json": request["package_sha256"], "activation.json": request["selection_sha256"],
                "configuration.json": request["configuration_sha256"], "artifact.mcpb": plan["artifact_digest"]}
    return {"package.json": request["package_sha256"], selected_document(request)[1]: request["selection_sha256"],
            ("artifact.whl" if request["profile"] == "python" else "artifact.tgz"): plan["artifact_digest"],
            **{"closure/" + name: sha for name, sha in plan["closure"].items()}}


def selected_read(root, name, maximum=MAX_JSON):
    """Use the existing no-follow reader with a fixed selected-setup error."""
    try:
        return read_file(root, name, maximum)
    except (OSError, SetupError):
        pass
    raise SetupError("selected_input_unavailable", 409)


def selected_proposal(root, identifier, expected, host_binding):
    """Read only fixed host-owned proposal locations, including during recovery."""
    if not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier) or not selected_hash(expected):
        raise SetupError("selected_proposal_changed", 409)
    path = root / "selections" / identifier
    raw = selected_read(root, "selections/" + identifier + "/effect.json", MAX_JSON)
    if digest(raw) != expected:
        raise SetupError("selected_proposal_changed", 409)
    effect = decode(raw)
    keys(effect, ("schema_version", "kind", "request", "host", "destination", "files", "plan_sha256",
                  *(("build_review",) if isinstance(effect, dict) and "build_review" in effect else ())))
    if (type(effect["schema_version"]) is not int or effect["schema_version"] != 1
            or effect["kind"] != SELECTED_PROPOSAL or effect["host"] != host_binding
            or effect["destination"] != str(root / "installations" / identifier)):
        raise SetupError("selected_proposal_changed", 409)
    selected_source_request(effect["request"])
    plan_raw = selected_read(root, "selections/" + identifier + "/plan.json", MAX_JSON)
    if digest(plan_raw) != effect["plan_sha256"]:
        raise SetupError("selected_proposal_changed", 409)
    plan = selected_plan(plan_raw, effect["request"])
    if effect["files"] != selected_files(plan, effect["request"]):
        raise SetupError("selected_proposal_changed", 409)
    total = 0
    for name, sha in effect["files"].items():
        data = selected_read(root, "selections/" + identifier + "/" + name, SELECTED_DOCUMENT if name in ("package.json", "activation.json", "declaration.json", "configuration.json") else SELECTED_FILE)
        total += len(data)
        if total > SELECTED_TOTAL or digest(data) != sha:
            raise SetupError("selected_proposal_changed", 409)
    if "build_plan_sha256" in effect["request"]:
        review_raw = selected_read(path, "build-review.json", MAX_JSON)
        if encoded(decode(review_raw)) != encoded(effect.get("build_review")):
            raise SetupError("selected_proposal_changed", 409)
        selected_build_review(effect["build_review"], effect["request"], host_binding)
    elif "build_review" in effect:
        raise SetupError("selected_proposal_changed", 409)
    if "plugin_source" in effect["request"]:
        source = selected_read(path, "plugin-source.json", MAX_JSON)
        if digest(source) != effect["request"]["plugin_source"]["inspection_sha256"]:
            raise SetupError("selected_proposal_changed", 409)
    return path, effect, plan


def selected_build_review(review, request, host_binding):
    if (not isinstance(review, dict) or type(review.get("schema_version")) is not int
            or review["schema_version"] != 1 or review.get("scope") != "selected_node_build"
            or review.get("plan_sha256") != request.get("build_plan_sha256")
            or review.get("profile_sha256") != host_binding["tools"].get("node_build")
            or not isinstance(review.get("limits"), dict)
            or type(review["limits"].get("timeout_seconds")) is not int
            or not 1 <= review["limits"]["timeout_seconds"] <= 3600
            or len(encoded(review)) > MAX_JSON):
        raise SetupError("selected_build_review_unavailable", 409)
    return review


def load_selected_recovery(root, identifier, expected, host_binding):
    if not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier) or not selected_hash(expected):
        raise SetupError("invalid_recovery_startup")
    root = canonical_directory(root)
    raw = selected_read(root, "requests/" + identifier + ".json", MAX_BODY * 2)
    if digest(raw) != expected:
        raise SetupError("recovery_request_changed")
    record = decode(raw)
    if not isinstance(record, dict) or record.get("kind") != SELECTED_REQUEST:
        return None
    keys(record, ("schema_version", "kind", "job_id", "validation_id", "workspace", "effect_sha256")
         + (("retry_of",) if "retry_of" in record else ()))
    if (type(record["schema_version"]) is not int or record["schema_version"] != 1
            or record["job_id"] != identifier or record["workspace"] != str(root)
            or "retry_of" in record and (not isinstance(record["retry_of"], str)
                or not REQUEST_ID.fullmatch(record["retry_of"]) or record["retry_of"] == identifier)):
        raise SetupError("invalid_recovery_request")
    _, effect, _ = selected_proposal(root, record["validation_id"], record["effect_sha256"], host_binding)
    destination = root / "installations" / record["validation_id"]
    canonical_directory(destination.parent)
    if destination.is_symlink():
        raise SetupError("invalid_recovery_path")
    family = "selected-" + effect["request"]["profile"]
    return {**record, "selection": {"family": family}}, {
        "kind": "selected", "family": family, "destination": destination,
        "root": root, "validation_id": record["validation_id"], "effect_sha256": record["effect_sha256"],
        "recovery_anchor": {"root": root, "id": identifier, "sha256": expected}}


def load_acquisition_recovery(root, identifier, expected, *, discover=False):
    """Load an exact host-selected request, without interpreting output as success."""
    try:
        if (root is None or not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier)
                or not selected_hash(expected)):
            raise SetupError("invalid_recovery_startup")
        root = canonical_directory(root)
        raw = selected_read(root, "requests/" + identifier + ".json", MAX_BODY * 2)
        record = decode(raw)
    except (SetupError, OSError, TypeError, ValueError):
        if discover:
            return None
        raise
    if not isinstance(record, dict) or record.get("kind") != ACQUISITION_REQUEST:
        return None
    if digest(raw) != expected:
        raise SetupError("recovery_request_changed")
    try:
        keys(record, ("schema_version", "kind", "job_id", "family", "workspace", "staging_root", "arguments", "host"))
        kind = record["family"].removeprefix("selected-")
        if (type(record["schema_version"]) is not int or record["schema_version"] != 1
                or kind not in ACQUISITION_ASSETS or record["family"] != "selected-" + kind
                or record["job_id"] != identifier or record["workspace"] != str(root)
                or not isinstance(record["staging_root"], str)
                or not Path(record["staging_root"]).is_absolute()
                or str(Path(record["staging_root"])) != record["staging_root"]
                or ".." in Path(record["staging_root"]).parts
                or not isinstance(record["arguments"], dict)
                or not isinstance(record["arguments"].get("request_id"), str)
                or not REQUEST_ID.fullmatch(record["arguments"]["request_id"])):
            raise ValueError()
        staging = Path(record["staging_root"])
        if root == staging or root in staging.parents or staging in root.parents:
            raise ValueError()
        host = record["host"]
        keys(host, ("profile_sha256", "assets", "selected_profile_sha256"))
        if (not selected_hash(host["profile_sha256"]) or not isinstance(host["assets"], dict)
                or set(host["assets"]) != ACQUISITION_ASSETS[kind]
                or not all(selected_hash(value) for value in host["assets"].values())
                or host["assets"]["profile.json"] != host["profile_sha256"]
                or (host["selected_profile_sha256"] is not None if kind == "compose"
                    else not selected_hash(host["selected_profile_sha256"]))):
            raise ValueError()
    except (SetupError, TypeError, ValueError, KeyError, AttributeError):
        raise SetupError("invalid_recovery_request") from None
    return record, {"kind": "acquire", "family": record["family"], "root": root,
                    "recovery_anchor": {"root": root, "id": identifier, "sha256": expected}}


def acquisition_module(kind):
    # The current setup implementation owns readback. Never import retained code.
    if kind not in ACQUISITION_ASSETS:
        raise SetupError("invalid_recovery_request")
    module_name = kind.replace("-", "_") + "_acquisition"
    spec = importlib.util.spec_from_file_location("consumer_setup_" + module_name,
        Path(__file__).with_name(module_name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def acquisition_io():
    return SimpleNamespace(Error=SetupError, read_file=read_file, write_new=write_new,
                           canonical_directory=canonical_directory, decode=decode, keys=keys)


def check_recovery_profile(path, expected, original):
    """An optional current profile may confirm an anchor, never replace it."""
    if path is None and expected is None:
        return
    if (not isinstance(path, Path) or not path.is_absolute() or not selected_hash(expected)
            or expected != original or digest(read_file(canonical_directory(path.parent), path.name, MAX_JSON)) != expected):
        raise SetupError("recovery_profile_changed")


def load_recovery_selection(path, expected):
    """Read a host-pinned list of existing requests, never discover prior work."""
    if not isinstance(path, Path) or not path.is_absolute() or not selected_hash(expected):
        raise SetupError("invalid_recovery_manifest")
    raw = read_file(canonical_directory(path.parent), path.name, MAX_BODY)
    if digest(raw) != expected:
        raise SetupError("recovery_manifest_changed")
    root, entries = read_recovery_selection(raw)
    return raw, root, entries


def read_recovery_selection(raw):
    """Validate manifest bytes and their exact request anchors, without effects."""
    if not isinstance(raw, bytes) or len(raw) > MAX_BODY:
        raise SetupError("invalid_recovery_manifest")
    value = decode(raw)
    keys(value, ("schema_version", "workspace", "requests"))
    if (type(value["schema_version"]) is not int or value["schema_version"] != 1
            or not isinstance(value["workspace"], str) or not Path(value["workspace"]).is_absolute()
            or not isinstance(value["requests"], list) or not 1 <= len(value["requests"]) <= 256):
        raise SetupError("invalid_recovery_manifest")
    # Bound and validate the complete list before reading any request payload.
    identifiers = set()
    for entry in value["requests"]:
        keys(entry, ("id", "sha256"))
        if (not isinstance(entry["id"], str) or not REQUEST_ID.fullmatch(entry["id"])
                or entry["id"] in identifiers or not selected_hash(entry["sha256"])):
            raise SetupError("invalid_recovery_manifest")
        identifiers.add(entry["id"])
    root = canonical_directory(Path(value["workspace"]))
    if str(root) != value["workspace"]:
        raise SetupError("invalid_recovery_manifest")
    entries = []
    for entry in value["requests"]:
        data = read_file(root, "requests/" + entry["id"] + ".json", MAX_BODY * 2)
        if digest(data) != entry["sha256"]:
            raise SetupError("recovery_request_changed")
        record = decode(data)
        kind = record.get("kind") if isinstance(record, dict) else None
        if kind not in (ACQUISITION_REQUEST, SELECTED_REQUEST, REMOTE_EXPORT_REQUEST,
                        "toolkit.consumer_setup.configured_install_request"):
            raise SetupError("invalid_recovery_request")
        if record.get("job_id") != entry["id"] or record.get("workspace") != str(root):
            raise SetupError("invalid_recovery_request")
        entries.append({**entry, "kind": kind})
    return root, entries


def retained_request_summary(root, entry):
    """Names from one checkpoint-selected request, not an installation outcome."""
    raw = read_file(root, "requests/" + entry["id"] + ".json", MAX_BODY * 2)
    if digest(raw) != entry["sha256"]:
        raise SetupError("recovery_request_changed")
    record = decode(raw)
    if record.get("job_id") != entry["id"] or record.get("workspace") != str(root):
        raise SetupError("invalid_recovery_request")
    if record.get("kind") not in (SELECTED_REQUEST, "toolkit.consumer_setup.configured_install_request"):
        return None
    validation = record.get("validation_id")
    if not isinstance(validation, str) or not REQUEST_ID.fullmatch(validation):
        raise SetupError("invalid_recovery_request")
    if record["kind"] == SELECTED_REQUEST:
        effect_raw = selected_read(root, "selections/" + validation + "/effect.json", MAX_JSON)
        if digest(effect_raw) != record.get("effect_sha256"):
            raise SetupError("selected_proposal_changed")
        profile = decode(effect_raw).get("request", {}).get("profile")
        if profile not in ("python", "node"):
            return None
        family = "selected-" + profile
    else:
        selection = record.get("selection", {})
        family, profile = selection.get("family"), selection.get("profile")
        if family not in ("agno", "services", "activepieces"):
            return None
        if not isinstance(profile, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", profile):
            raise SetupError("invalid_recovery_request")
    return {"family": family, "profile": profile, "validation_id": validation}


def load_request_recovery(root, identifier, expected, *, acquisition=None, kind=None,
                          recipes_sha=None, selected_binding=None, verifier=False, **library):
    """Dispatch to existing intent owners; legacy selection order stays intact."""
    if acquisition is not None:
        return acquisition
    if kind == ACQUISITION_REQUEST:
        result = load_acquisition_recovery(root, identifier, expected)
    elif kind == REMOTE_EXPORT_REQUEST or kind is None and recipes_sha is not None:
        if recipes_sha is None:
            raise SetupError("invalid_local_recipes_profile")
        result = load_export_recovery(root, identifier, expected, recipes_sha)
        if result is None:
            raise SetupError("invalid_local_recipes_profile")
    elif kind == SELECTED_REQUEST or kind is None and selected_binding is not None:
        if selected_binding is None:
            raise SetupError("invalid_selected_host_profile")
        result = load_selected_recovery(root, identifier, expected, selected_binding)
    else:
        result = None
    if result is None:
        if not verifier:
            raise SetupError("invalid_recovery_startup")
        result = load_recovery(root, identifier, expected, **library)
    return result


class Controller:
    def __init__(self, bundle: Path, contents_sha256: str, workspace: Path,
                 families: tuple[str, ...], python: str, node: str | None,
                 npm_cli: str | None, runner_factory=Runner,
                 verifier: Path | None = None, verifier_sha256: str | None = None,
                 recover_workspace: Path | None = None, recover_request: str | None = None,
                 recover_request_sha256: str | None = None,
                 official_bundle: Path | None = None, official_contents_sha256: str | None = None,
                 official_installer: Path | None = None, official_installer_sha256: str | None = None,
                 playwright_host_profile: Path | None = None, playwright_host_profile_sha256: str | None = None,
                 prior_request_path: Path | None = None, prior_request_sha256: str | None = None,
                 selected_install_profile: Path | None = None, selected_install_profile_sha256: str | None = None,
                 local_recipes_profile: Path | None = None, local_recipes_profile_sha256: str | None = None,
                 allow_remote_recipes: bool = False,
                 compose_profile: Path | None = None, compose_profile_sha256: str | None = None,
                 python_acquisition_profile: Path | None = None,
                 python_acquisition_profile_sha256: str | None = None,
                 node_acquisition_profile: Path | None = None,
                 node_acquisition_profile_sha256: str | None = None,
                 node_prebuilt_profile: Path | None = None,
                 node_prebuilt_profile_sha256: str | None = None,
                 compose_acquisition_profile: Path | None = None,
                 compose_acquisition_profile_sha256: str | None = None,
                 mcpb_acquisition_profile: Path | None = None,
                 mcpb_acquisition_profile_sha256: str | None = None,
                 recover_manifest: Path | None = None, recover_manifest_sha256: str | None = None,
                 selected_node: str | None = None, selected_npm_cli: str | None = None,
                 allow_acquired_inspection: bool = False, allow_node_search: bool = False,
                 allow_public_github_research: bool = False,
                 recovery_selection_writer=None, *, _retained_selections=(), _local_read_only=False):
        if recovery_selection_writer is not None and not callable(recovery_selection_writer):
            raise SetupError("invalid_recovery_selection_writer")
        self.recovery_selection_writer = recovery_selection_writer
        # Only the local process owner supplies these exact prior selections.
        # Each stays a legacy single-workspace checkpoint. They authorize
        # readback and duplicate correlation, never historical setup effects.
        if (type(_retained_selections) is not tuple or type(_local_read_only) is not bool
                or _local_read_only and not _retained_selections
                or _retained_selections and any(value is not None for value in
                    (recover_workspace, recover_request, recover_request_sha256,
                     recover_manifest, recover_manifest_sha256, prior_request_path, prior_request_sha256))):
            raise SetupError("invalid_local_history")
        retained_entries, retained_ids = [], set()
        for selection in _retained_selections:
            if type(selection) is not bytes:
                raise SetupError("invalid_local_history")
            selected_root, entries = read_recovery_selection(selection)
            for entry in entries:
                if entry["id"] in retained_ids or len(retained_ids) >= 256:
                    raise SetupError("invalid_local_history")
                retained_ids.add(entry["id"])
                retained_entries.append((selected_root, entry))
        if selected_node is None and selected_npm_cli is not None:
            raise SetupError("invalid_runtime")
        recovery_manifest_raw, recovery_entries = None, None
        if recover_manifest is not None or recover_manifest_sha256 is not None:
            if any(value is not None for value in (recover_workspace, recover_request, recover_request_sha256,
                                                   prior_request_path, prior_request_sha256)):
                raise SetupError("invalid_recovery_startup")
            recovery_manifest_raw, recover_workspace, recovery_entries = load_recovery_selection(
                recover_manifest, recover_manifest_sha256)
        recovering = recover_workspace is not None or recover_request is not None or recover_request_sha256 is not None
        if (type(allow_public_github_research) is not bool
                or allow_public_github_research and (recovering or _local_read_only)):
            raise SetupError("invalid_github_research_scope")
        if type(allow_node_search) is not bool or allow_node_search and (recovering or node_acquisition_profile is None):
            raise SetupError("invalid_node_search_scope")
        if type(allow_acquired_inspection) is not bool:
            raise SetupError("invalid_acquired_inspection_scope")
        if (not families and selected_install_profile is None and local_recipes_profile is None and compose_profile is None and compose_acquisition_profile is None and recover_workspace is None) or len(set(families)) != len(families) or not set(families) <= {*FAMILIES, *OFFICIAL_FAMILIES}:
            raise SetupError("invalid_startup")
        if not workspace.is_absolute() or workspace.exists() or workspace.is_symlink():
            raise SetupError("workspace_must_be_new")
        parent = workspace.parent.resolve(strict=True)
        if not parent.is_dir():
            raise SetupError("invalid_workspace")
        workspace = parent / workspace.name
        acquisition_recovery = None
        acquisition_recoveries = {}
        if recovery_entries is not None:
            for entry in recovery_entries:
                if entry["kind"] == ACQUISITION_REQUEST:
                    acquisition_recoveries[entry["id"]] = load_acquisition_recovery(
                        recover_workspace, entry["id"], entry["sha256"])
        elif recovering:
            acquisition_recovery = load_acquisition_recovery(recover_workspace, recover_request, recover_request_sha256, discover=True)
            if acquisition_recovery is not None:
                acquisition_recoveries[recover_request] = acquisition_recovery
        for selected_root, entry in retained_entries:
            if entry["kind"] == ACQUISITION_REQUEST:
                acquisition_recoveries[entry["id"]] = load_acquisition_recovery(
                    selected_root, entry["id"], entry["sha256"])
        if _local_read_only and not any(record["family"] in
                ("selected-python", "selected-node", "selected-mcpb")
                for record, _ in acquisition_recoveries.values()):
            # A saved all-capabilities descriptor may have completed only a
            # library install. It has no retained acquired artifact to inspect.
            allow_acquired_inspection = False
        inspection_recovery = (recovering or _local_read_only) and allow_acquired_inspection
        acquisition_only = (bool(acquisition_recoveries) and
            ((not retained_entries and (recovery_entries is None
                or len(acquisition_recoveries) == len(recovery_entries)))
             or _local_read_only and len(acquisition_recoveries) == len(retained_entries)))
        acquisition_modules = {}
        acquisition_profiles = {"python": (python_acquisition_profile, python_acquisition_profile_sha256),
                                "node": (node_acquisition_profile, node_acquisition_profile_sha256),
                                "node-prebuilt": (node_prebuilt_profile, node_prebuilt_profile_sha256),
                                "compose": (compose_acquisition_profile, compose_acquisition_profile_sha256),
                                "mcpb": (mcpb_acquisition_profile, mcpb_acquisition_profile_sha256)}
        acquisition_identities = {}
        for record, _ in acquisition_recoveries.values():
            staging = Path(record["staging_root"])
            if workspace == staging or staging in workspace.parents or workspace in staging.parents:
                raise SetupError("invalid_recovery_path")
            kind = record["family"].removeprefix("selected-")
            identity = (record["staging_root"], record["host"])
            if kind in acquisition_identities and acquisition_identities[kind] != identity:
                raise SetupError("recovery_profile_changed")
            acquisition_identities[kind] = identity
            check_recovery_profile(*acquisition_profiles[kind], record["host"]["profile_sha256"])
            if kind != "compose" or recovery_entries is None:
                check_recovery_profile(selected_install_profile, selected_install_profile_sha256,
                                       record["host"]["selected_profile_sha256"])
            acquisition_modules[kind] = acquisition_module(kind)
        if recovering:
            for kind, (path, expected) in acquisition_profiles.items():
                if (path is not None or expected is not None) and kind not in acquisition_identities:
                    raise SetupError("recovery_profile_changed")
        compose = None
        if compose_profile is not None or compose_profile_sha256 is not None:
            if any(x is not None for x in (recover_workspace, recover_request, recover_request_sha256)):
                raise SetupError("invalid_compose_recovery")
            spec = importlib.util.spec_from_file_location("consumer_setup_compose_bridge", Path(__file__).with_name("compose_bridge.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            io = SimpleNamespace(Error=SetupError, read_file=read_file, write_new=write_new,
                                 canonical_directory=canonical_directory, decode=decode, keys=keys)
            compose = module.ComposeBridge(io, compose_profile, compose_profile_sha256, workspace)
        self.python = executable(python)
        self.node = executable(node) if node and not acquisition_only else None
        self.npm_cli = executable(npm_cli) if npm_cli and not acquisition_only else None
        # Omission preserves the original shared runtime contract. A Node-only
        # override is permitted only for an MCPB profile, checked below.
        self.selected_node = (executable(selected_node) if not acquisition_only else None) if selected_node is not None else self.node
        self.selected_npm_cli = ((executable(selected_npm_cli) if not acquisition_only else None)
                                 if selected_npm_cli is not None else
                                 (None if selected_node is not None else self.npm_cli))
        if ("activepieces" in families or PLAYWRIGHT_FAMILY in families) and not (self.node and self.npm_cli):
            raise SetupError("invalid_runtime")
        recipes = recipes_raw = recipes_tool = None
        if local_recipes_profile is not None or local_recipes_profile_sha256 is not None:
            recipes, recipes_raw, recipes_tool = load_local_recipes_profile(
                local_recipes_profile, local_recipes_profile_sha256)
        if type(allow_remote_recipes) is not bool or (allow_remote_recipes and (recipes is None or recipes["scope"] != LOCAL_RECIPES_RETAIN_SCOPE)):
            raise SetupError("invalid_remote_recipes_profile")
        selected = selected_raw = selected_payloads = selected_binding = None
        if (not acquisition_only or inspection_recovery) and (selected_install_profile is not None or selected_install_profile_sha256 is not None):
            selected, selected_raw, selected_payloads = selected_profile(selected_install_profile,
                selected_install_profile_sha256, recovering=recover_workspace is not None or _local_read_only)
            if not acquisition_only and "node" in selected["validators"] and not (self.selected_node and self.selected_npm_cli):
                raise SetupError("invalid_runtime")
            if not acquisition_only and "mcpb" in selected["validators"] and not self.selected_node:
                raise SetupError("invalid_runtime")
            staging = Path(selected["staging_root"])
            if workspace == staging or workspace in staging.parents or staging in workspace.parents:
                raise SetupError("invalid_selected_staging_root")
            selected_binding = {"profile_sha256": selected_install_profile_sha256,
                "tools": {name: digest(data) for name, data in selected_payloads.items()},
                "runtimes": {"python": self.python, "node": self.selected_node, "npm_cli": self.selected_npm_cli}}
        self.acquired_inspection_raw = None
        self.acquired_inspection_python_raw = None
        self.allow_acquired_inspection = allow_acquired_inspection
        if self.allow_acquired_inspection:
            package_reads = (any(kind in acquisition_identities for kind in ("python", "node")) if recovering or _local_read_only
                             else any(x is not None for x in (python_acquisition_profile, node_acquisition_profile)))
            mcpb_reads = ("mcpb" in acquisition_identities if recovering or _local_read_only else mcpb_acquisition_profile is not None)
            if (selected is None or not (package_reads or mcpb_reads)
                    or package_reads and "installer" not in selected
                    or mcpb_reads and selected.get("mcpb_inspection") is not True):
                raise SetupError("invalid_acquired_inspection_scope")
            if package_reads:
                self.acquired_inspection_raw = read_file(Path(__file__).parent, "acquired_inspection.py", SELECTED_DOCUMENT)
            if inspection_recovery and package_reads:
                # Current explicit host inputs authorize this read-only worker.
                # Never revive an interpreter or installer from retained assets.
                runtime = Path(self.python)
                self.acquired_inspection_python_raw = selected_read(runtime.parent, runtime.name, SELECTED_FILE)
        if (selected_node is not None and not acquisition_only
                and not allow_node_search and (selected is None or not {"node", "mcpb"}.intersection(selected["validators"]))):
            raise SetupError("invalid_runtime")
        if (selected_node is not None and selected_npm_cli is None and not allow_node_search
                and (selected is None or "mcpb" not in selected["validators"]
                     or "node" in selected["validators"])):
            raise SetupError("invalid_runtime")
        acquisition = None
        if not (recovering or _local_read_only) and (python_acquisition_profile is not None or python_acquisition_profile_sha256 is not None):
            if (selected is None or "python" not in selected["validators"]
                    or any(x is not None for x in (recover_workspace, recover_request, recover_request_sha256))):
                raise SetupError("invalid_python_acquisition_profile")
            spec = importlib.util.spec_from_file_location("consumer_setup_python_acquisition",
                Path(__file__).with_name("python_acquisition.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            acquisition_modules["python"] = module
            io = SimpleNamespace(Error=SetupError, read_file=read_file, write_new=write_new,
                                 canonical_directory=canonical_directory, decode=decode, keys=keys)
            acquisition = module.PythonAcquisition(io, python_acquisition_profile,
                python_acquisition_profile_sha256, self.python, Path(selected["staging_root"]),
                Path(selected["installer"]["path"]), selected["installer"]["sha256"])
            if acquisition.target != selected.get("python_target", "macos-arm64"):
                raise SetupError("python_target_mismatch")
        node_acquisition = None
        node_search = None
        if not (recovering or _local_read_only) and (node_acquisition_profile is not None or node_acquisition_profile_sha256 is not None):
            spec = importlib.util.spec_from_file_location("consumer_setup_node_acquisition",
                Path(__file__).with_name("node_acquisition.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            acquisition_modules["node"] = module
            io = SimpleNamespace(Error=SetupError, read_file=read_file, write_new=write_new,
                                 canonical_directory=canonical_directory, decode=decode, keys=keys)
            owner = module.NodeAcquisition(io, node_acquisition_profile,
                node_acquisition_profile_sha256, self.python, Path(selected["staging_root"]) if selected else None,
                Path(selected["installer"]["path"]) if selected and "installer" in selected else None,
                selected["installer"]["sha256"] if selected and "installer" in selected else None, self.selected_node)
            if owner.allow_acquisition:
                if selected is None or "node" not in selected["validators"]:
                    raise SetupError("invalid_node_acquisition_profile")
                node_acquisition = owner
            elif not allow_node_search:
                raise SetupError("invalid_node_search_scope")
            if allow_node_search:
                node_search = owner
        node_prebuilt_acquisition = None
        if not (recovering or _local_read_only) and (node_prebuilt_profile is not None or node_prebuilt_profile_sha256 is not None):
            if node_acquisition is None or selected is None or "node" not in selected["validators"]:
                raise SetupError("invalid_node_prebuilt_acquisition_profile")
            module = acquisition_module("node-prebuilt")
            acquisition_modules["node-prebuilt"] = module
            node_prebuilt_acquisition = module.NodePrebuiltAcquisition(node_prebuilt_profile,
                node_prebuilt_profile_sha256, Path(selected["staging_root"]), io=acquisition_io(),
                node_owner=node_acquisition, source_reader=self._node_prebuilt_source,
                selected_node=self.selected_node)
        mcpb_acquisition = None
        if not (recovering or _local_read_only) and (mcpb_acquisition_profile is not None or mcpb_acquisition_profile_sha256 is not None):
            if selected is None or selected.get("mcpb_inspection") is not True or "mcpb" not in selected["validators"]:
                raise SetupError("invalid_mcpb_acquisition_profile")
            module = acquisition_module("mcpb")
            acquisition_modules["mcpb"] = module
            mcpb_acquisition = module.McpbAcquisition(mcpb_acquisition_profile,
                mcpb_acquisition_profile_sha256, Path(selected["staging_root"]), io=acquisition_io())
            if str(mcpb_acquisition.python) != self.python:
                raise SetupError("invalid_mcpb_acquisition_profile")
        compose_acquisition = None
        if not (recovering or _local_read_only) and (compose_acquisition_profile is not None or compose_acquisition_profile_sha256 is not None):
            if any(x is not None for x in (recover_workspace, recover_request, recover_request_sha256)):
                raise SetupError("invalid_compose_acquisition_profile")
            spec = importlib.util.spec_from_file_location("consumer_setup_compose_acquisition",
                Path(__file__).with_name("compose_acquisition.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            acquisition_modules["compose"] = module
            io = SimpleNamespace(Error=SetupError, read_file=read_file, write_new=write_new,
                                 canonical_directory=canonical_directory, decode=decode, keys=keys)
            compose_acquisition = module.ComposeAcquisition(io, compose_acquisition_profile,
                compose_acquisition_profile_sha256, self.python)
        # A separate verifier is an explicit host choice, never a fallback to
        # another executable or an agent-provided path. Freeze it before use.
        verifier_bytes = None
        if verifier is not None or verifier_sha256 is not None:
            if (verifier is None or not verifier.is_absolute() or not isinstance(verifier_sha256, str)
                    or not HEX.fullmatch(verifier_sha256)):
                raise SetupError("invalid_verifier")
            verifier_bytes = read_file(verifier.parent.resolve(strict=True), verifier.name)
            if digest(verifier_bytes) != verifier_sha256:
                raise SetupError("verifier_digest_mismatch")
        official = None
        if any(v is not None for v in (official_bundle, official_contents_sha256, official_installer, official_installer_sha256)):
            if (official_bundle is None or official_installer is None or not official_bundle.is_absolute() or not official_installer.is_absolute()
                    or not isinstance(official_contents_sha256, str) or not HEX.fullmatch(official_contents_sha256)
                    or not isinstance(official_installer_sha256, str) or not HEX.fullmatch(official_installer_sha256)):
                raise SetupError("invalid_official_selection")
            official_root = canonical_directory(official_bundle)
            official_raw = read_file(official_root, "CONTENTS.json")
            official_code = read_file(canonical_directory(official_installer.parent), official_installer.name)
            if digest(official_raw) != official_contents_sha256 or digest(official_code) != official_installer_sha256:
                raise SetupError("official_digest_mismatch")
            official_manifest = decode(official_raw)
            if (not isinstance(official_manifest, dict) or type(official_manifest.get("format")) is not int
                    or official_manifest["format"] != 1 or not isinstance(official_manifest.get("files"), dict)
                    or not 1 <= len(official_manifest["files"]) <= 512):
                raise SetupError("invalid_official_selection")
            official = (official_root, official_raw, official_code, official_manifest["files"])
        if bool(set(OFFICIAL_FAMILIES) & set(families)) != (official is not None):
            raise SetupError("invalid_official_selection")
        profile_raw = None
        if PLAYWRIGHT_FAMILY in families:
            if (playwright_host_profile is None or not playwright_host_profile.is_absolute()
                    or not isinstance(playwright_host_profile_sha256, str) or not HEX.fullmatch(playwright_host_profile_sha256)):
                raise SetupError("invalid_playwright_host_profile")
            profile_raw = read_file(canonical_directory(playwright_host_profile.parent), playwright_host_profile.name, MAX_BODY)
            if digest(profile_raw) != playwright_host_profile_sha256:
                raise SetupError("playwright_host_profile_changed")
            profile = decode(profile_raw)
            keys(profile, ("schema_version", "chrome_executable", "chrome_sha256", "process_parent"))
            if type(profile["schema_version"]) is not int or profile["schema_version"] != 1:
                raise SetupError("invalid_playwright_host_profile")
        elif playwright_host_profile is not None or playwright_host_profile_sha256 is not None:
            raise SetupError("invalid_playwright_host_profile")
        library_families = [f for f in families if f not in OFFICIAL_FAMILIES]
        if bundle is None:
            if contents_sha256 is not None or library_families:
                raise SetupError("invalid_bundle")
            raw, files = None, {}
        else:
            if not isinstance(contents_sha256, str) or not HEX.fullmatch(contents_sha256):
                raise SetupError("invalid_bundle")
            bundle = bundle.resolve(strict=True)
            raw = read_file(bundle, "CONTENTS.json", 1024 * 1024)
            if digest(raw) != contents_sha256:
                raise SetupError("bundle_digest_mismatch")
            manifest = decode(raw)
            files = manifest.get("files")
            if not isinstance(files, dict) or not 1 <= len(files) <= 512 or "install-library-consumer.py" not in files:
                raise SetupError("invalid_bundle")
            for family in library_families:
                if family + "/PROFILE-GUIDE.md" not in files:
                    raise SetupError("invalid_bundle")
        recoveries = []
        if (any(value is not None for value in (prior_request_path, prior_request_sha256))
                and any(value is not None for value in (recover_workspace, recover_request, recover_request_sha256))):
            raise SetupError("invalid_prior_selection")
        if recovering:
            if recovery_entries is None and (recover_workspace is None or recover_request is None or recover_request_sha256 is None):
                raise SetupError("invalid_recovery_startup")
            entries = recovery_entries if recovery_entries is not None else [
                {"id": recover_request, "sha256": recover_request_sha256, "kind": None}]
            for entry in entries:
                recovery = load_request_recovery(recover_workspace, entry["id"], entry["sha256"],
                    acquisition=acquisition_recoveries.get(entry["id"]), kind=entry["kind"],
                    recipes_sha=local_recipes_profile_sha256 if recipes is not None else None,
                    selected_binding=selected_binding, verifier=verifier_bytes is not None,
                    contents_sha=contents_sha256, installer_sha=files.get("install-library-consumer.py", {}).get("sha256"),
                    runtimes={"python": self.python, "node": self.node, "npm_cli": self.npm_cli}, families=families,
                    official_selection=(official_contents_sha256, official_installer_sha256) if official else None,
                    playwright_profile_sha=playwright_host_profile_sha256)
                recoveries.append((*recovery, entry["sha256"]))
            if (workspace == recover_workspace or recover_workspace in workspace.parents
                    or recovery_entries is not None and workspace in recover_workspace.parents):
                raise SetupError("invalid_recovery_path")
        for selected_root, entry in retained_entries:
            if (workspace == selected_root or selected_root in workspace.parents
                    or workspace in selected_root.parents):
                raise SetupError("invalid_recovery_path")
            recovery = load_request_recovery(selected_root, entry["id"], entry["sha256"],
                acquisition=acquisition_recoveries.get(entry["id"]), kind=entry["kind"],
                recipes_sha=local_recipes_profile_sha256 if recipes is not None else None,
                selected_binding=selected_binding, verifier=verifier_bytes is not None,
                contents_sha=contents_sha256, installer_sha=files.get("install-library-consumer.py", {}).get("sha256"),
                runtimes={"python": self.python, "node": self.node, "npm_cli": self.npm_cli}, families=families,
                official_selection=(official_contents_sha256, official_installer_sha256) if official else None,
                playwright_profile_sha=playwright_host_profile_sha256)
            recoveries.append((*recovery, entry["sha256"]))
        # Re-read the same selections before any new workspace publication.
        # Current host profiles checked above do not change any old anchor.
        for selection in _retained_selections:
            read_recovery_selection(selection)
        prior_raw = prior_record = None
        if prior_request_path is not None or prior_request_sha256 is not None:
            if (recoveries or prior_request_path is None or not prior_request_path.is_absolute()
                    or not isinstance(prior_request_sha256, str) or not HEX.fullmatch(prior_request_sha256)):
                raise SetupError("invalid_prior_selection")
            prior_raw = read_file(canonical_directory(prior_request_path.parent), prior_request_path.name, MAX_BODY * 2)
            if digest(prior_raw) != prior_request_sha256:
                raise SetupError("prior_request_changed")
            prior_record = prior_request(prior_raw)
        if recovery_entries is not None:
            if read_file(canonical_directory(recover_manifest.parent), recover_manifest.name, MAX_BODY) != recovery_manifest_raw:
                raise SetupError("recovery_manifest_changed")
            for entry in recovery_entries:
                if digest(read_file(recover_workspace, "requests/" + entry["id"] + ".json", MAX_BODY * 2)) != entry["sha256"]:
                    raise SetupError("recovery_request_changed")
        # Freeze exact operator-selected bytes before executing any bundle code.
        # The invoking operator controls this directory throughout its lifetime.
        workspace.mkdir(mode=0o700)
        self.workspace = workspace
        if recovery_manifest_raw is not None:
            write_new(workspace / "recovery-manifest.json", recovery_manifest_raw)
            sync_file(workspace, "recovery-manifest.json")
        self.bundle = workspace / "bundle"
        self.bundle.mkdir(mode=0o700)
        size = 0
        for name, metadata in sorted(files.items()):
            relative(name)
            expected = metadata.get("sha256") if isinstance(metadata, dict) else None
            if not isinstance(expected, str) or not HEX.fullmatch(expected):
                raise SetupError("invalid_bundle")
            data = read_file(bundle, name, 64 * 1024 * 1024)
            size += len(data)
            if size > 1024 * 1024 * 1024 or digest(data) != expected:
                raise SetupError("bundle_file_mismatch")
            destination = self.bundle / name
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            write_new(destination, data)
        if raw is not None:
            write_new(self.bundle / "CONTENTS.json", raw)
        self.prior_request_sha256 = prior_request_sha256
        self.prior_summary = None
        if prior_raw is not None:
            write_new(workspace / "prior-request.json", prior_raw)
            self.prior_summary = {"request_sha256": prior_request_sha256,
                                  **{key: prior_record["selection"][key] for key in ("family", "profile")}}
        self.contents_sha256 = contents_sha256
        self.installer_sha256 = files.get("install-library-consumer.py", {}).get("sha256")
        self.official_bundle = None
        self.official_contents_sha256, self.official_installer_sha256 = official_contents_sha256, official_installer_sha256
        self.official_installer = workspace / "official-installer.py" if official else None
        if official:
            official_root, official_raw, official_code, official_files = official
            self.official_bundle = workspace / "official"
            self.official_bundle.mkdir(mode=0o700)
            total = 0
            for name, metadata in official_files.items():
                parts = relative(name)
                if name != "delivery.json" and parts[0] not in {"playwright" if f == PLAYWRIGHT_FAMILY else "github" for f in families if f in OFFICIAL_FAMILIES}:
                    continue
                if not isinstance(metadata, dict) or not isinstance(metadata.get("sha256"), str):
                    raise SetupError("invalid_official_selection")
                data = read_file(official_root, name, 64 * 1024 * 1024)
                total += len(data)
                if total > 128 * 1024 * 1024 or digest(data) != metadata["sha256"]:
                    raise SetupError("official_digest_mismatch")
                path = self.official_bundle / name
                path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                write_new(path, data)
            write_new(self.official_bundle / "CONTENTS.json", official_raw)
            write_new(self.official_installer, official_code)
        self.playwright_host_profile = workspace / "playwright-host.json" if profile_raw is not None else None
        self.playwright_host_profile_sha256 = playwright_host_profile_sha256
        if profile_raw is not None:
            write_new(self.playwright_host_profile, profile_raw)
        self.catalog_sha256 = files.get("catalog.json", {}).get("sha256")
        self.routes_sha256 = files.get("routes.json", {}).get("sha256")
        self.bundle_files = copy.deepcopy(files)
        self.runtime_target = copy.deepcopy(manifest.get("runtime_target")) if bundle is not None else None
        self.verifier = workspace / "verifier.py" if verifier_bytes is not None else None
        self.verifier_sha256 = verifier_sha256
        if self.verifier is not None:
            write_new(self.verifier, verifier_bytes)
        self.guide_hashes = {family: files[family + "/PROFILE-GUIDE.md"]["sha256"] for family in library_families}
        for name in ("tmp", "inspection", "configurations", "installations", "requests"):
            (workspace / name).mkdir(mode=0o700)
        self.selected, self.selected_binding = selected, selected_binding
        self.selected_attempts = 0
        self.selected_retries = {}
        self.plugin_inspections = {}
        if selected is not None:
            (workspace / "selected-tools").mkdir(mode=0o700)
            (workspace / "selections").mkdir(mode=0o700)
            (workspace / "plugin-inspections").mkdir(mode=0o700)
            write_new(workspace / "selected-host-profile.json", selected_raw)
            for name, data in selected_payloads.items():
                tool = workspace / "selected-tools" / (name + (".py" if name == "installer" else ""))
                write_new(tool, data)
                if name not in ("installer", "node_build"):
                    tool.chmod(0o700)
        self.local_recipes = recipes
        self.allow_remote_recipes = allow_remote_recipes
        self.recipe_exports = {}
        self.export_retries = {}
        self.export_intents = {}
        # Only session-local handles cross the model boundary. Canonical
        # keyset cursors may contain non-recipe store keys and stay host-side.
        self.recipe_cursors = {}
        self.recipe_cursor_handles = {}
        self.local_recipes_profile_sha256 = local_recipes_profile_sha256
        if recipes is not None:
            write_new(workspace / "local-recipes-profile.json", recipes_raw)
            write_new(workspace / "local-recipes-tool", recipes_tool)
            (workspace / "local-recipes-tool").chmod(0o700)
            if recipes["scope"] == LOCAL_RECIPES_RETAIN_SCOPE:
                (workspace / "recipe-retention").mkdir(mode=0o700)
        if allow_remote_recipes:
            (workspace / "remote-recipes").mkdir(mode=0o700)
        self.runner = runner_factory(workspace)
        self._job_context = getattr(self.runner, "_job_context", threading.local())
        self._job_controls = {}
        self.families = {family: {"id": family, "status": "unprepared", "profiles": []} for family in families}
        self.lock = threading.RLock()
        self.operation_lock = threading.Lock()
        self.validations, self.jobs, self.commands = {}, {}, {}
        self.preparation_retries = {}
        self.install_intents, self.observations = {}, {}
        self.observation_revision = 0
        self.job = None
        self.threads = []
        self._job_threads = {}
        self.closing = False
        self.request_retention_failed = False
        self.recovery_only = _local_read_only if _retained_selections else bool(recoveries)
        if self.acquired_inspection_raw is not None:
            write_new(self.workspace / "acquired-inspection.py", self.acquired_inspection_raw)
        self.recovery_selection = recovery_entries is not None or bool(_retained_selections)
        self.compose = compose
        if compose is not None:
            compose.bind(self.runner)
        self.python_acquisition = acquisition
        self.node_acquisition = node_acquisition
        self.node_prebuilt_acquisition = node_prebuilt_acquisition
        self.node_search = node_search
        self.public_github = None
        if allow_public_github_research:
            spec = importlib.util.spec_from_file_location("consumer_setup_public_github",
                Path(__file__).with_name("public_github.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            io = SimpleNamespace(Error=SetupError, read_file=read_file, write_new=write_new)
            self.public_github = module.PublicGitHub(io, self.python)
            self.public_github.bind(workspace)
        self.compose_acquisition = compose_acquisition
        self.mcpb_acquisition = mcpb_acquisition
        self.acquisition_requests = {}
        self.acquisition_modules = acquisition_modules
        self.acquisition_bindings = {}
        self.acquisition_intents = {}
        self.acquisition_recovery_summary = {}
        for kind, owner in (("python", acquisition), ("node", node_acquisition), ("node-prebuilt", node_prebuilt_acquisition), ("compose", compose_acquisition), ("mcpb", mcpb_acquisition)):
            if owner is not None:
                owner.bind(workspace)
                self.acquisition_bindings[kind] = {
                    "profile_sha256": owner.profile_sha,
                    "assets": {name: digest(owner.frozen[name]) for name in sorted(ACQUISITION_ASSETS[kind])},
                    "selected_profile_sha256": selected_binding["profile_sha256"] if kind != "compose" else None}
        if self.acquisition_bindings:
            (workspace / "acquisitions").mkdir(mode=0o700)
        if node_search is not None and node_acquisition is None:
            node_search.bind(workspace)
        # Selection order is host input, not historical execution chronology.
        # The compatibility state.job field points to the last selected item.
        for record, intent, request_sha256 in recoveries:
            exporting, acquiring = intent.get("kind") == "export", intent.get("kind") == "acquire"
            self.job = {"id": record["job_id"], "kind": "acquire" if acquiring else "export" if exporting else "install",
                        "family": record["family"] if acquiring else "remote-recipe" if exporting else record["selection"]["family"],
                        "status": "unconfirmed", "error": "historical_outcome_unknown", "recovered": True,
                        "recovery_request": {"id": record["job_id"], "sha256": request_sha256}}
            self.jobs[self.job["id"]] = self.job
            if intent.get("kind") == "selected" and "retry_of" in record:
                # Lineage only. Recovery never restores retry/install authority.
                self.job["retry_of"] = record["retry_of"]
            if acquiring:
                kind = record["family"].removeprefix("selected-")
                self.job["request_id"] = record["arguments"]["request_id"]
                self.acquisition_intents[self.job["id"]] = intent
                identity = {"kind": kind, "arguments": copy.deepcopy(record["arguments"])}
                identifier = record["arguments"]["request_id"]
                # Multiple old anchors remain separately readable. Ambiguity
                # refuses repeat acquisition instead of selecting by chronology.
                if identifier in self.acquisition_requests:
                    self.acquisition_requests[identifier] = (None, None)
                else:
                    self.acquisition_requests[identifier] = (identity, self.job["id"])
                if self.recovery_only:
                    self.acquisition_recovery_summary[kind.replace("-", "_") + "_acquisition"] = {
                        "available": True, "readback_only": True, "scope": ACQUISITION_SCOPES[kind],
                        "staging_root": record["staging_root"], "workspace": record["workspace"],
                        "host_profile_sha256": record["host"]["profile_sha256"]}
            elif exporting:
                self.job["request_sha256"] = record["request_sha256"]
                self.export_intents[self.job["id"]] = intent
                key = record["request_sha256"]
                self.recipe_exports[key] = None if key in self.recipe_exports else self.job["id"]
            else:
                self.install_intents[self.job["id"]] = intent

    def admit_setup(self):
        self.admit()
        if self.request_retention_failed:
            raise SetupError("recovery_request_unavailable", 409)
        if self.recovery_only:
            raise SetupError("recovery_read_only", 409)

    def admit(self):
        if self.closing:
            raise SetupError("host_closing", 409)

    def close_admission(self):
        with self.lock:
            self.closing = True

    def scope(self):
        return ("Host-permitted local software installation and separately delegated container-plan execution; "
                "no account access, tool attachment or native authority is supplied." if self.compose else SCOPE)

    def selected_build_planning(self):
        if self.selected is None or "node_build" not in self.selected or self.recovery_only:
            return None
        if not self.operation_lock.acquire(blocking=False):
            return {"available": False, "reason": "operation_in_progress"}
        try:
            tools = self.selected_tools()
            code, raw = self._run([self.python, "-I", "-B", tools["installer"], "--describe-node-build",
                "--node", self.selected_node, "--node-build-profile", tools["node_build"],
                "--node-build-profile-sha256", self.selected_binding["tools"]["node_build"]], capture=True)
            value = decode(raw)
            if (code or not isinstance(value, dict) or value.get("scope") != "selected_node_build_planning"
                    or value.get("profile_sha256") != self.selected_binding["tools"]["node_build"]
                    or len(encoded(value)) > MAX_BODY):
                raise SetupError("selected_build_review_unavailable", 409)
            self.selected_tools()
            return {"available": True, "planning": value}
        except (SetupError, ValueError, OSError, TypeError, KeyError):
            return {"available": False, "reason": "host_toolchain_unavailable",
                    "guidance": "The host must supply the selected toolchain files and matching Node target before checking a build plan."}
        finally:
            self.operation_lock.release()

    def state(self):
        build_planning = self.selected_build_planning()
        with self.lock:
            return copy.deepcopy({"families": list(self.families.values()), "job": self.job,
                                  "scope": self.scope(),
                                  **({"node_build": build_planning} if build_planning is not None else {}),
                                  **({"compose": self.compose.summary()} if self.compose else {}),
                                  **({"python_acquisition": {**self.python_acquisition.summary(), "workspace": str(self.workspace)}}
                                     if self.python_acquisition is not None else {}),
                                  **({"node_acquisition": {**self.node_acquisition.summary(), "workspace": str(self.workspace)}}
                                     if self.node_acquisition is not None else {}),
                                  **({"node_search": self.node_search.search_summary()} if self.node_search is not None else {}),
                                  **({"compose_acquisition": {**self.compose_acquisition.summary(), "workspace": str(self.workspace)}}
                                     if self.compose_acquisition is not None else {}),
                                  **({"node_prebuilt_acquisition": {**self.node_prebuilt_acquisition.summary(), "workspace": str(self.workspace)}}
                                     if self.node_prebuilt_acquisition is not None else {}),
                                  **({"mcpb_acquisition": {**self.mcpb_acquisition.summary(), "workspace": str(self.workspace)}}
                                     if self.mcpb_acquisition is not None else {}),
                                  **self.acquisition_recovery_summary,
                                  "family_contents_sha256": {family: self.input_binding(family)[0] for family in self.families},
                                  "job_history": {"available": True, "count": len(self.jobs),
                                      **({"recovery_selection": True} if self.recovery_selection else {})},
                                  "recovery_manifest": {"available": True, "max_requests": 256},
                                  "operation_in_progress": self.operation_lock.locked(),
                                  **({"acquired_inspection": {"available": True, "scope": "acquired_primary_archive_only",
                                      "dependency_scope": "acquired_dependency_archive_only",
                                      "archive_selection": "optional_archive_sha256_from_job_artifacts",
                                      "page_size": 64, "max_text_bytes": SELECTED_DOCUMENT,
                                      "max_chunk_bytes": SELECTED_INLINE_DOCUMENT}}
                                     if self.acquired_inspection_raw is not None else {}),
                                  "stop": {"available": not self.recovery_only},
                                  "preparation_retry": {"available": not self.recovery_only},
                                  "export_retry": {"available": self.allow_remote_recipes and not self.recovery_only},
                                  "selected_retry": {"available": self.selected is not None and not self.recovery_only},
                                  **({"prior_selection": self.prior_summary} if self.prior_summary is not None and not self.recovery_only else {}),
                                  "mode": "recovery" if self.recovery_only else "setup",
                                  "catalog": {"available": bool(self.catalog_sha256) and not self.recovery_only,
                                      "catalog_sha256": self.catalog_sha256, "contents_sha256": self.contents_sha256},
                                  "local_recipes": {"available": self.local_recipes is not None and not self.recovery_only,
                                      **({"remote_retention_available": True, "export_available": True}
                                         if self.allow_remote_recipes and not self.recovery_only else {}),
                                      **({"profile_sha256": self.local_recipes_profile_sha256, "max_output_bytes": MAX_JSON}
                                         if self.local_recipes is not None and not self.recovery_only else {}),
                                      **({"retention_available": True} if self.local_recipes is not None
                                         and self.local_recipes["scope"] == LOCAL_RECIPES_RETAIN_SCOPE and not self.recovery_only else {})},
                                  **({"selected_installation": {"scope": SELECTED_SCOPE,
                                      "profiles": list(self.selected["validators"]), "staging_root": self.selected["staging_root"],
                                      "max_document_bytes": SELECTED_DOCUMENT, "max_file_bytes": SELECTED_FILE,
                                      "max_inline_document_bytes": SELECTED_INLINE_DOCUMENT, "max_request_bytes": MAX_BODY,
                                      "max_total_bytes": SELECTED_TOTAL, "max_closure_files": 256,
                                      "max_arguments_json_bytes": SELECTED_ARGUMENTS, "max_readback_output_bytes": MAX_JSON,
                                      "max_receipt_bytes": SELECTED_FILE}}
                                      if self.selected is not None and not self.recovery_only else {}),
                                  **({"package_inspection": {"available": True, "scope": "metadata_only",
                                       "max_document_bytes": SELECTED_INLINE_DOCUMENT, "max_request_bytes": MAX_BODY}}
                                      if self.selected is not None and "package_inspector" in self.selected and not self.recovery_only else {}),
                                  **({"plugin_inspection": {"available": True, "scope": PLUGIN_SOURCE_SCOPE,
                                       "max_files": PLUGIN_FILE_COUNT, "max_file_bytes": SELECTED_INLINE_DOCUMENT,
                                       "max_total_bytes": PLUGIN_FILES_TOTAL}}
                                      if self.selected is not None and self.selected.get("plugin_inspection") is True and not self.recovery_only else {}),
                                  **({"mcpb_inspection": {"available": True, "scope": "metadata_only",
                                       "max_files": 16, "max_file_bytes": SELECTED_INLINE_DOCUMENT,
                                       "max_total_bytes": PLUGIN_FILES_TOTAL, "inventory_limit": 128}}
                                      if self.selected is not None and self.selected.get("mcpb_inspection") is True
                                      and (not self.recovery_only or self.allow_acquired_inspection) else {}),
                                  "reconciliation": {"available": self.reconciliation_available(self.job),
                                      "observation": self.observations.get(self.job["id"]) if self.job else None}})

    def job_summaries(self, request):
        keys(request, ("before",))
        with self.lock:
            before = request["before"]
            identifiers = list(reversed(self.jobs))
            if before is not None:
                if not isinstance(before, str) or before not in self.jobs:
                    raise SetupError("unknown_job", 404)
                identifiers = identifiers[identifiers.index(before) + 1:]
            return self._job_summary_page(identifiers[:51])

    def _job_summary_page(self, identifiers):
        """Project one bounded page selected by the owning host connection."""
        if not isinstance(identifiers, list) or len(identifiers) > 51:
            raise SetupError("invalid_job_page", 500)
        with self.lock:
            if any(identifier not in self.jobs for identifier in identifiers):
                raise SetupError("unknown_job", 404)
            page = identifiers[:50]
            return {"jobs": [{**{key: self.jobs[identifier][key] for key in ("id", "kind", "family", "status")},
                              "profile": self.install_intents.get(identifier, {}).get("profile"),
                              **({key: self.jobs[identifier][key] for key in ("action", "request_id", "stop_supported")}
                                 if self.jobs[identifier]["kind"] == "compose" else {}),
                              **({"stop": copy.deepcopy(self.jobs[identifier]["stop"])} if "stop" in self.jobs[identifier] else {}),
                              **({"recovered": True, "recovery_request": copy.deepcopy(self.jobs[identifier]["recovery_request"])}
                                 if self.recovery_selection and self.jobs[identifier].get("recovered") else {})}
                             for identifier in page],
                    "next_before": page[-1] if len(identifiers) > len(page) else None}

    def job_detail(self, request):
        keys(request, ("job_id",))
        with self.lock:
            identifier = request["job_id"]
            if not isinstance(identifier, str) or identifier not in self.jobs:
                raise SetupError("unknown_job", 404)
            job = self.jobs[identifier]
            return copy.deepcopy({"job": job,
                                  "reconciliation": {"available": self.reconciliation_available(job),
                                                     "observation": self.observations.get(identifier)}})

    def wait_acquisition(self, identifier):
        """Join this controller's exact acquisition worker without holding state.

        The MCP adapter calls this only with the job returned by acquisition
        admission. Its existing owner enforces the acquisition deadline and
        stop checks. Joining also waits for the worker's cleanup and operation
        lock release; reading a terminal row alone would not establish that.
        A recovered row without a worker remains historically unconfirmed.
        """
        with self.lock:
            job = self.jobs.get(identifier) if isinstance(identifier, str) else None
            if job is None or job["kind"] != "acquire":
                raise SetupError("unknown_acquisition_job", 404)
            thread = self._job_threads.get(identifier)
            if thread is None and job["status"] == "running":
                raise SetupError("acquisition_worker_unavailable", 409)
        if thread is not None:
            thread.join()
        with self.lock:
            return {"job": copy.deepcopy(job)}

    def wait_selected_install(self, identifier):
        """Join the admitted selected installer, including verified readback.

        This is the MCP delivery path, not another admission or a retry. The
        existing worker retains stop and effect ownership; caller cancellation
        does not request stopping it. No native deadline is changed here.
        """
        with self.lock:
            job = self.jobs.get(identifier) if isinstance(identifier, str) else None
            intent = self.install_intents.get(identifier) if job is not None else None
            if (job is None or job["kind"] != "install"
                    or job["family"] not in ("selected-python", "selected-node", "selected-mcpb")
                    or intent is None or intent.get("kind") != "selected"):
                raise SetupError("unknown_selected_install_job", 404)
            thread = self._job_threads.get(identifier)
            if thread is None and job["status"] == "running":
                raise SetupError("selected_install_worker_unavailable", 409)
        # Join outside state custody so observation, exact replay and explicit
        # stop remain available. A terminal row precedes final worker cleanup.
        if thread is not None:
            thread.join()
        with self.lock:
            return {"job": copy.deepcopy(job)}

    def reconciliation_available(self, job):
        return bool(job and (job["kind"] == "acquire" and job["id"] in self.acquisition_intents or
                    job["kind"] == "export" and job["id"] in self.export_intents or
                    job["kind"] == "install" and
                    (self.verifier is not None or job["family"] in ("selected-python", "selected-node", "selected-mcpb"))))

    def stop(self, request):
        keys(request, ("job_id",))
        with self.lock:
            self.admit_setup()
            identifier = request["job_id"]
            if not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier) or identifier not in self.jobs:
                raise SetupError("unknown_job", 404)
            job = self.jobs[identifier]
            if job.get("stop_supported") is False:
                raise SetupError("job_stop_not_supported", 409)
            if job["status"] == "running" and "stop" not in job:
                control = self._job_controls[identifier]
                with control["spawn_lock"]:
                    control["requested"].set()
                    job["stop"] = {"status": "requested", "scope": "local_job_only"}
            # Acceptance is not settlement or compensation. The worker alone
            # signals and joins its process before releasing the operation.
            return {"job": copy.deepcopy(job)}

    def _check_stop(self):
        control = getattr(self._job_context, "control", None)
        if control is not None and control["requested"].is_set():
            raise SetupError("operation_stopped", 409)

    def _run(self, argv, **kwargs):
        # Keep test/custom runners' existing call contract. Production Runner
        # shares this thread-local binding and also checks while waiting.
        self._check_stop()
        result = self.runner.run(argv, **kwargs)
        self._check_stop()
        return result

    def _run_selected_installer(self, argv, profile, *, installer_timeout=None, **kwargs):
        # Only these selected commands invoke the pinned Python installer with
        # its TERM/join handler. Custom run signatures and MCPB stay unchanged.
        if profile not in ("python", "node"):
            return self._run(argv, **kwargs)
        if installer_timeout is not None and (profile != "node" or type(installer_timeout) is not int
                                               or not 1 <= installer_timeout <= 3600):
            raise SetupError("selected_build_review_unavailable", 409)
        prior_timeout = getattr(self._job_context, "installer_timeout", None)
        if installer_timeout is not None:
            self._job_context.installer_timeout = installer_timeout
        previous = getattr(self._job_context, "termination_grace", None)
        self._job_context.termination_grace = SELECTED_INSTALLER_TERMINATION_GRACE
        try:
            return self._run(argv, **kwargs)
        finally:
            if previous is None:
                del self._job_context.termination_grace
            else:
                self._job_context.termination_grace = previous
            if installer_timeout is not None:
                if prior_timeout is None:
                    del self._job_context.installer_timeout
                else:
                    self._job_context.installer_timeout = prior_timeout

    def compose_action(self, action, request):
        with self.lock:
            self.admit_setup()
            if self.compose is None:
                raise SetupError("compose_unavailable", 409)
            request = self.compose.arguments(action, request)
            if action not in self.compose.host["allowed_actions"]:
                raise SetupError("compose_action_not_allowed", 403)
            if action != "observe":
                previous = self.compose.intents.get(request["request_id"])
                identity = {"action": action, "arguments": request}
                if previous is not None:
                    if previous[0] != identity:
                        raise SetupError("compose_request_changed", 409)
                    return {"job": copy.deepcopy(self.jobs[previous[1]])}
                self.compose.admit(action)
                if len(self.compose.intents) >= 128:
                    raise SetupError("compose_attempt_limit", 409)
                def reserved(job):
                    job.update(action=action, request_id=request["request_id"], stop_supported=False)
                    self.compose.intents[request["request_id"]] = (copy.deepcopy(identity), job["id"])
                def settled(result):
                    return "unconfirmed" if result["backend"].get("status") == "unconfirmed" else "succeeded"
                job = self.start_job("compose", "compose", lambda: self.compose.execute(action, request),
                                     before_start=reserved, result_status=settled)
                return {"job": copy.deepcopy(job)}
            self.compose.admit(action)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            return self.compose.execute(action, request)
        finally:
            self.operation_lock.release()

    def acquire_python(self, request):
        """Stage an explicitly chosen distribution using the host's trusted pip."""
        return self._acquire("python", self.python_acquisition, request)

    def acquire_node(self, request):
        """Stage an explicitly chosen package using the host's trusted npm."""
        return self._acquire("node", self.node_acquisition, request)

    def acquire_node_prebuilt(self, request):
        """Derive inert payloads from the exact current owned Node acquisition."""
        return self._acquire("node-prebuilt", self.node_prebuilt_acquisition, request)

    def _node_prebuilt_source(self, source):
        # Called only by the entered acquisition worker, which already holds
        # operation_lock. A retained/recovered or derived job is not a source.
        with self.lock:
            job = self.jobs.get(source["job_id"])
            if not job or job.get("family") != "selected-node":
                raise SetupError("node_prebuilt_source_unavailable", 409)
        return self.selected_acquired_source({"profile": "node", "staged_id": source["job_id"],
                                             "acquired_source": source})

    def github_research(self, operation, request):
        with self.lock:
            self.admit_setup()
            if self.public_github is None:
                raise SetupError("github_research_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            return self.public_github.run(operation, request, self.runner)
        finally:
            self.operation_lock.release()

    def search_github(self, request):
        return self.github_research("search", request)

    def read_github(self, request):
        return self.github_research("read", request)

    def search_node(self, request):
        with self.lock:
            self.admit_setup()
            if self.node_search is None:
                raise SetupError("node_search_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            return self.node_search.search(request, self.runner)
        finally:
            self.operation_lock.release()

    def acquire_compose(self, request):
        """Acquire selected images and prepare their graph for host review."""
        return self._acquire("compose", self.compose_acquisition, request)

    def acquire_mcpb(self, request):
        """Retain hash-matched bytes from the agent's host-permitted public URL."""
        return self._acquire("mcpb", self.mcpb_acquisition, request)

    def _acquire(self, kind, acquisition, request):
        with self.lock:
            self.admit_setup()
            if acquisition is None:
                raise SetupError(kind + "_acquisition_unavailable", 409)
            request = acquisition.arguments(request)
            identity = {"kind": kind, "arguments": request}
            previous = self.acquisition_requests.get(request["request_id"])
            if previous is not None:
                if previous[0] != identity:
                    raise SetupError("acquisition_request_changed", 409)
                return {"job": copy.deepcopy(self.jobs[previous[1]])}
            if len(self.acquisition_requests) >= 128:
                raise SetupError("new_workspace_required", 409)
            directory = None
            def reserved(job):
                nonlocal directory
                directory = self.workspace / "acquisitions" / job["id"]
                directory.mkdir(mode=0o700)
                write_new(directory / "request.json", encoded(request))
                if kind in self.acquisition_bindings:
                    host = self.acquisition_bindings[kind]
                    record = {"schema_version": 1, "kind": ACQUISITION_REQUEST, "job_id": job["id"],
                              "family": "selected-" + kind, "workspace": str(self.workspace),
                              "staging_root": str(acquisition.staging), "arguments": request, "host": host}
                    data = encoded(record)
                    if len(data) > MAX_BODY * 2:
                        raise SetupError("invalid_recovery_request")
                    assets = self.workspace / (kind + "-acquisition")
                    for name, expected in host["assets"].items():
                        if digest(read_file(assets, name, len(acquisition.frozen[name]))) != expected:
                            raise SetupError("acquisition_host_changed", 409)
                        sync_file(assets, name)
                    sync_file(directory, "request.json")
                    sync_directory(directory.parent)
                    with self._retaining_request(job, data):
                        if read_file(directory, "request.json", MAX_BODY) != encoded(request):
                            raise SetupError("recovery_request_changed", 409)
                        record, intent = load_acquisition_recovery(self.workspace, job["id"], digest(data))
                        self.acquisition_intents[job["id"]] = intent
                        job["recovery_request"] = {"id": job["id"], "sha256": digest(data)}
                job.update(request_id=request["request_id"])
                self.acquisition_requests[request["request_id"]] = (copy.deepcopy(identity), job["id"])
            runner = self.runner.run_compose if kind == "compose" else self._run
            job = self.start_job("acquire", "selected-" + kind,
                lambda: acquisition.run(request, directory, runner, self._check_stop),
                before_start=reserved)
            return {"job": copy.deepcopy(job)}

    def catalog_read(self):
        self.admit_setup()
        if self.catalog_sha256 is None:
            raise SetupError("catalog_unavailable", 409)
        try:
            data = read_file(self.bundle, "catalog.json", MAX_JSON)
            if digest(data) != self.catalog_sha256:
                raise SetupError("catalog_digest_mismatch")
            return catalog_records(data)
        except OSError as error:
            raise SetupError("catalog_unavailable", 409) from error

    def recipes(self, request):
        if request == {}:
            return self.local_recipe_observation(None)
        return self.local_recipe_observation(None, page=recipe_page_arguments(request))

    def recipe(self, request):
        keys(request, ("publisher", "name", "version", "digest"))
        if (any(not isinstance(request[k], str) or not 1 <= len(request[k]) <= 256
                or "\0" in request[k] for k in ("publisher", "name", "version"))
                or not isinstance(request["digest"], str) or not HEX.fullmatch(request["digest"])):
            raise SetupError("invalid_request")
        return self.local_recipe_observation(request)

    def local_recipe_observation(self, request, page=None):
        with self.lock:
            self.admit_setup()
            if self.local_recipes is None:
                raise SetupError("local_recipes_unavailable", 409)
            public_page = page
            if page is not None:
                page = copy.deepcopy(page)
                if "cursor" in page:
                    if page["cursor"] not in self.recipe_cursors:
                        raise SetupError("recipe_cursor_unknown", 409)
                    page["cursor"] = self.recipe_cursors[page["cursor"]]
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            canonical_directory(self.workspace)
            if (digest(read_file(self.workspace, "local-recipes-profile.json", MAX_BODY)) != self.local_recipes_profile_sha256
                    or digest(read_file(self.workspace, "local-recipes-tool", LOCAL_RECIPES_TOOL_BYTES)) != self.local_recipes["tool"]["sha256"]):
                raise ValueError()
            local_recipe_database(self.local_recipes["database"])
            argv = [str(self.workspace / "local-recipes-tool"), "list-page" if page is not None else "list" if request is None else "read",
                    "--database", self.local_recipes["database"], "--realm", self.local_recipes["realm"],
                    "--owner", self.local_recipes["owner"]]
            if request is not None:
                for key in ("publisher", "name", "version", "digest"):
                    argv.extend(["--" + key, request[key]])
            if page is not None:
                argv.extend(["--limit", str(page["limit"])])
                if "cursor" in page:
                    argv.extend(["--cursor", page["cursor"]])
            code, raw = self._run(argv, capture=True)
            if code != 0:
                raise ValueError()
            value = local_recipe_output(raw, request, page=page)
            if page is not None:
                cursor = value["next_cursor"]
                with self.lock:
                    if cursor is not None and cursor not in self.recipe_cursor_handles:
                        if len(self.recipe_cursors) >= 1024:
                            raise SetupError("recipe_cursor_limit", 409)
                        handle = secrets.token_hex(16)
                        while handle in self.recipe_cursors:
                            handle = secrets.token_hex(16)
                        self.recipe_cursors[handle] = cursor
                        self.recipe_cursor_handles[cursor] = handle
                    value["next_cursor"] = self.recipe_cursor_handles.get(cursor)
                    value["request_cursor"] = public_page.get("cursor")
            return value
        except SetupError as error:
            if error.code == "recipe_cursor_limit":
                raise
            raise SetupError("local_recipes_failed", 500) from None
        except (OSError, TypeError, ValueError, RecursionError):
            # Child messages and profile values are never error diagnostics.
            raise SetupError("local_recipes_failed", 500) from None
        finally:
            self.operation_lock.release()

    def remember(self, request):
        request = retention_arguments(request)
        with self.lock:
            self.admit_setup()
            if self.local_recipes is None or self.local_recipes["scope"] != LOCAL_RECIPES_RETAIN_SCOPE:
                raise SetupError("recipe_retention_unavailable", 409)
            job = self.jobs.get(request["job_id"])
            intent = self.install_intents.get(request["job_id"])
            if (job is None or intent is None or intent.get("kind") != "selected"
                    or job.get("kind") != "install" or job.get("recovered")
                    or job.get("family") not in ("selected-python", "selected-node", "selected-mcpb")):
                raise SetupError("retention_job_unavailable", 409)
            if job["status"] == "running":
                raise SetupError("operation_in_progress", 409)
            if job["status"] not in ("succeeded", "unconfirmed"):
                raise SetupError("retention_job_unavailable", 409)
            job, intent = copy.deepcopy(job), copy.deepcopy(intent)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        failure = "retention_source_unavailable"
        value = None
        try:
            canonical_directory(self.workspace)
            if (digest(read_file(self.workspace, "local-recipes-profile.json", MAX_BODY)) != self.local_recipes_profile_sha256
                    or digest(read_file(self.workspace, "local-recipes-tool", LOCAL_RECIPES_TOOL_BYTES)) != self.local_recipes["tool"]["sha256"]):
                raise ValueError()
            local_recipe_database(self.local_recipes["database"])
            self.selected_tools()
            anchor = job["recovery_request"]
            if anchor["id"] != request["job_id"] or intent["root"] != self.workspace:
                raise ValueError()
            record, recovered = load_selected_recovery(self.workspace, anchor["id"], anchor["sha256"], self.selected_binding)
            if (record["validation_id"] != intent["validation_id"] or record["effect_sha256"] != intent["effect_sha256"]
                    or recovered["destination"] != intent["destination"] or recovered["family"] != job["family"]):
                raise ValueError()
            source, effect, plan = selected_proposal(self.workspace, intent["validation_id"], intent["effect_sha256"], self.selected_binding)
            package = decode(selected_read(self.workspace, "selections/" + intent["validation_id"] + "/package.json", SELECTED_DOCUMENT))
            display_name = package["display_name"]
            if not isinstance(display_name, str) or not display_name or len(display_name.encode("utf-8")) > MAX_BODY:
                raise ValueError()
            verification = encoded(request["verification"])
            directory = canonical_directory(self.workspace / "recipe-retention")
            filename = digest(verification) + ".json"
            try:
                write_new(directory / filename, verification)
            except FileExistsError:
                pass
            if read_file(self.workspace, "recipe-retention/" + filename, MAX_BODY) != verification:
                raise ValueError()
            argv = [str(self.workspace / "local-recipes-tool"), "save",
                    "--database", self.local_recipes["database"], "--realm", self.local_recipes["realm"],
                    "--owner", self.local_recipes["owner"], "--package", str(source / "package.json"),
                    selected_document(effect["request"])[0], str(source / selected_document(effect["request"])[1]),
                    "--profile", effect["request"]["profile"],
                    "--component", plan["component"], "--verification", str(directory / filename)]
            for key in ("owner", "record", "revision"):
                argv.extend(["--decision-" + key, request["decision"][key]])
            if effect["request"]["profile"] == "mcpb":
                argv.extend(["--configuration", str(source / "configuration.json")])
            if "plugin_source" in effect["request"]:
                # selected_proposal already checked these exact retained bytes.
                # The recipe owner validates and stores them as historical
                # evidence, never as an executable installer input.
                argv.extend(["--plugin-source", str(source / "plugin-source.json")])
            # From this point a failed/lost observation cannot prove no save.
            failure = "retention_unconfirmed"
            code, raw = self._run(argv, capture=True)
            value = retention_output(raw, code, request, effect, plan, display_name)
            if value is None:
                failure = "retention_conflict"
        except (SetupError, OSError, KeyError, TypeError, ValueError, RecursionError):
            pass
        finally:
            self.operation_lock.release()
        if value is None:
            # Raised outside the handler so raw child/parser exceptions cannot
            # become a retained exception context.
            raise SetupError(failure, 409 if failure == "retention_conflict" else 500)
        return value

    def admit_remote_recipes(self):
        self.admit_setup()
        if not self.allow_remote_recipes or self.local_recipes is None or self.local_recipes["scope"] != LOCAL_RECIPES_RETAIN_SCOPE:
            raise SetupError("remote_recipes_unavailable", 409)

    def remote_recipe_command(self, action):
        canonical_directory(self.workspace)
        if (digest(read_file(self.workspace, "local-recipes-profile.json", MAX_BODY)) != self.local_recipes_profile_sha256
                or digest(read_file(self.workspace, "local-recipes-tool", LOCAL_RECIPES_TOOL_BYTES)) != self.local_recipes["tool"]["sha256"]):
            raise SetupError("recipe_source_changed", 409)
        local_recipe_database(self.local_recipes["database"])
        return [str(self.workspace / "local-recipes-tool"), action,
                "--database", self.local_recipes["database"], "--realm", self.local_recipes["realm"],
                "--owner", self.local_recipes["owner"]]

    def freeze_remote_request(self, name, request, documents):
        directory = canonical_directory(self.workspace / "remote-recipes") / name
        directory.mkdir(mode=0o700)
        files = {"request.json": encoded(request), **{name: text.encode("utf-8") for name, text in documents.items() if text is not None}}
        for filename, data in files.items():
            write_new(directory / filename, data)
        return directory, files

    def verify_remote_request(self, directory, files):
        for name, data in files.items():
            if read_file(self.workspace, str(directory.relative_to(self.workspace) / name), MAX_BODY) != data:
                raise SetupError("recipe_source_changed", 409)

    def remember_remote(self, request):
        request = remote_recipe_arguments(request)
        with self.lock:
            self.admit_remote_recipes()
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        failure, result = "retention_source_unavailable", None
        try:
            argv = self.remote_recipe_command("save")
            documents = {"plugin.json": request["plugin_json"], "mcp.json": request["mcp_json"], "recipes.json": request["recipes_json"]}
            directory, files = self.freeze_remote_request("save-" + secrets.token_hex(16), request, documents)
            self.verify_remote_request(directory, files)
            argv += ["--plugin", str(directory / "plugin.json"), "--recipes", str(directory / "recipes.json")]
            if request["mcp_json"] is not None:
                argv += ["--mcp", str(directory / "mcp.json")]
            if "decision" in request:
                for key in ("owner", "record", "revision"):
                    argv += ["--decision-" + key, request["decision"][key]]
            failure = "retention_unconfirmed"
            code, raw = self._run(argv, capture=True)
            if code:
                # Recheck the submitted bytes before attributing a diagnostic
                # to them. A late readback error cannot use this path.
                self.verify_remote_request(directory, files)
                failure = remote_recipe_save_failure(raw)
                raise ValueError()
            value = remote_recipe_envelope(raw, "retained", ("recipe",))
            remote_recipe_summary(value["recipe"], decision=request.get("decision"))
            identity = {**value["recipe"]["identity"], "digest": value["recipe"]["digest"]}
            self.verify_remote_request(directory, files)
            argv = self.remote_recipe_command("read")
            for key, text in identity.items():
                argv += ["--" + key, text]
            code, raw = self._run(argv, capture=True)
            if code:
                raise ValueError()
            observed = local_recipe_output(raw, identity)
            expected = {**value["recipe"], "documents": {name: request[name] for name in ("plugin_json", "mcp_json", "recipes_json")}}
            if observed.get("status") != "found" or observed.get("recipe") != expected:
                raise ValueError()
            self.verify_remote_request(directory, files)
            result = value
        except (SetupError, OSError, TypeError, KeyError, ValueError, RecursionError):
            pass
        finally:
            self.operation_lock.release()
        if result is None:
            status = 400 if failure.startswith("recipe_") and failure.endswith("_rejected") else 409 if failure == "retention_conflict" else 500
            raise SetupError(failure, status)
        return result

    def export_recipe(self, request):
        request = remote_recipe_arguments(request, exporting=True)
        with self.lock:
            self.admit_remote_recipes()
            request_sha256 = digest(encoded(request))
            if request_sha256 in self.recipe_exports:
                if self.recipe_exports[request_sha256] is None:
                    raise SetupError("ambiguous_historical_request", 409)
                return {"job": copy.deepcopy(self.jobs[self.recipe_exports[request_sha256]])}
            return self._export_attempt(request)

    def retry_export(self, request):
        keys(request, ("job_id",))
        with self.lock:
            self.admit_remote_recipes()
            identifier = request["job_id"]
            if not isinstance(identifier, str) or identifier not in self.jobs:
                raise SetupError("unknown_job", 404)
            # Accepted retry replay only observes the same successor, even if
            # a later attempt or external file change has occurred.
            if identifier in self.export_retries:
                return {"job": copy.deepcopy(self.jobs[self.export_retries[identifier]])}
            source = self.jobs[identifier]
            intent = self.export_intents.get(identifier)
            if (source["kind"] != "export" or source["family"] != "remote-recipe"
                    or source["status"] != "unconfirmed" or source.get("recovered")
                    or intent is None or intent["root"] != self.workspace
                    or intent["recovery_anchor"]["id"] != identifier
                    or source.get("recovery_request") != {
                        "id": identifier, "sha256": intent["recovery_anchor"]["sha256"]}):
                raise SetupError("export_retry_unavailable", 409)
            # Recheck current pinned profile/tool and the exact frozen request
            # before allocating a successor. Neither operation invokes the CLI.
            self.remote_recipe_command("export")
            intent = self.checked_export_intent(intent)
            if source.get("request_sha256") != digest(encoded(intent["request"])):
                raise SetupError("export_retry_unavailable", 409)
            return self._export_attempt(copy.deepcopy(intent["request"]), retry_of=identifier)

    def _export_attempt(self, request, retry_of=None):
        # The caller holds the state lock; start_job takes the operation lock
        # before reserving durable inputs or allowing worker dispatch.
        request_sha256 = digest(encoded(request))
        job_id = None

        def reserve(job):
            nonlocal job_id
            job_id = job["id"]
            document_key, document_name, _ = remote_export_document(request)
            directory, frozen = self.freeze_remote_request("export-" + job_id, request,
                                                           {document_name: request[document_key]})
            for name in frozen:
                sync_file(directory, name)
            sync_directory(directory.parent)
            record = {"schema_version": 1, "kind": REMOTE_EXPORT_REQUEST, "job_id": job_id,
                      "workspace": str(self.workspace), "profile_sha256": self.local_recipes_profile_sha256,
                      "request_sha256": request_sha256, "request": request}
            raw = encoded(record)
            with self._retaining_request(job, raw):
                _, intent = load_export_recovery(self.workspace, job_id, digest(raw), self.local_recipes_profile_sha256)
                job["request_sha256"] = request_sha256
                job["recovery_request"] = {"id": job_id, "sha256": digest(raw)}
                self.export_intents[job_id] = intent
                if retry_of is None:
                    self.recipe_exports[request_sha256] = job_id
                else:
                    job["retry_of"] = retry_of
                    self.export_retries[retry_of] = job_id

        def work():
            argv = self.remote_recipe_command("export")
            intent = self.checked_export_intent(self.export_intents[job_id])
            directory = intent["directory"]
            document_key, document_name, document_flag = remote_export_document(request)
            frozen = {"request.json": encoded(request), document_name: request[document_key].encode("utf-8")}
            self.verify_remote_request(directory, frozen)
            destination = directory / "output"
            for key in ("publisher", "name", "version", "digest"):
                argv += ["--" + key, request[key]]
            argv += [document_flag, str(directory / document_name), "--destination", str(destination)]
            code, raw = self._run(argv, capture=True)
            if code:
                raise SetupError("export_unconfirmed", 500)
            value = remote_recipe_envelope(raw, "exported", ("recipe", "selection_digest", "files", "destination"),
                                           optional=("declared_remote_service", "selection_kind"))
            remote_recipe_summary(value["recipe"], identity=request)
            if (not isinstance(value["selection_digest"], str) or not HEX.fullmatch(value["selection_digest"])
                    or value["destination"] != str(destination)):
                raise ValueError()
            keys(value["files"], ("plugin.json", "mcp.json"))
            if {p.name for p in canonical_directory(destination).iterdir()} != set(value["files"]):
                raise ValueError()
            contents = {}
            for name, metadata in value["files"].items():
                keys(metadata, ("sha256", "bytes"))
                data = read_file(destination, name, REMOTE_EXPORT_FILE)
                if type(metadata["bytes"]) is not int or metadata["bytes"] != len(data) or metadata["sha256"] != digest(data):
                    raise ValueError()
                contents[name] = data
            self.verify_remote_request(directory, frozen)
            mcp = decode(contents["mcp.json"])
            keys(mcp, ("$schema", "mcpServers"))
            if (mcp["$schema"] != "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"
                    or not isinstance(mcp["mcpServers"], dict) or len(mcp["mcpServers"]) != 1
                    or not isinstance(decode(contents["plugin.json"]), dict)):
                raise ValueError()
            result = {"scope": "portable_mcp_configuration_only", "recipe": value["recipe"],
                      "selection_digest": value["selection_digest"], "files": value["files"],
                      "destination": str(destination), "plugin_path": str(destination / "plugin.json"),
                      "mcp_path": str(destination / "mcp.json"), "plugin_json": contents["plugin.json"].decode("utf-8"),
                      "mcp_configuration": mcp}
            result.update(remote_service_declaration(value, request, value["selection_digest"], mcp))
            if len(encoded(result)) > REMOTE_EXPORT_RESULT:
                raise ValueError()
            return result

        return {"job": copy.deepcopy(self.start_job("export", "remote-recipe", work, before_start=reserve))}

    def checked_export_intent(self, intent):
        anchor = intent["recovery_anchor"]
        _, observed = load_export_recovery(anchor["root"], anchor["id"], anchor["sha256"],
                                           self.local_recipes_profile_sha256)
        if observed != intent:
            raise SetupError("recovery_request_changed")
        return observed

    def export_readback(self, intent):
        intent = self.checked_export_intent(intent)
        request, destination = intent["request"], intent["destination"]
        argv = self.remote_recipe_command("check-export")
        for key in ("publisher", "name", "version", "digest"):
            argv += ["--" + key, request[key]]
        _, document_name, document_flag = remote_export_document(request)
        argv += [document_flag, str(intent["directory"] / document_name), "--destination", str(destination)]
        code, raw = self._run(argv, capture=True)
        if code:
            raise SetupError("reconciliation_unconfirmed", 500)
        value = remote_recipe_envelope(raw, "inspected", ("recipe", "selection_digest", "destination", "export_check"),
                                       optional=("declared_remote_service", "selection_kind"))
        remote_recipe_summary(value["recipe"], identity=request)
        if not selected_hash(value["selection_digest"]) or value["destination"] != str(destination):
            raise ValueError()
        check = value["export_check"]
        keys(check, ("state", "files", "inventory"))
        states = ("matches", "missing", "mismatch", "unavailable")
        if check["state"] not in states:
            raise ValueError()
        keys(check["files"], ("plugin.json", "mcp.json"))
        keys(check["inventory"], ("state", "complete", "unexpected_entries"))
        inventory = check["inventory"]
        if (inventory["state"] not in states or type(inventory["complete"]) is not bool
                or type(inventory["unexpected_entries"]) is not int or inventory["unexpected_entries"] < 0):
            raise ValueError()
        files = {}
        for name, item in check["files"].items():
            keys(item, ("state", "expected", "observed"))
            if item["state"] not in states:
                raise ValueError()
            for label in ("expected", "observed"):
                metadata = item[label]
                if metadata is None and label == "observed":
                    continue
                keys(metadata, ("sha256", "bytes"))
                if (not selected_hash(metadata["sha256"]) or type(metadata["bytes"]) is not int
                        or not 0 <= metadata["bytes"] <= REMOTE_EXPORT_FILE):
                    raise ValueError()
            files[name] = item["expected"]
        result = {"scope": REMOTE_EXPORT_READBACK, "recipe": value["recipe"],
                  "selection_digest": value["selection_digest"], "destination": str(destination),
                  "export_check": check}
        declaration = remote_service_declaration(value, request, value["selection_digest"])
        result.update(declaration)
        if check["state"] == "matches":
            if (inventory != {"state": "matches", "complete": True, "unexpected_entries": 0}
                    or any(item["state"] != "matches" or item["expected"] != item["observed"] for item in check["files"].values())):
                raise ValueError()
            if {p.name for p in canonical_directory(destination).iterdir()} != set(files):
                raise ValueError()
            contents = {}
            for name, metadata in files.items():
                data = read_file(destination, name, REMOTE_EXPORT_FILE)
                if digest(data) != metadata["sha256"] or len(data) != metadata["bytes"]:
                    raise ValueError()
                contents[name] = data
            mcp = decode(contents["mcp.json"])
            keys(mcp, ("$schema", "mcpServers"))
            if (mcp["$schema"] != "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"
                    or not isinstance(mcp["mcpServers"], dict) or len(mcp["mcpServers"]) != 1
                    or not isinstance(decode(contents["plugin.json"]), dict)):
                raise ValueError()
            result["configuration"] = {"scope": "portable_mcp_configuration_only", "recipe": value["recipe"],
                "selection_digest": value["selection_digest"], "destination": str(destination), "files": files,
                "plugin_path": str(destination / "plugin.json"), "mcp_path": str(destination / "mcp.json"),
                "plugin_json": contents["plugin.json"].decode("utf-8"), "mcp_configuration": mcp,
                **remote_service_declaration(value, request, value["selection_digest"], mcp)}
        self.checked_export_intent(intent)
        self.remote_recipe_command("check-export")
        if len(encoded(result)) > REMOTE_EXPORT_RESULT:
            raise ValueError()
        return result

    def catalog_binding(self):
        return {"scope": CATALOG_SCOPE, "catalog_sha256": self.catalog_sha256,
                "contents_sha256": self.contents_sha256}

    def catalog(self, value):
        keys(value, ())
        with self.lock:
            rows = self.catalog_read()
            return {**self.catalog_binding(), "entries": [
                {k: copy.deepcopy(row[k]) for k in ("identity", "display_name", "description")} for row in rows]}

    def catalog_entry(self, value):
        keys(value, ("publisher", "name", "version"))
        if any(not isinstance(v, str) or not 1 <= len(v) <= 256 for v in value.values()):
            raise SetupError("invalid_request")
        with self.lock:
            rows = self.catalog_read()
            for row in rows:
                if row["identity"] == value:
                    result = {**self.catalog_binding(), "entry": row}
                    if self.routes_sha256 is not None:
                        index = self.routes_read(rows)
                        result.update(routes_sha256=self.routes_sha256,
                                      setup_options=self.route_options(index, row))
                    return result
            raise SetupError("unknown_catalog_entry", 404)

    def routes_read(self, catalog):
        """Verify host-trusted build output, not a second installer selection policy.

        The exporter binds consumed artifacts and maintainer associations. Here
        the declared component files are verified against frozen bytes.
        """
        if self.routes_sha256 is None:
            raise SetupError("catalog_routes_unavailable", 409)
        try:
            raw = read_file(self.bundle, "routes.json", MAX_JSON)
            if digest(raw) != self.routes_sha256:
                raise SetupError("catalog_routes_changed")
            value = decode(raw)
            keys(value, ("schema_version", "catalog_sha256", "runtime_target", "provenance", "components", "routes"))
            if type(value["schema_version"]) is not int or value["schema_version"] not in (1, 2):
                raise SetupError("invalid_catalog_routes")
            if value["catalog_sha256"] != self.catalog_sha256 or encoded(value["runtime_target"]) != encoded(self.runtime_target):
                raise SetupError("catalog_route_binding_mismatch")
            provenance = value["provenance"]
            keys(provenance, ("inventory", "profile_captures"))
            keys(provenance["profile_captures"], FAMILIES)
            for source in [provenance["inventory"], *provenance["profile_captures"].values()]:
                keys(source, ("path", "sha256"))
                relative(source["path"])
                if not isinstance(source["sha256"], str) or not HEX.fullmatch(source["sha256"]):
                    raise SetupError("invalid_catalog_routes")
            components = value["components"]
            if value["schema_version"] == 1:
                keys(components, FAMILIES)
            elif (not isinstance(components, dict) or not set(FAMILIES) <= set(components)
                    or not set(components) <= set(FAMILIES + OFFICIAL_FAMILIES)):
                raise SetupError("invalid_catalog_routes")
            for family, component in components.items():
                if family in OFFICIAL_FAMILIES:
                    self.official_route_component(family, component)
                    continue
                keys(component, ("path", "sha256", "profiles"))
                parts = relative(component["path"])
                profiles = component["profiles"]
                if (len(parts) != 2 or parts[0] != family or not isinstance(component["sha256"], str)
                        or not HEX.fullmatch(component["sha256"]) or not isinstance(profiles, list)
                        or not 1 <= len(profiles) <= 256 or any(not isinstance(p, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", p) for p in profiles)
                        or len(set(profiles)) != len(profiles)):
                    raise SetupError("invalid_catalog_routes")
                if (self.bundle_files.get(component["path"], {}).get("sha256") != component["sha256"]
                        or digest(read_file(self.bundle, component["path"], 64 * 1024 * 1024)) != component["sha256"]):
                    raise SetupError("catalog_route_artifact_mismatch")
            rows, seen = value["routes"], set()
            if not isinstance(rows, list) or len(rows) > 512:
                raise SetupError("invalid_catalog_routes")
            for entry in catalog:
                ids = set()
                for strategy in entry["strategies"]:
                    if (not isinstance(strategy, dict) or not isinstance(strategy.get("id"), str)
                            or not 1 <= len(strategy["id"]) <= 256 or strategy["id"] in ids):
                        raise SetupError("invalid_catalog_routes")
                    ids.add(strategy["id"])
            entries = {tuple(row["identity"][key] for key in ("publisher", "name", "version")): row for row in catalog}
            for route in rows:
                keys(route, ("identity", "strategy_id", "family", "profile"))
                keys(route["identity"], ("publisher", "name", "version"))
                identity = tuple(route["identity"][key] for key in ("publisher", "name", "version"))
                if any(not isinstance(v, str) or not 1 <= len(v) <= 256 for v in identity):
                    raise SetupError("invalid_catalog_routes")
                if (not isinstance(route["strategy_id"], str) or not 1 <= len(route["strategy_id"]) <= 256
                        or not isinstance(route["family"], str) or route["family"] not in components
                        or route["profile"] not in components[route["family"]]["profiles"]):
                    raise SetupError("invalid_catalog_routes")
                key = (*identity, route["strategy_id"])
                entry = entries.get(identity)
                if key in seen or entry is None or sum(s.get("id") == route["strategy_id"] for s in entry["strategies"]) != 1:
                    raise SetupError("invalid_catalog_routes")
                if route["family"] in OFFICIAL_FAMILIES:
                    name = "github" if route["family"] == OFFICIAL_FAMILY else "playwright"
                    if (identity[0] != "meerkat-toolkit" or identity[1] != name
                            or not identity[2].startswith("metadata-sha256:")
                            or not HEX.fullmatch(identity[2].removeprefix("metadata-sha256:"))
                            or route["strategy_id"] != "selected-local-mcp"
                            or next(s for s in entry["strategies"] if s["id"] == route["strategy_id"]).get("kind") != "local_mcp"):
                        raise SetupError("invalid_catalog_routes")
                seen.add(key)
            return value
        except (OSError, TypeError, KeyError, ValueError):
            raise SetupError("invalid_catalog_routes") from None

    def official_route_component(self, family, component):
        """Check a declared separate delivery without enabling or executing it."""
        keys(component, ("path", "sha256", "profiles", "contents_sha256", "installer_sha256"))
        name, profile, path = (("github", "selected-reads", "github/github-mcp-server")
                               if family == OFFICIAL_FAMILY else
                               ("playwright", "isolated-chrome", "playwright/package-lock.json"))
        if (component["path"] != path or component["profiles"] != [profile]
                or any(not isinstance(component[k], str) or not HEX.fullmatch(component[k])
                       for k in ("sha256", "contents_sha256", "installer_sha256"))):
            raise SetupError("invalid_catalog_routes")
        if family not in self.families:
            return
        if (self.official_bundle is None
                or component["contents_sha256"] != self.official_contents_sha256
                or component["installer_sha256"] != self.official_installer_sha256):
            raise SetupError("catalog_route_artifact_mismatch")
        raw = read_file(self.official_bundle, "CONTENTS.json")
        if (digest(raw) != component["contents_sha256"]
                or digest(read_file(self.workspace, "official-installer.py")) != component["installer_sha256"]):
            raise SetupError("catalog_route_artifact_mismatch")
        files = decode(raw)["files"]
        if files[path]["sha256"] != component["sha256"]:
            raise SetupError("catalog_route_artifact_mismatch")
        total = 0
        for member, metadata in files.items():
            if member == "delivery.json" or member.startswith(name + "/"):
                data = read_file(self.official_bundle, member, 64 * 1024 * 1024)
                total += len(data)
                if total > 128 * 1024 * 1024 or digest(data) != metadata["sha256"]:
                    raise SetupError("catalog_route_artifact_mismatch")

    def route_options(self, index, entry):
        options = []
        for strategy in entry["strategies"]:
            route = next((r for r in index["routes"] if r["identity"] == entry["identity"] and r["strategy_id"] == strategy["id"]), None)
            if route is None:
                options.append({"strategy_id": strategy["id"], "status": "manual", "reason": "no_bound_setup_route"})
            else:
                family = self.families.get(route["family"])
                status = "disabled" if family is None else family["status"]
                options.append({**copy.deepcopy(route), "status": status,
                                "artifact": copy.deepcopy({k: index["components"][route["family"]][k] for k in ("path", "sha256")})})
        return options

    def catalog_route(self, value):
        keys(value, ("publisher", "name", "version", "strategy_id", "routes_sha256"))
        if any(not isinstance(v, str) or not 1 <= len(v) <= 256 for v in value.values()):
            raise SetupError("invalid_request")
        if not self.operation_lock.acquire(blocking=False):
            raise SetupError("operation_in_progress", 409)
        try:
            with self.lock:
                catalog = self.catalog_read()
                index = self.routes_read(catalog)
                if value["routes_sha256"] != self.routes_sha256:
                    raise SetupError("catalog_routes_changed", 409)
                identity = {k: value[k] for k in ("publisher", "name", "version")}
                route = next((r for r in index["routes"] if r["identity"] == identity and r["strategy_id"] == value["strategy_id"]), None)
                if route is None:
                    raise SetupError("catalog_route_unavailable", 409)
                state = self.family(route["family"])
                result = {**self.catalog_binding(), "scope": "catalog_setup_handoff", "routes_sha256": self.routes_sha256,
                          "runtime_target": copy.deepcopy(index["runtime_target"]), "route": copy.deepcopy(route),
                          "artifact": {k: index["components"][route["family"]][k] for k in ("path", "sha256")}}
                if state["status"] == "unprepared":
                    return {**result, "status": "preparation_required"}
                if state["status"] != "prepared":
                    raise SetupError("catalog_route_preparation_unconfirmed", 409)
                return {**result, "status": "prepared", "help": self.help_locked(route["family"], route["profile"])}
        finally:
            self.operation_lock.release()

    def successor(self, value):
        """Propose historical values for explicit current review, never install."""
        keys(value, ("prior_request_sha256",))
        self.admit_setup()
        if self.prior_request_sha256 is None:
            raise SetupError("prior_selection_unavailable", 409)
        if value["prior_request_sha256"] != self.prior_request_sha256:
            raise SetupError("prior_request_changed", 409)
        if not self.operation_lock.acquire(blocking=False):
            raise SetupError("operation_in_progress", 409)
        try:
            raw = read_file(self.workspace, "prior-request.json", MAX_BODY * 2)
            if digest(raw) != self.prior_request_sha256:
                raise SetupError("prior_request_changed", 409)
            record = prior_request(raw)
            selection = record["selection"]
            state = self.family(selection["family"])
            result = {"scope": "successor_configuration_candidate", "prior_request_sha256": self.prior_request_sha256,
                      "prior_configuration_sha256": record["configuration_sha256"],
                      "reported_prior_contents_sha256": record["contents_sha256"],
                      "current_contents_sha256": self.input_binding(selection["family"])[0],
                      "family": selection["family"], "profile": selection["profile"]}
            if state["status"] == "unprepared":
                return {**result, "status": "preparation_required"}
            if state["status"] != "prepared":
                raise SetupError("prior_selection_preparation_unconfirmed", 409)
            if not any(profile["id"] == selection["profile"] for profile in state["profiles"]):
                return {**result, "status": "profile_unavailable"}
            help_value = self.help_locked(selection["family"], selection["profile"])
            projection = prior_projection(selection, help_value)
            return {**result, "status": "review_required", "help": help_value, **projection}
        finally:
            self.operation_lock.release()

    def selected_tools(self):
        if self.selected is None:
            raise SetupError("selected_setup_unavailable", 409)
        tools = {}
        for name, sha in self.selected_binding["tools"].items():
            path = self.workspace / "selected-tools" / (name + (".py" if name == "installer" else ""))
            if digest(selected_read(self.workspace, "selected-tools/" + path.name, SELECTED_FILE)) != sha:
                raise SetupError("selected_tool_changed", 409)
            tools[name] = str(path)
        return tools

    def selected_summary(self, identifier, value):
        effect, plan = value["effect"], value["plan"]
        result = {"scope": SELECTED_PROPOSAL, "validation_id": identifier, "effect_sha256": value["effect_sha256"],
                  "profile": effect["request"]["profile"], "package": plan["package"], "component": plan["component"],
                  "selection_sha256": effect["request"]["selection_sha256"], **selected_identity(plan, effect["request"]),
                  "files": effect["files"], "destination": effect["destination"],
                  "entrypoint": plan["entrypoint"], "mcp_name": plan["mcp_name"]}
        if "declared_service" in plan:
            result["declared_service"] = copy.deepcopy(plan["declared_service"])
        if "plugin_source" in effect["request"]:
            result["plugin_source"] = copy.deepcopy(effect["request"]["plugin_source"])
        if "acquired_source" in effect["request"]:
            result["acquired_source"] = copy.deepcopy(effect["request"]["acquired_source"])
        if effect["request"]["profile"] == "mcpb":
            result["configuration_sha256"] = effect["request"]["configuration_sha256"]
        if "build_review" in effect:
            result["build"] = copy.deepcopy(effect["build_review"])
        if "retry_of" in value:
            result.update(retry_of=value["retry_of"], job_id=value["job_id"])
        if len(encoded(result)) > MAX_JSON:
            raise SetupError("selected_output_too_large")
        return copy.deepcopy(result)

    def selected_validator(self, argv, profile, snapshot, documents):
        """Attribute only exact canonical refusals to unchanged frozen inputs."""
        try:
            self._check_stop()
            capture = getattr(self.runner, "run_selected_validator", None)
            kwargs = {"inspection_timeout": MCPB_INSPECTION_TIMEOUT} if profile == "mcpb" else {}
            if capture is None:
                # Existing custom runners retain their original two-value API.
                code, raw = self._run(argv, capture=True, **kwargs)
                diagnostic = b""
            else:
                code, raw, diagnostic = capture(argv, **kwargs)
            self._check_stop()
            if type(code) is not int or type(raw) is not bytes or type(diagnostic) is not bytes:
                raise SetupError("selected_validation_unavailable")
            if code == 0 and not diagnostic:
                return code, raw
            for name, data in documents.items():
                limit = SELECTED_FILE if name == "artifact.mcpb" else SELECTED_DOCUMENT
                if selected_read(snapshot, name, limit) != data:
                    raise SetupError("selected_validation_unavailable")
            self.selected_tools()
        except SetupError as error:
            if error.code == "operation_stopped":
                raise
            raise SetupError("selected_validation_unavailable") from None
        except Exception:
            raise SetupError("selected_validation_unavailable") from None
        if type(code) is int and code == 1 and raw == b"":
            if profile == "mcpb":
                for reason in MCPB_REJECTION_HELP:
                    if diagnostic == (reason + MCPB_REJECTION_SUFFIX).encode("ascii"):
                        raise SetupError("selected_mcpb_" + reason)
            else:
                for reason in ("invalid_package", "invalid_selection", "unsupported_" + profile + "_selection"):
                    if diagnostic == (reason + "\n").encode("ascii"):
                        raise SetupError("selected_" + reason)
        raise SetupError("selected_validation_unavailable")

    def inspect_package(self, request):
        keys(request, ("package_json",))
        document = selected_inline_document(request["package_json"])
        if len(encoded(request)) > MAX_BODY:
            raise SetupError("invalid_selected_request")
        with self.lock:
            self.admit_setup()
            tools = self.selected_tools()
            if "package_inspector" not in tools:
                raise SetupError("package_inspection_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            if self.selected_attempts >= 128:
                raise SetupError("new_workspace_required", 409)
            self.selected_attempts += 1
            run = getattr(self.runner, "run_compose", None)
            if not callable(run):
                # Custom runners must opt into this existing stdin contract;
                # never bypass their run override with a separate subprocess.
                raise SetupError("package_inspection_unavailable")
            with tempfile.TemporaryDirectory(prefix="package-inspection-", dir=self.workspace / "tmp") as temporary:
                path = Path(temporary) / "package.json"
                write_new(path, document)
                try:
                    code, raw = run([tools["package_inspector"], "integration-package"],
                                    input_file=path, input_sha256=digest(document),
                                    output_limit=PACKAGE_INSPECTION_OUTPUT, timeout=INSPECTION_TIMEOUT)
                    if selected_read(path.parent, path.name, SELECTED_INLINE_DOCUMENT) != document:
                        raise ValueError()
                    self.selected_tools()
                except Exception:
                    raise SetupError("package_inspection_unavailable") from None
                return package_inspection(raw, digest(document), code)
        finally:
            self.operation_lock.release()

    def inspect_schema(self, request):
        """Hash supplied schema text without asserting its source or meaning."""
        keys(request, ("schema_json",))
        try:
            document = selected_inline_document(request["schema_json"])
            if len(encoded(request)) > MAX_BODY or document.startswith(b"\xef\xbb\xbf"):
                raise ValueError()
            value = decode(document)
            if type(value) not in (dict, bool):
                raise ValueError()
            # A bounded JSON syntax check, not JSON Schema validation. In
            # particular, references remain inert and no schema is fetched.
            pending, nodes = [(value, 0)], 0
            while pending:
                item, depth = pending.pop()
                nodes += 1
                if nodes > 4096 or depth > 32:
                    raise ValueError()
                if type(item) is dict:
                    pending.extend((child, depth + 1) for child in item.values())
                elif type(item) is list:
                    pending.extend((child, depth + 1) for child in item)
            encoded(value)  # Reject nonfinite parsed numbers; do not hash this encoding.
        except (SetupError, TypeError, ValueError, UnicodeError, RecursionError):
            raise SetupError("invalid_schema_evidence") from None
        with self.lock:
            self.admit_setup()
            if "package_inspector" not in self.selected_tools():
                raise SetupError("schema_inspection_unavailable", 409)
            return {"schema_version": 1, "scope": "supplied_schema_evidence_only",
                "schema_digest": digest(document), "bytes": len(document),
                "hash_algorithm": "sha256", "byte_convention": "exact_supplied_utf8",
                "runtime_observation": "not_established", "schema_validation": "not_performed",
                "notice": "This digest identifies the exact supplied text, including whitespace. "
                    "It does not establish schema provenance, runtime equality, readiness or authority. "
                    "Retain the evidence source and any host observation's hashing convention; "
                    "do not replace an existing host convention with this one."}

    def plugin_source_bytes(self, reference):
        """Read this session's retained observation, never reopen the imported files."""
        plugin_source_reference(reference)
        identifier = reference["inspection_id"]
        if self.plugin_inspections.get(identifier) != reference["inspection_sha256"]:
            raise SetupError("plugin_source_unavailable", 409)
        raw = selected_read(self.workspace, "plugin-inspections/" + identifier + ".json", MAX_JSON)
        if digest(raw) != reference["inspection_sha256"]:
            raise SetupError("plugin_source_changed", 409)
        return raw

    def plugin_source(self, request):
        """Read one selected job's retained evidence, including in recovery mode."""
        keys(request, ("job_id",))
        with self.lock:
            self.admit()
            identifier = request["job_id"]
            if (self.selected is None or self.selected.get("plugin_inspection") is not True
                    or not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier)):
                raise SetupError("plugin_source_unavailable", 409)
            job = copy.deepcopy(self.jobs.get(identifier))
            intent = copy.deepcopy(self.install_intents.get(identifier))
            if (job is None or intent is None or intent.get("kind") != "selected"
                    or job["kind"] != "install" or job["family"] != intent["family"]):
                raise SetupError("plugin_source_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            original, retained = load_selected_recovery(intent["root"], identifier,
                job["recovery_request"]["sha256"], self.selected_binding)
            if any(retained[key] != intent[key] for key in (
                    "kind", "family", "root", "validation_id", "effect_sha256", "destination")):
                raise ValueError()
            path, effect, _ = selected_proposal(intent["root"], intent["validation_id"],
                intent["effect_sha256"], self.selected_binding)
            reference = effect["request"].get("plugin_source")
            if reference is None:
                raise SetupError("plugin_source_unavailable", 409)
            raw = selected_read(path, "plugin-source.json", MAX_JSON)
            if digest(raw) != reference["inspection_sha256"]:
                raise ValueError()
            source = decode(raw)
            keys(source, ("schema_version", "scope", "staged_id", "inspector_sha256", "report"))
            if (type(source["schema_version"]) is not int or source["schema_version"] != 1
                    or source["scope"] != PLUGIN_SOURCE_SCOPE or not selected_name(source["staged_id"])
                    or source["inspector_sha256"] != self.selected_binding["tools"].get("package_inspector")):
                raise ValueError()
            report = plugin_files_output(encoded(source["report"]), 0,
                [row["path"] for row in source["report"]["files"]])
            if (load_selected_recovery(intent["root"], identifier, job["recovery_request"]["sha256"],
                    self.selected_binding) != (original, retained)
                    or selected_read(path, "plugin-source.json", MAX_JSON) != raw):
                raise ValueError()
            return {**report, "plugin_source": copy.deepcopy(reference), "job_id": identifier}
        except SetupError as error:
            if error.code == "plugin_source_unavailable":
                raise
            raise SetupError("plugin_source_changed", 409) from None
        except (OSError, KeyError, TypeError, ValueError, RecursionError):
            raise SetupError("plugin_source_changed", 409) from None
        finally:
            self.operation_lock.release()

    def inspect_plugin(self, request):
        inventory = isinstance(request, dict) and "inventory" in request
        plugin_inventory_request(request) if inventory else plugin_files_request(request)
        with self.lock:
            self.admit_setup()
            tools = self.selected_tools()
            if self.selected.get("plugin_inspection") is not True or "package_inspector" not in tools:
                raise SetupError("plugin_inspection_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            if self.selected_attempts >= 128:
                raise SetupError("new_workspace_required", 409)
            self.selected_attempts += 1
            run = getattr(self.runner, "run_compose", None)
            if not callable(run):
                raise SetupError("plugin_inspection_unavailable")
            # The canonical reader opens this host-owned root once, then reads
            # the chosen child below that handle. Do not resolve a child here.
            settings = {"root": self.selected["staging_root"], "directory": "./" + request["staged_id"],
                        **({"inventory": request["inventory"]} if inventory else {"files": request["files"]})}
            document = encoded(settings)
            with tempfile.TemporaryDirectory(prefix="plugin-inspection-", dir=self.workspace / "tmp") as temporary:
                path = Path(temporary) / "request.json"
                write_new(path, document)
                try:
                    code, raw = run([tools["package_inspector"], "agent-plugin-files"],
                                    input_file=path, input_sha256=digest(document),
                                    output_limit=PACKAGE_INSPECTION_OUTPUT, timeout=INSPECTION_TIMEOUT)
                    if selected_read(path.parent, path.name, MAX_BODY) != document:
                        raise ValueError()
                    self.selected_tools()
                except Exception:
                    raise SetupError("plugin_inspection_unavailable") from None
                if inventory:
                    # Names are observations only, never retained source provenance.
                    return plugin_inventory_output(raw, code, request["inventory"])
                report = plugin_files_output(raw, code, request["files"])
            retained = encoded({"schema_version": 1, "scope": PLUGIN_SOURCE_SCOPE,
                                "staged_id": request["staged_id"],
                                "inspector_sha256": self.selected_binding["tools"]["package_inspector"],
                                "report": report})
            expected = digest(retained)
            identifier = next((key for key, sha in self.plugin_inspections.items() if sha == expected), None)
            if identifier is None:
                identifier = secrets.token_hex(16)
                write_new(self.workspace / "plugin-inspections" / (identifier + ".json"), retained)
                self.plugin_inspections[identifier] = expected
            reference = {"inspection_id": identifier, "inspection_sha256": expected}
            if self.plugin_source_bytes(reference) != retained:
                raise SetupError("plugin_source_changed", 409)
            return {**report, "plugin_source": reference}
        finally:
            self.operation_lock.release()

    def inspect_mcpb(self, request):
        acquired = isinstance(request, dict) and ("job_id" in request or "receipt_sha256" in request)
        if acquired:
            keys(request, ("job_id", "receipt_sha256", "files"))
            if (not isinstance(request["job_id"], str) or not REQUEST_ID.fullmatch(request["job_id"])
                    or not selected_hash(request["receipt_sha256"])):
                raise SetupError("invalid_mcpb_inspection_request")
            files = mcpb_files_request({"staged_id": request["job_id"],
                "artifact_sha256": request["receipt_sha256"], "files": request["files"]})
        else:
            files = mcpb_files_request(request)
        with self.lock:
            self.admit() if acquired else self.admit_setup()
            tools = self.selected_tools()
            if self.selected.get("mcpb_inspection") is not True or "package_inspector" not in tools:
                raise SetupError("mcpb_inspection_unavailable", 409)
            if acquired:
                if not self.allow_acquired_inspection:
                    raise SetupError("mcpb_inspection_unavailable", 409)
                job = copy.deepcopy(self.jobs.get(request["job_id"]))
                if (not job or job["kind"] != "acquire" or job["family"] != "selected-mcpb"
                        or job["id"] not in self.acquisition_intents):
                    raise SetupError("mcpb_inspection_unavailable", 409)
                if self.recovery_only:
                    observation = self.observations.get(job["id"], {})
                    if (not job.get("recovered") or observation.get("status") != "matched"
                            or observation.get("scope") != ACQUISITION_READBACK
                            or observation.get("job_id") != job["id"]):
                        raise SetupError("mcpb_inspection_unavailable", 409)
                    expected_result = copy.deepcopy(observation.get("result", {}))
                else:
                    if job["status"] != "succeeded":
                        raise SetupError("mcpb_inspection_unavailable", 409)
                    expected_result = job["result"]
                if expected_result.get("receipt_sha256") != request["receipt_sha256"]:
                    raise SetupError("mcpb_inspection_unavailable", 409)
                intent = copy.deepcopy(self.acquisition_intents[job["id"]])
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            if self.selected_attempts >= 128:
                raise SetupError("new_workspace_required", 409)
            self.selected_attempts += 1
            run = getattr(self.runner, "run_compose", None)
            if not callable(run):
                raise SetupError("mcpb_inspection_unavailable")
            def observe():
                anchor = intent["recovery_anchor"]
                record, current = load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"])
                if (current != intent or record["host"]["selected_profile_sha256"] != self.selected_binding["profile_sha256"]
                        or record["staging_root"] != self.selected["staging_root"]):
                    raise ValueError()
                directory = anchor["root"] / "acquisitions" / job["id"]
                if selected_read(directory, "request.json", MAX_BODY) != encoded(record["arguments"]):
                    raise ValueError()
                result = self.acquisition_modules["mcpb"].readback(acquisition_io(), record, directory)
                if (result != expected_result or result["staged_id"] != job["id"]
                        or result["primary"]["sha256"] != record["arguments"]["artifact_sha256"]
                        or load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"]) != (record, current)
                        or selected_read(directory, "request.json", MAX_BODY) != encoded(record["arguments"])):
                    raise ValueError()
                return record, result
            if acquired:
                record, result = observe()
                staged_id, artifact_sha256 = result["staged_id"], result["primary"]["sha256"]
            else:
                staged_id, artifact_sha256 = request["staged_id"], request["artifact_sha256"]
            # Only the canonical archive owner opens the staged bundle. The
            # returned hash is evidence, not an install or activation grant.
            document = encoded({"root": self.selected["staging_root"],
                "directory": "./" + staged_id,
                "artifact_sha256": artifact_sha256, "files": files})
            with tempfile.TemporaryDirectory(prefix="mcpb-inspection-", dir=self.workspace / "tmp") as temporary:
                path = Path(temporary) / "request.json"
                write_new(path, document)
                try:
                    code, raw = run([tools["package_inspector"], "mcpb-archive"],
                        input_file=path, input_sha256=digest(document),
                        output_limit=PACKAGE_INSPECTION_OUTPUT, timeout=MCPB_INSPECTION_TIMEOUT)
                    if selected_read(path.parent, path.name, MAX_BODY) != document:
                        raise ValueError()
                    self.selected_tools()
                    if acquired and observe() != (record, result):
                        raise ValueError()
                except Exception:
                    raise SetupError("mcpb_inspection_unavailable") from None
                projection = mcpb_files_output(raw, code, artifact_sha256, files)
                return ({**projection, "job_id": job["id"], "receipt_sha256": request["receipt_sha256"]}
                        if acquired else projection)
        except (OSError, ValueError, KeyError, TypeError):
            raise SetupError("mcpb_inspection_unavailable") from None
        finally:
            self.operation_lock.release()

    def inspect_acquired(self, request):
        if not isinstance(request, dict):
            raise SetupError("invalid_acquired_inspection_request")
        keys(request, ("job_id", "receipt_sha256", *(("archive_sha256",) if "archive_sha256" in request else ()),
                       *(("after",) if "after" in request else ("member", "offset"))))
        if (not isinstance(request["job_id"], str) or not REQUEST_ID.fullmatch(request["job_id"])
                or not selected_hash(request["receipt_sha256"])
                or "archive_sha256" in request and not selected_hash(request["archive_sha256"])):
            raise SetupError("invalid_acquired_inspection_request")
        listing = "after" in request
        path = request["after"] if listing else request["member"]
        if ((path is not None or not listing) and (type(path) is not str or not 1 <= len(path) <= 1024)
                or not listing and (type(request["offset"]) is not int or not 0 <= request["offset"] <= 1024 * 1024)):
            raise SetupError("invalid_acquired_inspection_request")
        with self.lock:
            self.admit()
            if not self.allow_acquired_inspection:
                raise SetupError("acquired_inspection_unavailable", 409)
            job = copy.deepcopy(self.jobs.get(request["job_id"]))
            if (not job or job["kind"] != "acquire" or job["family"] not in ("selected-python", "selected-node")
                    or job["id"] not in self.acquisition_intents):
                raise SetupError("acquired_inspection_unavailable", 409)
            if self.recovery_only or job.get("recovered"):
                observation = self.observations.get(job["id"], {})
                if (not job.get("recovered") or observation.get("status") != "matched"
                        or observation.get("scope") != ACQUISITION_READBACK
                        or observation.get("job_id") != job["id"]):
                    raise SetupError("acquired_inspection_unavailable", 409)
                expected_result = copy.deepcopy(observation.get("result", {}))
            else:
                if job["status"] != "succeeded":
                    raise SetupError("acquired_inspection_unavailable", 409)
                expected_result = job["result"]
            if expected_result.get("receipt_sha256") != request["receipt_sha256"]:
                raise SetupError("acquired_inspection_unavailable", 409)
            if not self.operation_lock.acquire(False):
                raise SetupError("operation_in_progress", 409)
        try:
            if self.selected_attempts >= 128:
                raise SetupError("new_workspace_required", 409)
            self.selected_attempts += 1
            intent = copy.deepcopy(self.acquisition_intents[job["id"]])
            anchor = intent["recovery_anchor"]
            kind = job["family"].removeprefix("selected-")
            owner = getattr(self, kind.replace("-", "_") + "_acquisition")
            def verify_python():
                # Readback proves old payloads, not the executable we enter now.
                runtime = Path(self.python)
                if self.recovery_only:
                    expected = self.acquired_inspection_python_raw
                else:
                    expected = owner.python_raw if kind == "python" else owner.inputs[owner.python]
                    if runtime != owner.python:
                        raise ValueError()
                if expected is None or selected_read(runtime.parent, runtime.name, len(expected)) != expected:
                    raise ValueError()
            verify_python()
            def observe():
                record, current = load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"])
                if current != intent or record["host"]["selected_profile_sha256"] != self.selected_binding["profile_sha256"]:
                    raise ValueError()
                directory = anchor["root"] / "acquisitions" / job["id"]
                if selected_read(directory, "request.json", MAX_BODY) != encoded(record["arguments"]):
                    raise ValueError()
                result = self.acquisition_modules[kind].readback(acquisition_io(), record, directory)
                if (result != expected_result
                        or load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"]) != (record, current)
                        or selected_read(directory, "request.json", MAX_BODY) != encoded(record["arguments"])):
                    raise ValueError()
                return record, result
            tools = self.selected_tools()
            worker = self.workspace / "acquired-inspection.py"
            if selected_read(self.workspace, worker.name, SELECTED_DOCUMENT) != self.acquired_inspection_raw:
                raise ValueError()
            record, result = observe()
            primary = result["primary"]
            selected_sha = request.get("archive_sha256", primary["sha256"])
            archives = [row for row in result["artifacts"] if row["sha256"] == selected_sha]
            if len(archives) != 1:
                raise ValueError()
            archive = archives[0]
            is_primary = archive == primary
            filename = archive["filename"] if kind == "python" else archive["url"].rsplit("/", 1)[-1]
            relative = ("artifact.whl" if kind == "python" else "artifact.tgz") if is_primary else (
                "closure/" + filename if kind == "python" else "closure/archive-" + selected_sha)
            if result["files"].get(relative) != selected_sha:
                raise ValueError()
            settings = {"installer": tools["installer"], "installer_sha256": self.selected_binding["tools"]["installer"],
                        "artifact": str(Path(record["staging_root"]) / job["id"] / relative),
                        "archive_sha256": selected_sha, "archive_role": "primary" if is_primary else "dependency", "profile": kind,
                        "filename": filename,
                        "selection": {key: request[key] for key in (("after",) if listing else ("member", "offset"))}}
            raw = encoded(settings)
            with tempfile.TemporaryDirectory(prefix="acquired-inspection-", dir=self.workspace / "tmp") as temporary:
                input_file = Path(temporary) / "input.json"
                write_new(input_file, raw)
                code, output = self.runner.run_compose([self.python, "-I", "-S", "-B", str(worker)],
                    input_file=input_file, input_sha256=digest(raw), output_limit=PACKAGE_INSPECTION_OUTPUT, timeout=INSPECTION_TIMEOUT)
                if selected_read(input_file.parent, input_file.name, MAX_BODY) != raw:
                    raise ValueError()
            if observe() != (record, result) or selected_read(self.workspace, worker.name, SELECTED_DOCUMENT) != self.acquired_inspection_raw:
                raise ValueError()
            verify_python()
            self.selected_tools()
            value = acquired_inspection_output(output, code, settings)
            return {**value, "job_id": job["id"], "receipt_sha256": request["receipt_sha256"]}
        except SetupError as error:
            if error.code in ("new_workspace_required", "acquired_member_unreadable"):
                raise
            raise SetupError("acquired_inspection_unavailable") from None
        except Exception:
            raise SetupError("acquired_inspection_unavailable") from None
        finally:
            self.operation_lock.release()

    def selected_acquired_source(self, request):
        """Reobserve one live acquisition under the selected operation lock."""
        self._check_stop()
        try:
            reference = request["acquired_source"]
            kind = request["profile"]
            job = copy.deepcopy(self.jobs.get(reference["job_id"]))
            # Derived acquisition identity remains distinct, while its normal
            # installation profile and canonical validator are still Node.
            if kind == "node" and job and job.get("family") == "selected-node-prebuilt":
                kind = "node-prebuilt"
            intent = copy.deepcopy(self.acquisition_intents.get(reference["job_id"]))
            if (self.recovery_only or not job or job["kind"] != "acquire"
                    or job["family"] != "selected-" + kind or job["status"] != "succeeded"
                    or job.get("recovered") or job["id"] != request["staged_id"] or not intent
                    or intent["root"] != self.workspace
                    or job["result"].get("receipt_sha256") != reference["receipt_sha256"]):
                raise ValueError()
            anchor = intent["recovery_anchor"]
            record, current = load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"])
            if (current != intent or record["staging_root"] != self.selected["staging_root"]
                    or record["host"]["selected_profile_sha256"] != self.selected_binding["profile_sha256"]
                    or digest(selected_read(self.workspace, "selected-host-profile.json", MAX_BODY))
                    != self.selected_binding["profile_sha256"]
                    or self.selected_binding["runtimes"] != {"python": self.python,
                        "node": self.selected_node, "npm_cli": self.selected_npm_cli}):
                raise ValueError()
            owner = getattr(self, kind.replace("-", "_") + "_acquisition")
            if owner is None or Path(self.python) != owner.python or (kind in ("node", "node-prebuilt") and Path(self.selected_node) != owner.node):
                raise ValueError()
            owner.verify()
            self.selected_tools()
            directory = self.workspace / "acquisitions" / job["id"]
            if selected_read(directory, "request.json", MAX_BODY) != encoded(record["arguments"]):
                raise ValueError()
            result = self.acquisition_modules[kind].readback(acquisition_io(), record, directory)
            if (result != job["result"]
                    or load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"]) != (record, current)
                    or selected_read(directory, "request.json", MAX_BODY) != encoded(record["arguments"])):
                raise ValueError()
        except (SetupError, OSError, KeyError, TypeError, ValueError, AttributeError):
            raise SetupError("selected_acquired_source_unavailable", 409) from None
        self._check_stop()
        return result

    def selected_check(self, request):
        request, documents = selected_check_request(request)
        document_flag, document_name, _ = selected_document(request)
        with self.lock:
            self.admit_setup()
            tools = self.selected_tools()
            if request["profile"] not in self.selected["validators"]:
                raise SetupError("selected_profile_unavailable", 409)
            # Exact retained checks are observations of this same Controller.
            # They may replay while its worker owns the effect lock, but only
            # after the current host and every selected byte are revalidated.
            staging = canonical_directory(Path(self.selected["staging_root"]))
            acquired = None
            if "acquired_source" in request:
                acquired = self.selected_acquired_source(request)
                documents[document_name] = materialize_acquired_source(
                    documents[document_name], request["component"], acquired, request.get("build_plan_sha256"))
                request["selection_sha256"] = digest(documents[document_name])
            if "build_plan_sha256" in request and "node_build" not in tools:
                raise SetupError("selected_profile_unavailable", 409)
            # Read from the host-fixed root through no-follow child traversal.
            prefix = request["staged_id"] + "/"
            mcpb = request["profile"] == "mcpb"
            if documents is None:
                documents = {"package.json": selected_read(staging, prefix + "package.json", SELECTED_DOCUMENT),
                             document_name: selected_read(staging, prefix + document_name, SELECTED_DOCUMENT)}
                if mcpb:
                    documents["configuration.json"] = selected_read(staging, prefix + "configuration.json", SELECTED_DOCUMENT)
            if (digest(documents["package.json"]) != request["package_sha256"]
                    or digest(documents[document_name]) != request["selection_sha256"]
                    or (mcpb and digest(documents["configuration.json"]) != request["configuration_sha256"])):
                raise SetupError("selected_source_changed", 409)
            if "build_plan_sha256" in request:
                if "closure/node-build-plan" not in documents:
                    documents["closure/node-build-plan"] = selected_read(staging, prefix + "closure/node-build-plan", SELECTED_INLINE_DOCUMENT)
                if digest(documents["closure/node-build-plan"]) != request["build_plan_sha256"]:
                    raise SetupError("selected_source_changed", 409)
            if "plugin_source" in request:
                documents["plugin-source.json"] = self.plugin_source_bytes(request["plugin_source"])
            for identifier, value in self.validations.items():
                if value.get("kind") == "selected" and "retry_of" not in value and value["effect"]["request"] == request:
                    _, effect, _ = selected_proposal(self.workspace, identifier, value["effect_sha256"], self.selected_binding)
                    for name, sha in effect["files"].items():
                        data = documents[name] if name in documents else selected_read(staging, prefix + name, SELECTED_FILE)
                        if digest(data) != sha:
                            raise SetupError("selected_source_changed", 409)
                    if acquired is not None and self.selected_acquired_source(request) != acquired:
                        raise SetupError("selected_acquired_source_unavailable", 409)
                    return self.selected_summary(identifier, value)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            if self.selected_attempts >= 128 or len(self.validations) >= 128:
                raise SetupError("new_workspace_required", 409)
            self.selected_attempts += 1
            identifier = secrets.token_hex(16)
            snapshot = self.workspace / "selections" / identifier
            snapshot.mkdir(mode=0o700)
            if mcpb:
                # The same pinned owner must inspect the archive before a plan
                # exists. Freeze it first; bind its digest to the returned plan.
                documents["artifact.mcpb"] = selected_read(staging, prefix + "artifact.mcpb", SELECTED_FILE)
            else:
                (snapshot / "closure").mkdir(mode=0o700)
            for name, data in documents.items():
                write_new(snapshot / name, data)
            argv = (self.mcpb_command(tools["mcpb"], "check", snapshot, request, snapshot / "inspection") if mcpb else
                    [tools[request["profile"]], "--package", str(snapshot / "package.json"),
                     document_flag, str(snapshot / document_name), "--component", request["component"]])
            code, raw = self.selected_validator(argv, request["profile"], snapshot, documents)
            if code:
                raise SetupError("selected_validation_unavailable")
            plan = selected_plan(raw, request)
            files = selected_files(plan, request)
            total = sum(map(len, documents.values()))
            for name, sha in files.items():
                if name in documents:
                    if digest(documents[name]) != sha:
                        raise SetupError("selected_source_changed", 409)
                    continue
                data = selected_read(staging, prefix + name, SELECTED_FILE)
                total += len(data)
                if total > SELECTED_TOTAL or digest(data) != sha:
                    raise SetupError("selected_source_changed", 409)
                write_new(snapshot / name, data)
            if acquired is not None and self.selected_acquired_source(request) != acquired:
                raise SetupError("selected_acquired_source_unavailable", 409)
            plan_raw = encoded(plan)
            write_new(snapshot / "plan.json", plan_raw)
            effect = {"schema_version": 1, "kind": SELECTED_PROPOSAL, "request": copy.deepcopy(request),
                      "host": copy.deepcopy(self.selected_binding), "destination": str(self.workspace / "installations" / identifier),
                      "files": files, "plan_sha256": digest(plan_raw)}
            if "build_plan_sha256" in request:
                code, raw = self._run([self.python, "-I", "-B", tools["installer"],
                    "--review-node-build", str(snapshot), "--node", self.selected_node,
                    "--node-build-profile", tools["node_build"],
                    "--node-build-profile-sha256", self.selected_binding["tools"]["node_build"]], capture=True)
                if code:
                    raise SetupError("selected_build_review_unavailable", 409)
                review = selected_build_review(decode(raw), request, self.selected_binding)
                write_new(snapshot / "build-review.json", encoded(review))
                effect["build_review"] = review
            effect_raw = encoded(effect)
            write_new(snapshot / "effect.json", effect_raw)
            value = {"kind": "selected", "effect": effect, "plan": plan,
                     "effect_sha256": digest(effect_raw), "job_id": None}
            selected_proposal(self.workspace, identifier, value["effect_sha256"], self.selected_binding)
            self.selected_tools()
            summary = self.selected_summary(identifier, value)
            with self.lock:
                self.validations[identifier] = value
            return summary
        finally:
            self.operation_lock.release()

    def selected_retry(self, request):
        """Propose fresh output for one failed local install; never install it."""
        keys(request, ("job_id",))
        with self.lock:
            self.admit_setup()
            if self.selected is None:
                raise SetupError("selected_setup_unavailable", 409)
            identifier = request["job_id"]
            if not isinstance(identifier, str) or identifier not in self.jobs:
                raise SetupError("unknown_job", 404)
            if identifier in self.selected_retries:
                successor = self.selected_retries[identifier]
                return self.selected_summary(successor, self.validations[successor])
            source = self.jobs[identifier]
            intent = self.install_intents.get(identifier)
            value = self.validations.get(intent.get("validation_id")) if intent else None
            if (source["kind"] != "install" or source["status"] != "unconfirmed" or source.get("recovered")
                    or intent is None or intent.get("kind") != "selected" or intent["root"] != self.workspace
                    or intent["family"] != source["family"] or value is None or value.get("kind") != "selected"
                    or value["job_id"] != identifier or value["effect_sha256"] != intent["effect_sha256"]
                    or source.get("recovery_request", {}).get("id") != identifier):
                raise SetupError("selected_retry_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            self.selected_tools()
            _, retained = load_selected_recovery(self.workspace, identifier,
                source["recovery_request"]["sha256"], self.selected_binding)
            if any(retained[key] != intent[key] for key in ("kind", "family", "root", "validation_id", "effect_sha256", "destination")):
                raise SetupError("selected_retry_unavailable", 409)
            old_path, effect, plan = selected_proposal(self.workspace, intent["validation_id"], intent["effect_sha256"], self.selected_binding)
            if effect["request"]["profile"] not in self.selected["validators"]:
                raise SetupError("selected_profile_unavailable", 409)
            if self.selected_attempts >= 128 or len(self.validations) >= 128:
                raise SetupError("new_workspace_required", 409)
            # Charge before allocation, including attempts that leave partial
            # snapshots. Never cache a partial proposal or share mutable files.
            self.selected_attempts += 1
            successor = secrets.token_hex(16)
            snapshot = self.workspace / "selections" / successor
            snapshot.mkdir(mode=0o700)
            for name, expected in effect["files"].items():
                data = selected_read(old_path, name, SELECTED_DOCUMENT if name in ("package.json", "activation.json", "declaration.json", "configuration.json") else SELECTED_FILE)
                if digest(data) != expected:
                    raise SetupError("selected_proposal_changed", 409)
                (snapshot / name).parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                write_new(snapshot / name, data)
            if "plugin_source" in effect["request"]:
                write_new(snapshot / "plugin-source.json", selected_read(old_path, "plugin-source.json", MAX_JSON))
            plan_raw = selected_read(old_path, "plan.json", MAX_JSON)
            if digest(plan_raw) != effect["plan_sha256"]:
                raise SetupError("selected_proposal_changed", 409)
            write_new(snapshot / "plan.json", plan_raw)
            if "build_review" in effect:
                write_new(snapshot / "build-review.json", selected_read(old_path, "build-review.json", MAX_JSON))
            effect["destination"] = str(self.workspace / "installations" / successor)
            effect_raw = encoded(effect)
            write_new(snapshot / "effect.json", effect_raw)
            fresh = {"kind": "selected", "effect": effect, "plan": plan,
                     "effect_sha256": digest(effect_raw), "job_id": None, "retry_of": identifier}
            selected_proposal(self.workspace, successor, fresh["effect_sha256"], self.selected_binding)
            load_selected_recovery(self.workspace, identifier, source["recovery_request"]["sha256"], self.selected_binding)
            self.selected_tools()
            summary = self.selected_summary(successor, fresh)
            with self.lock:
                self.validations[successor] = fresh
                self.selected_retries[identifier] = successor
            return summary
        finally:
            self.operation_lock.release()

    def mcpb_command(self, tool, action, source, request, destination):
        argv = [tool, action, "--package", str(source / "package.json"),
                "--selection", str(source / "activation.json"), "--selection-sha256", request["selection_sha256"],
                "--component", request["component"], "--config", str(source / "configuration.json"),
                "--config-sha256", request["configuration_sha256"], "--destination", str(destination)]
        if action != "verify":
            argv += ["--artifact", str(source / "artifact.mcpb")]
        if action != "check":
            argv += ["--node", self.selected_node]
        return argv

    def selected_command(self, intent, *, verify):
        tools = self.selected_tools()
        path, effect, plan = selected_proposal(intent["root"], intent["validation_id"], intent["effect_sha256"], self.selected_binding)
        profile = effect["request"]["profile"]
        if profile not in self.selected["validators"]:
            raise SetupError("selected_profile_unavailable", 409)
        if profile == "mcpb":
            return self.mcpb_command(tools["mcpb"], "verify" if verify else "install", path,
                                     effect["request"], effect["destination"]), effect, plan
        argv = [self.python, "-I", "-B", tools["installer"], "--selected-" + profile, str(path),
                "--selection-sha256", effect["request"]["selection_sha256"], "--component", effect["request"]["component"],
                "--validator", tools[profile], "--destination", effect["destination"],
                "--" + profile, self.python if profile == "python" else self.selected_node]
        if "document_kind" in effect["request"]:
            argv += ["--document-kind", effect["request"]["document_kind"]]
        if profile == "python" and "python_target" in self.selected:
            argv += ["--python-target", self.selected["python_target"]]
        if "build_review" in effect:
            argv += ["--node-build-profile", tools["node_build"],
                     "--node-build-profile-sha256", self.selected_binding["tools"]["node_build"]]
        if verify:
            argv.append("--verify-existing")
        elif profile == "node":
            argv += ["--npm-cli", self.selected_npm_cli]
        return argv, effect, plan

    def selected_install(self, request):
        keys(request, ("validation_id", "effect_sha256"))
        with self.lock:
            self.admit_setup()
            identifier = request["validation_id"]
            if not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier):
                raise SetupError("validation_required", 409)
            value = self.validations.get(identifier)
            if value is None or value.get("kind") != "selected":
                raise SetupError("validation_required", 409)
            if request["effect_sha256"] != value["effect_sha256"]:
                raise SetupError("validation_changed", 409)
            if value["job_id"]:
                return {"job": copy.deepcopy(self.jobs[value["job_id"]])}
            _, effect, _ = selected_proposal(self.workspace, identifier, value["effect_sha256"], self.selected_binding)
            self.selected_tools()
            family = "selected-" + effect["request"]["profile"]
            intent = {"kind": "selected", "family": family, "root": self.workspace, "validation_id": identifier,
                      "effect_sha256": value["effect_sha256"], "destination": Path(effect["destination"])}
            if "build_review" in effect:
                # This live attempt requires its own successful installer reply.
                # A lost reply must not become cold consistency on reconciliation.
                intent["installer_reply_required"] = True
            def work():
                argv, _, _ = self.selected_command(intent, verify=False)
                review = effect.get("build_review")
                code, raw = self._run_selected_installer(argv, effect["request"]["profile"],
                    **({"capture": True, "installer_timeout": review["limits"]["timeout_seconds"]} if review else {}))
                if code:
                    raise SetupError("operation_unconfirmed", 500)
                if review:
                    # Observe the actual installer's bounded response before any
                    # mutable ready-file read. Overflow is a Runner error, never
                    # truncation accepted as success. Preserve this origin even
                    # when subsequent readback fails, in this same job intent.
                    if type(raw) is not bytes or len(raw) > MAX_JSON:
                        raise SetupError("operation_unconfirmed", 500)
                    receipt = decode(raw)
                    if (not isinstance(receipt, dict) or not isinstance(receipt.get("node_build"), dict)
                            or receipt["node_build"].get("review") != review
                            or receipt.get("destination") != effect["destination"]
                            or receipt.get("selection_sha256") != effect["request"]["selection_sha256"]):
                        raise SetupError("operation_unconfirmed", 500)
                    canonical = encoded(receipt)
                    if len(canonical) > MAX_JSON:
                        raise SetupError("operation_unconfirmed", 500)
                    intent["installer_reply"] = {"raw_sha256": digest(raw), "bytes": len(raw),
                                                  "semantic_sha256": digest(canonical)}
                verified = self.selected_readback(intent)
                return {**verified, "scope": "observed_software_installation_only", "readback_scope": READBACK_SCOPE}
            def retain(job):
                path, current, _ = selected_proposal(self.workspace, identifier, value["effect_sha256"], self.selected_binding)
                for name in (*current["files"], "plan.json", "effect.json",
                             *(("build-review.json",) if "build_review" in current else ()),
                             *(("plugin-source.json",) if "plugin_source" in current["request"] else ())):
                    file = path / name
                    sync_file(file.parent, file.name)
                sync_directory(self.workspace / "selections")
                record = {"schema_version": 1, "kind": SELECTED_REQUEST, "job_id": job["id"],
                          "validation_id": identifier, "workspace": str(self.workspace), "effect_sha256": value["effect_sha256"]}
                if "retry_of" in value:
                    record["retry_of"] = value["retry_of"]
                raw = encoded(record)
                name = job["id"] + ".json"
                with self._retaining_request(job, raw):
                    if selected_read(self.workspace / "requests", name) != raw:
                        raise SetupError("recovery_request_changed", 409)
                    value["job_id"] = job["id"]
                    intent["recovery_anchor"] = {"root": self.workspace, "id": job["id"], "sha256": digest(raw)}
                    self.install_intents[job["id"]] = intent
                    job["recovery_request"] = {"id": job["id"], "sha256": digest(raw)}
                    if "retry_of" in value:
                        job["retry_of"] = value["retry_of"]
            job = self.start_job("install", family, work, before_start=retain)
            return {"job": copy.deepcopy(job)}

    def selected_readback(self, intent):
        anchor = intent.get("recovery_anchor")
        if anchor is not None:
            canonical_directory(anchor["root"])
            canonical_directory(intent["destination"].parent)
            if digest(selected_read(anchor["root"], "requests/" + anchor["id"] + ".json", MAX_BODY * 2)) != anchor["sha256"]:
                raise SetupError("reconciliation_unconfirmed", 500)
        argv, effect, plan = self.selected_command(intent, verify=True)
        destination = Path(effect["destination"])
        receipt_raw = selected_read(intent["root"], "installations/" + intent["validation_id"] + "/ready/installed.json", SELECTED_FILE)
        profile = effect["request"]["profile"]
        origin = intent.get("installer_reply")
        if intent.get("installer_reply_required") and origin is None:
            raise SetupError("reconciliation_unconfirmed", 500)
        if origin is not None:
            # Both JSON producers use this one semantic projection. The ready
            # file's whitespace is not the installer's stdout wire format.
            if (digest(encoded(decode(receipt_raw))) != origin["semantic_sha256"]
                    or ("receipt_sha256" in origin and digest(receipt_raw) != origin["receipt_sha256"])):
                raise SetupError("reconciliation_unconfirmed", 500)
        code, raw = self._run_selected_installer(argv, profile, capture=True,
                                   **({"inspection_timeout": MCPB_INSPECTION_TIMEOUT} if profile == "mcpb" else {}))
        if code:
            raise SetupError("reconciliation_unconfirmed", 500)
        proof = decode(raw)
        receipt = decode(receipt_raw)
        mcp_raw = selected_read(intent["root"], "installations/" + intent["validation_id"] + "/ready/mcp.json", MAX_JSON)
        mcp = decode(mcp_raw)
        if profile == "mcpb":
            self.mcpb_readback(raw, receipt_raw, receipt, mcp, effect, plan)
        else:
            self.selected_package_readback(proof, receipt_raw, receipt, mcp, effect, plan)
        if (selected_read(intent["root"], "installations/" + intent["validation_id"] + "/ready/installed.json", SELECTED_FILE) != receipt_raw
                or selected_read(intent["root"], "installations/" + intent["validation_id"] + "/ready/mcp.json", MAX_JSON) != mcp_raw):
            raise SetupError("reconciliation_unconfirmed", 500)
        self.selected_command(intent, verify=True)
        if anchor is not None and digest(selected_read(anchor["root"], "requests/" + anchor["id"] + ".json", MAX_BODY * 2)) != anchor["sha256"]:
            raise SetupError("reconciliation_unconfirmed", 500)
        result = {"scope": READBACK_SCOPE, "selection_kind": profile, "component": effect["request"]["component"],
                  "selection_sha256": effect["request"]["selection_sha256"], **selected_identity(plan, effect["request"]),
                  "effect_sha256": intent["effect_sha256"], "mcp_path": str(destination / "ready/mcp.json"),
                  "receipt_path": str(destination / "ready/installed.json"), "receipt_sha256": digest(receipt_raw),
                  "mcp_configuration": mcp}
        if "build_review" in effect:
            result["build_receipt_binding"] = "original_installer_reply" if origin is not None else "current_receipt_consistency_only"
            if origin is not None:
                result["installer_reply"] = {**origin, "receipt_sha256": digest(receipt_raw)}
        if "declared_service" in plan:
            result["declared_service"] = copy.deepcopy(plan["declared_service"])
        if profile == "mcpb":
            result["configuration_sha256"] = effect["request"]["configuration_sha256"]
        if "plugin_source" in effect["request"]:
            result["plugin_source"] = copy.deepcopy(effect["request"]["plugin_source"])
        if "acquired_source" in effect["request"]:
            result["acquired_source"] = copy.deepcopy(effect["request"]["acquired_source"])
        if len(encoded(result)) > MAX_JSON:
            raise SetupError("selected_output_too_large")
        if origin is not None:
            # Only successful verification establishes the retained raw-file pin.
            # Future live observations also retain the original semantic origin.
            origin["receipt_sha256"] = digest(receipt_raw)
        return result

    def mcpb_readback(self, raw, receipt_raw, receipt, mcp, effect, plan):
        """Bind the canonical MCPB verifier's receipt, without reimplementing it."""
        try:
            keys(receipt, ("schema_version", "scope", "selection_sha256", "package_sha256", "configuration_sha256",
                           "destination", "plan", "runtime", "mcp_configuration", "launch_environment"))
            keys(receipt["runtime"], ("executable", "sha256", "version"))
            request = effect["request"]
            expected_mcp = {"mcpServers": {plan["mcp_name"]: {"command": self.selected_node,
                "args": [str(Path(effect["destination"]) / "prepared/tree" / plan["entrypoint"]["executable"]),
                         *plan["entrypoint"]["arguments"]]}}}
            runtime = receipt["runtime"]
            if (raw != receipt_raw + b"\n" or type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1
                    or receipt["scope"] != "observed_mcpb_software_installation_only"
                    or any(receipt[k] != request[k] for k in ("package_sha256", "selection_sha256", "configuration_sha256"))
                    or receipt["destination"] != effect["destination"] or encoded(receipt["plan"]) != encoded(plan)
                    or runtime["executable"] != self.selected_node or not selected_hash(runtime["sha256"])
                    or not isinstance(runtime["version"], str) or not 1 <= len(runtime["version"]) <= 128
                    or any(ord(c) < 32 for c in runtime["version"])
                    or not isinstance(receipt["launch_environment"], str)
                    or encoded(receipt["mcp_configuration"]) != encoded(expected_mcp) or encoded(mcp) != encoded(expected_mcp)):
                raise ValueError()
        except (SetupError, KeyError, TypeError, ValueError, RecursionError):
            raise SetupError("reconciliation_unconfirmed", 500) from None

    def selected_package_readback(self, proof, receipt_raw, receipt, mcp, effect, plan):
        profile = effect["request"]["profile"]
        destination = Path(effect["destination"])
        expected = {"schema_version": 1, "scope": READBACK_SCOPE, "verified": True,
                    "selection_kind": profile, "destination": str(destination),
                    "selection_sha256": effect["request"]["selection_sha256"], "component": effect["request"]["component"],
                    **selected_identity(plan, effect["request"]), "selected_runtime_path": self.python if profile == "python" else self.selected_node,
                    "historical_receipt": {"sha256": digest(receipt_raw), "recorded_scope": "observed_software_installation_only"}}
        if profile == "python" and self.selected.get("python_target") == "linux-x86_64":
            expected["python_target"] = "linux-x86_64"
            if not isinstance(receipt, dict) or receipt.get("python_target") != "linux-x86_64":
                raise SetupError("reconciliation_unconfirmed", 500)
        elif profile == "python" and ((isinstance(proof, dict) and "python_target" in proof)
                                      or (isinstance(receipt, dict) and "python_target" in receipt)):
            raise SetupError("reconciliation_unconfirmed", 500)
        try:
            identity = selected_identity(plan, effect["request"])
            if selected_identity(proof, effect["request"]) != identity or selected_identity(receipt, effect["request"]) != identity:
                raise ValueError()
        except (SetupError, ValueError):
            raise SetupError("reconciliation_unconfirmed", 500) from None
        if (not isinstance(proof, dict) or type(proof.get("schema_version")) is not int or proof.get("verified") is not True
                or any(proof.get(k) != v for k, v in expected.items()) or not isinstance(proof.get("retained_payloads"), dict)):
            raise SetupError("reconciliation_unconfirmed", 500)
        command = proof.get("command")
        if (not isinstance(command, list) or not command
                or any(not isinstance(item, str) for item in command)):
            raise SetupError("reconciliation_unconfirmed", 500)
        expected_mcp = {"mcpServers": {plan["mcp_name"]: {"command": command[0], "args": command[1:]}}}
        if (mcp != expected_mcp or not isinstance(receipt, dict) or receipt.get("scope") != "observed_software_installation_only"
                or receipt.get("command") != proof.get("command") or not isinstance(proof.get("command"), list)
                or receipt.get("mcp_configuration") != mcp or receipt.get("inputs") != effect["files"]
                or receipt.get("selection_sha256") != expected["selection_sha256"]
                or receipt.get("destination") != str(destination)):
            raise SetupError("reconciliation_unconfirmed", 500)

    def family(self, family):
        if not isinstance(family, str) or family not in self.families:
            raise SetupError("unknown_family")
        return self.families[family]

    def input_binding(self, family):
        return (self.official_contents_sha256, self.official_installer_sha256) if family in OFFICIAL_FAMILIES else (self.contents_sha256, self.installer_sha256)

    def official_command(self, extra, verifier=False, family=OFFICIAL_FAMILY):
        if (self.official_bundle is None or digest(read_file(self.official_bundle, "CONTENTS.json")) != self.official_contents_sha256
                or digest(read_file(self.workspace, "official-installer.py")) != self.official_installer_sha256):
            raise SetupError("frozen_official_changed", 500)
        tool = self.verifier if verifier else self.official_installer
        return [self.python, "-I", "-B", str(tool), "--official-bundle", str(self.official_bundle),
                "--contents-sha256", self.official_contents_sha256, "--component", "playwright" if family == PLAYWRIGHT_FAMILY else "github",
                *self.playwright_arguments(family), *extra]

    def playwright_arguments(self, family):
        if family != PLAYWRIGHT_FAMILY:
            return []
        if digest(read_file(self.workspace, "playwright-host.json", MAX_BODY)) != self.playwright_host_profile_sha256:
            raise SetupError("playwright_host_profile_changed")
        return ["--playwright-host-profile", str(self.playwright_host_profile),
                "--playwright-host-profile-sha256", self.playwright_host_profile_sha256,
                "--node", self.node, "--npm-cli", self.npm_cli]

    def official_description(self, family):
        code, raw = self._run(self.official_command(["--official-describe"], family=family), capture=True)
        if code:
            raise SetupError("official_metadata_unavailable", 500)
        value = decode(raw)
        if value.get("family") != family or value.get("contents_sha256") != self.official_contents_sha256 or value.get("scope") != "official_metadata_only":
            raise SetupError("official_metadata_unavailable", 500)
        return value

    def official_mcp(self, family, destination, server):
        if family == PLAYWRIGHT_FAMILY:
            self.playwright_arguments(family)
            profile = decode(read_file(self.workspace, "playwright-host.json", MAX_BODY))
            root = Path(profile["process_parent"]) / ("p-" + digest(str(destination).encode())[:16])
            template = decode(read_file(self.official_bundle, "playwright/mcp.template.json"))
            entry = copy.deepcopy(template["mcpServers"]["playwright-isolated"])
            replacements = {"ABS_PLAYWRIGHT_INSTALL/node_modules/@playwright/mcp/cli.js": str(destination / "node_modules/@playwright/mcp/cli.js"),
                "ABS_CHROME_EXECUTABLE": profile["chrome_executable"], "ABS_PROCESS_OUTPUT": str(root / "output")}
            entry.update(command=self.node, args=[replacements.get(a, a) for a in entry["args"]], cwd=str(root / "workspace"),
                env={"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"), "PWTEST_SOCKETS_DIR": str(root / "sockets")})
            return {"mcpServers": {server: entry}}
        template = decode(read_file(self.official_bundle, "github/mcp.template.json"))
        entry = copy.deepcopy(template["mcpServers"]["github-selected-reads"])
        entry.update(command=str(destination / "github-mcp-server"), cwd=str(destination))
        return {"mcpServers": {server: entry}}

    def expected_command(self, family: str, destination: Path):
        if family in OFFICIAL_FAMILIES:
            entry = self.official_mcp(family, destination, "selected")["mcpServers"]["selected"]
            return [entry["command"], *entry["args"]]
        if family == "activepieces":
            return [self.node, str(destination / "node_modules/@rkat/toolkit-activepieces/dist/mcp-cli.js")]
        module = "meerkat_toolkit_agno.agno_mcp" if family == "agno" else "meerkat_toolkit_services.adapters.services_mcp"
        return [str(destination / "runtime/bin/python"), "-I", "-m", module]

    def installer(self, family: str, destination: Path, configuration=None):
        if family in OFFICIAL_FAMILIES:
            if configuration is None:
                raise SetupError("configuration_required")
            return self._run(self.official_command(["--destination", str(destination),
                "--config", str(configuration["path"]), "--config-sha256", configuration["sha256"],
                "--server-name", configuration["server_name"]], family=family))[0]
        if (digest(read_file(self.bundle, "CONTENTS.json")) != self.contents_sha256
                or digest(read_file(self.bundle, "install-library-consumer.py")) != self.installer_sha256):
            raise SetupError("frozen_bundle_changed", 500)
        argv = [self.python, "-I", "-B", str(self.bundle / "install-library-consumer.py"),
                "--bundle", str(self.bundle), "--contents-sha256", self.contents_sha256,
                "--family", family, "--destination", str(destination)]
        if family == "activepieces":
            argv += ["--node", self.node, "--npm-cli", self.npm_cli]
        else:
            argv += ["--python", self.python]
        if configuration:
            argv += ["--config", str(configuration["path"]), "--config-sha256", configuration["sha256"],
                     "--server-name", configuration["server_name"]]
        return self._run(argv)[0]

    def receipt(self, family: str, destination: Path, configuration=None):
        name = "ready/installed.json" if configuration else "installed.json"
        receipt = decode(read_file(destination, name, 16 * 1024 * 1024))
        command = self.expected_command(family, destination)
        if (receipt.get("family") != family or receipt.get("destination") != str(destination)
                or receipt.get("contents_sha256") != self.input_binding(family)[0]
                or receipt.get("scope") != "observed_software_installation_only"):
            raise SetupError("receipt_unconfirmed", 500)
        if configuration:
            path = destination / "configuration/selected.json"
            if family not in OFFICIAL_FAMILIES:
                command += ["serve", "--config", str(path)]
            expected = {"sha256": configuration["sha256"], "path": str(path),
                        "server_name": configuration["server_name"], "validation": "configuration_only"}
            if receipt.get("configuration") != expected or digest(read_file(destination, "configuration/selected.json", MAX_BODY)) != expected["sha256"]:
                raise SetupError("receipt_unconfirmed", 500)
            mcp = decode(read_file(destination, "ready/mcp.json"))
            expected_mcp = self.official_mcp(family, destination, expected["server_name"]) if family in OFFICIAL_FAMILIES else {"mcpServers": {expected["server_name"]: {"command": command[0], "args": command[1:]}}}
            if mcp != expected_mcp or receipt.get("mcp_configuration") != mcp:
                raise SetupError("receipt_unconfirmed", 500)
        if receipt.get("command") != command:
            raise SetupError("receipt_unconfirmed", 500)
        if (configuration and "handoff" in configuration
                and target_projection(family, receipt) != target_projection(family, configuration["handoff"])):
            raise SetupError("receipt_unconfirmed", 500)
        return receipt

    def inspect(self, family, args):
        code, data = self._run(self.commands[family] + args, capture=True)
        if code:
            raise SetupError("inspection_unavailable", 500)
        return decode(data)

    @contextmanager
    def _retaining_request(self, job, raw):
        # Called under the existing state/operation locks. Keep the failure
        # boundary through each caller's readback and correlation writes.
        # A partial row is retained for diagnosis, never an invented job.
        try:
            name = job["id"] + ".json"
            write_new(self.workspace / "requests" / name, raw)
            sync_file(self.workspace / "requests", name)
            sync_directory(self.workspace)
            sync_directory(self.workspace.parent)
            yield
        except BaseException:
            # Close new setup effects only. Existing canonical readback stays
            # available, and the original operation error remains unchanged.
            self.request_retention_failed = True
            raise

    def start_job(self, kind, family, worker, before_start=None, on_success=None, result_status=None):
        if (self.recovery_selection_writer is not None and before_start is not None
                and sum("recovery_request" in item for item in self.jobs.values()) >= 256):
            raise SetupError("recovery_selection_full", 409)
        # Allocate handoff objects before acquiring the operation lock.
        dispatch_gate = threading.Event()
        control = {"requested": threading.Event(), "spawn_lock": threading.Lock()}
        job = {"id": secrets.token_hex(16), "kind": kind, "family": family, "status": "running"}
        # Called with state lock held. Lock is acquired before any thread starts.
        if not self.operation_lock.acquire(blocking=False):
            raise SetupError("operation_in_progress", 409)
        try:
            if before_start is not None:
                before_start(job)
            self.jobs[job["id"]] = job
            self._job_controls[job["id"]] = control
            self.job = job
        except BaseException:
            # The callback may have completed retention before registration
            # failed. That row must not be omitted by a later checkpoint.
            if "recovery_request" in job:
                self.request_retention_failed = True
                if self.jobs.get(job["id"]) is job:
                    job.update(status="unconfirmed", error="recovery_request_unavailable")
            self.operation_lock.release()
            raise
        if self.recovery_selection_writer is not None and "recovery_request" in job:
            try:
                # Publish the existing owner's exact readback selection before
                # any worker can run. It records requests, never their outcome
                # or permission to execute them again.
                selection = self.recovery_manifest({"job_ids": [identifier for identifier, item
                    in self.jobs.items() if "recovery_request" in item and not item.get("recovered")]})
                self.recovery_selection_writer(selection)
            except BaseException as error:
                # Preserve the new anchor and its visible failed admission.
                # No later effect may hide a failed durable checkpoint.
                job.update(status="unconfirmed", error="recovery_selection_unavailable")
                self.closing = True
                self._job_controls.pop(job["id"], None)
                self.operation_lock.release()
                if not isinstance(error, Exception):
                    raise
                return job
        dispatch_allowed = False
        def run():
            dispatch_gate.wait()
            if not dispatch_allowed:
                return
            self._job_context.control = control
            try:
                self._check_stop()
                result = worker()
                with self.lock:
                    self._check_stop()
                    if on_success is not None:
                        on_success()
                    job.update(status=result_status(result) if result_status else "succeeded", result=result)
            except BaseException as error:
                with self.lock:
                    stopped = control["requested"].is_set()
                    code = error.code if kind in ("compose", "acquire") and isinstance(error, SetupError) else "operation_unconfirmed"
                    job.update(status="unconfirmed", error="operation_stopped" if stopped else code)
                    if (kind == "compose" and code == "compose_operation_unconfirmed" and
                            getattr(error, "runtime_error", None) in self.compose.runtime_errors):
                        job["runtime_error"] = error.runtime_error
                    if stopped:
                        job["stop"]["status"] = "settled"
                    if kind == "prepare":
                        self.families[family]["status"] = "unconfirmed"
                if not isinstance(error, Exception):
                    raise
            finally:
                del self._job_context.control
                with self.lock:
                    self._job_controls.pop(job["id"], None)
                self.operation_lock.release()
        thread = None
        try:
            thread = threading.Thread(target=run, name="consumer-setup-operation", daemon=False)
            thread.start()
            self.threads.append(thread)
            self._job_threads[job["id"]] = thread
            dispatch_allowed = True
        except BaseException as error:
            dispatch_allowed = False
            job.update(status="unconfirmed", error="operation_unconfirmed")
            self._job_controls.pop(job["id"], None)
            self.operation_lock.release()
            if thread is not None and thread.ident is not None:
                if thread not in self.threads:
                    self.threads.append(thread)
                self._job_threads[job["id"]] = thread
            if not isinstance(error, Exception):
                raise
        finally:
            # A start failure after actual thread creation still releases the
            # waiting thread without running the installer or releasing twice.
            dispatch_gate.set()
        return job

    def prepare(self, request):
        keys(request, ("family",))
        family = request["family"]
        with self.lock:
            self.admit_setup()
            state = self.family(family)
            if state["status"] != "unprepared":
                raise SetupError("family_already_prepared_or_pending", 409)
            return self._prepare_attempt(family)

    def retry_prepare(self, request):
        keys(request, ("job_id",))
        with self.lock:
            self.admit_setup()
            identifier = request["job_id"]
            if not isinstance(identifier, str) or identifier not in self.jobs:
                raise SetupError("unknown_job", 404)
            # An accepted request keeps its identity even after a later attempt.
            if identifier in self.preparation_retries:
                return {"job": copy.deepcopy(self.jobs[self.preparation_retries[identifier]])}
            source = self.jobs[identifier]
            if source["kind"] != "prepare" or source["status"] != "unconfirmed":
                raise SetupError("preparation_retry_unavailable", 409)
            state = self.family(source["family"])
            if state.get("preparation_job_id") != identifier:
                raise SetupError("preparation_superseded", 409)
            if state["status"] != "unconfirmed":
                raise SetupError("preparation_retry_unavailable", 409)
            return self._prepare_attempt(source["family"], retry_of=identifier)

    def _prepare_attempt(self, family, retry_of=None):
        # Caller holds the state lock. start_job acquires the operation lock
        # before reserving the attempt or dispatching its worker.
        state = self.family(family)
        destination = None
        prepared_command = None
        prepared_profiles = []

        def reserve(job):
            nonlocal destination
            name = family if retry_of is None else family + "-" + job["id"]
            destination = self.workspace / "inspection" / name
            state["preparation_job_id"] = job["id"]
            if retry_of is not None:
                job["retry_of"] = retry_of
                self.preparation_retries[retry_of] = job["id"]

        def work():
            nonlocal prepared_command
            if family in OFFICIAL_FAMILIES:
                source = self.official_description(family)
            else:
                if self.installer(family, destination):
                    raise SetupError("operation_unconfirmed", 500)
                self.receipt(family, destination)
                prepared_command = self.expected_command(family, destination)
                code, data = self._run(prepared_command + ["profiles"], capture=True)
                if code:
                    raise SetupError("inspection_unavailable", 500)
                source = decode(data)
            profiles = [{"id": row["profile"], "operations": operation_rows(family, row)} for row in source["profiles"]]
            if not 1 <= len(profiles) <= 256 or len({row["id"] for row in profiles}) != len(profiles):
                raise SetupError("inspection_unavailable", 500)
            if any(not isinstance(row["id"], str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", row["id"]) for row in profiles):
                raise SetupError("inspection_unavailable", 500)
            prepared_profiles.extend(profiles)
            return {"scope": "official_metadata_only" if family in OFFICIAL_FAMILIES else "inspection_environment_only"}

        def complete():
            if prepared_command is not None:
                self.commands[family] = prepared_command
            state.update(status="prepared", profiles=prepared_profiles)

        job = self.start_job("prepare", family, work, before_start=reserve, on_success=complete)
        state["status"] = "preparing" if job["status"] == "running" else "unconfirmed"
        return {"job": copy.deepcopy(job)}

    def help_locked(self, family, profile, full_guide=False):
        state = self.family(family)
        if state["status"] != "prepared":
            raise SetupError("family_not_prepared", 409)
        if not isinstance(profile, str) or profile not in {row["id"] for row in state["profiles"]}:
            raise SetupError("unknown_profile")
        if family in OFFICIAL_FAMILIES:
            source = self.official_description(family)["help"]
            return package_help(family, profile, source, source["guide_text"], full_guide)
        source = self.inspect(family, ["config-help", "--profile", profile])
        guide = read_file(self.bundle, family + "/PROFILE-GUIDE.md")
        if digest(guide) != self.guide_hashes[family]:
            raise SetupError("frozen_bundle_changed", 500)
        help_value = package_help(family, profile, source, guide.decode("utf-8"), full_guide)
        listing = next(row for row in state["profiles"] if row["id"] == profile)
        if help_value["operations"] != listing["operations"]:
            raise SetupError("inspection_unavailable", 500)
        return help_value

    def help(self, request):
        self.admit_setup()
        if not isinstance(request, dict):
            raise SetupError("invalid_request")
        keys({key: value for key, value in request.items() if key != "full_guide"}, ("family", "profile"))
        full_guide = request.get("full_guide", False)
        if type(full_guide) is not bool:
            raise SetupError("invalid_request")
        if not self.operation_lock.acquire(blocking=False):
            raise SetupError("operation_in_progress", 409)
        try:
            return self.help_locked(request["family"], request["profile"], full_guide)
        finally:
            self.operation_lock.release()

    def configuration(self, request):
        keys(request, ("family", "profile", "operations", "values"))
        family, profile = request["family"], request["profile"]
        help_value = self.help_locked(family, profile)
        operations, values = request["operations"], request["values"]
        allowed = {row["id"] for row in help_value["operations"]}
        if (not isinstance(operations, list) or not operations or len(operations) > len(allowed)
                or any(not isinstance(value, str) or value not in allowed for value in operations)
                or len(set(operations)) != len(operations) or not isinstance(values, dict)):
            raise SetupError("invalid_configuration")
        fields = {row["name"]: row for row in help_value["fields"]
                  if "only_when_operation_selected" not in row or row["only_when_operation_selected"] in operations}
        if not set(values) <= set(fields):
            raise SetupError("invalid_configuration")
        for name, value in values.items():
            field = fields[name]
            expected = {"string": str, "integer": int, "boolean": bool, "string_list": list}[field["kind"]]
            if type(value) is not expected:
                raise SetupError("invalid_configuration")
            if field["kind"] == "string_list" and not string_list_value(value):
                raise SetupError("invalid_configuration")
            if isinstance(value, str):
                if len(value) > 2048 or any(ord(char) < 32 or ord(char) == 127 for char in value):
                    raise SetupError("invalid_configuration")
                if field["credential_reference"] and not REFERENCE.fullmatch(value):
                    raise SetupError("environment_reference_name_required")
        # Requirements, ranges and provider-specific meaning stay with the
        # installed parser, which receives these exact assembled bytes.
        return configuration_bytes(request)

    def parser_check(self, family: str, path: Path, selection):
        # The existing bounded inspection owns execution. Only verified names
        # leave this boundary; arbitrary validator output is discarded.
        data = read_file(path.parent, path.name, MAX_BODY)
        if selection["family"] != family or configuration_bytes(selection) != data:
            raise SetupError("configuration_changed", 409)
        help_value = self.help_locked(family, selection["profile"])
        if family in OFFICIAL_FAMILIES:
            argv = self.official_command(["--check-config-only", "--config", str(path), "--config-sha256", digest(data), "--server-name", "toolkit-check"], family=family)
        else:
            argv = self.commands[family] + ["check-config", "--config", str(path)]
        code, raw = self._run(argv, capture=True)
        if read_file(path.parent, path.name, MAX_BODY) != data:
            raise SetupError("configuration_changed", 409)
        if code:
            error = SetupError("invalid_configuration")
            if type(code) is int and code == 1:
                error.configuration_diagnostic = configuration_diagnostic(
                    raw, family, selection["profile"], digest(data), self.input_binding(family)[0])
            raise error
        try:
            source = decode(raw)
            projection = credential_projection(source, selection, help_value)
        except SetupError:
            raise SetupError("invalid_credential_projection") from None
        target = target_projection(family, source)
        if target != target_projection(family, help_value):
            raise SetupError("invalid_target_projection")
        projection.update(target)
        if read_file(path.parent, path.name, MAX_BODY) != data:
            raise SetupError("configuration_changed", 409)
        return {"family": family, "profile": selection["profile"],
                "configuration_sha256": digest(data), "contents_sha256": self.input_binding(family)[0], **projection}

    def check(self, request):
        with self.lock:
            self.admit_setup()
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
        try:
            if len(self.validations) >= 128:
                raise SetupError("new_workspace_required", 409)
            data = self.configuration(request)
            identifier = secrets.token_hex(16)
            path = self.workspace / "configurations" / (identifier + ".json")
            write_new(path, data)
            handoff = self.parser_check(request["family"], path, request)
            if read_file(path.parent, path.name, MAX_BODY) != data:
                raise SetupError("configuration_changed", 409)
            value = {"request": copy.deepcopy(request), "path": path, "sha256": digest(data),
                     "handoff": handoff, "job_id": None}
            with self.lock:
                self.validations[identifier] = value
            return {"validation_id": identifier, "scope": "configuration_only", **copy.deepcopy(handoff)}
        finally:
            self.operation_lock.release()

    def install(self, request):
        keys(request, ("validation_id", "configuration_sha256", "family", "profile", "operations", "values"))
        with self.lock:
            self.admit_setup()
            identifier = request["validation_id"]
            if not isinstance(identifier, str) or identifier not in self.validations:
                raise SetupError("validation_required", 409)
            value = self.validations[identifier]
            if value.get("kind") == "selected":
                raise SetupError("validation_required", 409)
            selected = {key: request[key] for key in ("family", "profile", "operations", "values")}
            if request["configuration_sha256"] != value["sha256"] or encoded(selected) != encoded(value["request"]):
                raise SetupError("validation_changed", 409)
            if value["job_id"]:
                return {"job": copy.deepcopy(self.jobs[value["job_id"]])}
            family = selected["family"]
            configuration = dict(value, server_name="toolkit-" + family + "-" + identifier[:12])
            destination = self.workspace / "installations" / identifier
            def work():
                if digest(read_file(value["path"].parent, value["path"].name, MAX_BODY)) != value["sha256"]:
                    raise SetupError("configuration_changed", 409)
                handoff = self.parser_check(family, value["path"], selected)
                if handoff != value["handoff"]:
                    raise SetupError("credential_projection_changed", 409)
                # Installer validates the same hash again, then its freshly
                # installed parser checks it before publishing ready/ files.
                if self.installer(family, destination, configuration):
                    raise SetupError("operation_unconfirmed", 500)
                receipt = self.receipt(family, destination, configuration)
                return {"scope": receipt["scope"], "mcp_path": str(destination / "ready/mcp.json"),
                        "receipt_path": str(destination / "ready/installed.json"),
                        "mcp_configuration": receipt["mcp_configuration"], **copy.deepcopy(handoff)}
            def retain(job):
                # This artifact records an original request, never whether an
                # installer started or finished. It cannot authorize replay.
                record = {"schema_version": 3 if family == PLAYWRIGHT_FAMILY else 2 if family == OFFICIAL_FAMILY else 1, "kind": "toolkit.consumer_setup.configured_install_request",
                    "job_id": job["id"], "validation_id": identifier, "workspace": str(self.workspace),
                    "selection": copy.deepcopy(selected), "configuration_sha256": value["sha256"],
                    "server_name": configuration["server_name"], "contents_sha256": self.input_binding(family)[0],
                    "installer_sha256": self.input_binding(family)[1],
                    "runtimes": {"python": self.python, "node": self.node, "npm_cli": self.npm_cli}}
                if family in OFFICIAL_FAMILIES:
                    record["official_component"] = "playwright" if family == PLAYWRIGHT_FAMILY else "github"
                if family == PLAYWRIGHT_FAMILY:
                    record["playwright_host_profile_sha256"] = self.playwright_host_profile_sha256
                data = encoded(record)
                if len(data) > MAX_BODY * 2 or digest(read_file(value["path"].parent, value["path"].name, MAX_BODY)) != value["sha256"]:
                    raise SetupError("configuration_changed", 409)
                sync_file(value["path"].parent, value["path"].name)
                name = job["id"] + ".json"
                with self._retaining_request(job, data):
                    if read_file(self.workspace / "requests", name, MAX_BODY * 2) != data:
                        raise SetupError("recovery_request_changed", 409)
                    # Both in-memory correlations exist before thread creation.
                    value["job_id"] = job["id"]
                    self.install_intents[job["id"]] = {"family": family, "destination": destination,
                        "configuration": copy.deepcopy(configuration), "profile": selected["profile"],
                        "recovery_anchor": {"root": self.workspace, "id": job["id"], "sha256": digest(data)}}
                    job["recovery_request"] = {"id": job["id"], "sha256": digest(data)}
            job = self.start_job("install", family, work, before_start=retain)
            return {"job": copy.deepcopy(job)}

    def reconcile(self, request, *, retained_setup_metadata=False):
        keys(request, ("job_id",))
        with self.lock:
            self.admit()
            if isinstance(request["job_id"], str) and request["job_id"] in self.export_intents:
                exporting = True
            else:
                exporting = False
        if exporting:
            return self.reconcile_export(request["job_id"])
        if isinstance(request["job_id"], str) and request["job_id"] in self.acquisition_intents:
            return self.reconcile_acquisition(request["job_id"])
        with self.lock:
            self.admit()
            identifier = request["job_id"]
            if not isinstance(identifier, str) or identifier not in self.install_intents or self.jobs[identifier]["kind"] != "install":
                raise SetupError("unknown_installation", 404)
            if self.jobs[identifier]["status"] == "running":
                raise SetupError("operation_in_progress", 409)
            if self.verifier is None and self.install_intents[identifier].get("kind") != "selected":
                raise SetupError("reconciliation_unavailable", 409)
            if not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
            intent = copy.deepcopy(self.install_intents[identifier])
            self.observation_revision += 1
            observation = {"job_id": identifier, "revision": self.observation_revision,
                           "status": "checking", "scope": READBACK_SCOPE, "excluded": (["No connector/browser process, credential lookup, current callability, account access or native attachment is established. Mutable serving directories and browser bundle integrity are excluded.", "Exclusive quiescent host custody is required; this is not an atomic filesystem snapshot."] if intent["family"] in OFFICIAL_FAMILIES else list(MCPB_READBACK_EXCLUSIONS if intent["family"] == "selected-mcpb" else READBACK_EXCLUSIONS))}
            if retained_setup_metadata and intent["family"] in ("agno", "services", "activepieces"):
                observation["excluded"][3] = (
                    "The canonical artifact verifier runs no installed configuration validator. "
                    "Separate config-help/check-config metadata observation may execute the verified installed adapter; "
                    "connector serving, authentication and native attachment are not run or established.")
            self.observations[identifier] = observation
        try:
            result = self.readback(intent)
            if retained_setup_metadata:
                # Explicit opt-in only. Ordinary readback still starts no
                # acquired adapter code. Keep the existing lock/cancellation
                # owner and verify the installed artifacts again afterwards.
                metadata = self.retained_setup_metadata(intent)
                if self.readback(intent) != result:
                    raise SetupError("reconciliation_unconfirmed", 500)
                observation["setup_metadata"] = metadata
            with self.lock:
                if intent.get("installer_reply_required"):
                    # readback used a copy. Commit its successful raw-file pin
                    # only to the same retained live origin, not to cold intent
                    # or the historical job outcome.
                    owned = self.install_intents[identifier]
                    origin, checked = owned.get("installer_reply"), intent.get("installer_reply")
                    if (not owned.get("installer_reply_required") or not isinstance(origin, dict)
                            or not isinstance(checked, dict)
                            or any(checked.get(key) != value for key, value in origin.items())
                            or checked.get("receipt_sha256") != result.get("receipt_sha256")):
                        raise SetupError("reconciliation_unconfirmed", 500)
                    origin["receipt_sha256"] = checked["receipt_sha256"]
                observation.update(status="matched", result=result)
        except BaseException as error:
            # Never project verifier output or exception values. A failed check
            # does not invalidate the historical job or establish missing files.
            with self.lock:
                observation.update(status="unconfirmed", error="reconciliation_unconfirmed")
            if not isinstance(error, Exception):
                raise
        finally:
            self.operation_lock.release()
        with self.lock:
            return {"observation": copy.deepcopy(observation)}

    def reconcile_acquisition(self, identifier):
        with self.lock:
            self.admit()
            if self.jobs[identifier]["status"] == "running" or not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
            intent = copy.deepcopy(self.acquisition_intents[identifier])
            self.observation_revision += 1
            observation = {"job_id": identifier, "revision": self.observation_revision, "status": "checking",
                           "scope": ACQUISITION_READBACK, "excluded": list(ACQUISITION_EXCLUSIONS)}
            self.observations[identifier] = observation
        try:
            anchor = intent["recovery_anchor"]
            record, current = load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"])
            if current != intent:
                raise ValueError()
            directory = anchor["root"] / "acquisitions" / identifier
            raw = selected_read(directory, "request.json", MAX_BODY)
            if raw != encoded(record["arguments"]):
                raise ValueError()
            kind = record["family"].removeprefix("selected-")
            result = self.acquisition_modules[kind].readback(acquisition_io(), record, directory)
            if len(encoded(result)) > MAX_JSON:
                raise ValueError()
            # The owner checks acquired members; retain the exact request anchor
            # across that check too. Host custody is required, not inferred.
            after, after_intent = load_acquisition_recovery(anchor["root"], anchor["id"], anchor["sha256"])
            if after != record or after_intent != intent or selected_read(directory, "request.json", MAX_BODY) != raw:
                raise ValueError()
            with self.lock:
                observation.update(status="matched", result=result)
        except BaseException as error:
            with self.lock:
                observation.update(status="unconfirmed", error="reconciliation_unconfirmed")
            if not isinstance(error, Exception):
                raise
        finally:
            self.operation_lock.release()
        with self.lock:
            return {"observation": copy.deepcopy(observation)}

    def reconcile_export(self, identifier):
        with self.lock:
            self.admit()
            if self.jobs[identifier]["status"] == "running" or not self.operation_lock.acquire(blocking=False):
                raise SetupError("operation_in_progress", 409)
            intent = copy.deepcopy(self.export_intents[identifier])
            self.observation_revision += 1
            observation = {"job_id": identifier, "revision": self.observation_revision, "status": "checking",
                           "scope": REMOTE_EXPORT_READBACK, "excluded": list(REMOTE_EXPORT_EXCLUSIONS)}
            self.observations[identifier] = observation
        try:
            result = self.export_readback(intent)
            with self.lock:
                observation.update(status="matched" if result["export_check"]["state"] == "matches" else "unconfirmed",
                                   result=result, observed_at=time.time())
        except BaseException as error:
            with self.lock:
                observation.update(status="unconfirmed", error="reconciliation_unconfirmed", observed_at=time.time())
            if not isinstance(error, Exception):
                raise
        finally:
            self.operation_lock.release()
        with self.lock:
            return {"observation": copy.deepcopy(observation)}

    def retained_setup_metadata(self, intent):
        """Current metadata only, under reconcile's existing operation custody.

        Library config-help/check-config executes the verified installed adapter,
        never prepare/install/serve. Selected declarations require no adapter run.
        This is separate from the payload verifier's no-parser-execution claim.
        """
        if intent.get("kind") == "selected":
            _, effect, plan = self.selected_command(intent, verify=True)
            profile = effect["request"]["profile"]
            if profile not in ("python", "node") or "declared_service" not in plan:
                raise SetupError("retained_metadata_unavailable")
            identity = selected_identity(plan, effect["request"])
            document = "declaration.json" if "document_kind" in identity else "activation.json"
            declaration = {"profile": profile, "declared_service": copy.deepcopy(plan["declared_service"]),
                "files": {name: effect["files"][name] for name in ("package.json", document)},
                **identity, "selection_sha256": effect["request"]["selection_sha256"],
                "effect_sha256": intent["effect_sha256"]}
            for name in ("plugin_source", "acquired_source"):
                if name in effect["request"]:
                    declaration[name] = copy.deepcopy(effect["request"][name])
            return {"kind": "selected", "declaration": declaration}
        family = intent["family"]
        if family not in ("agno", "services", "activepieces"):
            raise SetupError("retained_metadata_unavailable")
        anchor = intent["recovery_anchor"]
        raw = read_file(anchor["root"], "requests/" + anchor["id"] + ".json", MAX_BODY * 2)
        if digest(raw) != anchor["sha256"]:
            raise SetupError("recovery_request_changed")
        selection = decode(raw)["selection"]
        configuration = intent["configuration"]
        data = read_file(configuration["path"].parent, configuration["path"].name, MAX_BODY)
        if digest(data) != configuration["sha256"] or configuration_bytes(selection) != data:
            raise SetupError("configuration_changed")
        command = self.expected_command(family, intent["destination"])
        code, raw_help = self._run(command + ["config-help", "--profile", selection["profile"]], capture=True)
        if code:
            raise SetupError("retained_metadata_unavailable")
        guide = read_file(self.bundle, family + "/PROFILE-GUIDE.md")
        if digest(guide) != self.guide_hashes[family]:
            raise SetupError("frozen_bundle_changed", 500)
        help_value = package_help(family, selection["profile"], decode(raw_help), guide.decode("utf-8"))
        allowed = {row["id"] for row in help_value["operations"]}
        if not set(selection["operations"]) <= allowed:
            raise SetupError("retained_metadata_unavailable")
        code, raw_check = self._run(command + ["check-config", "--config", str(configuration["path"])], capture=True)
        if code:
            raise SetupError("retained_metadata_unavailable")
        checked = decode(raw_check)
        references = credential_projection(checked, selection, help_value)
        if target_projection(family, checked) != target_projection(family, help_value):
            raise SetupError("invalid_target_projection")
        values = {field["name"]: selection["values"][field["name"]]
                  for field in help_value["fields"] if field["credential_reference"] and field["name"] in selection["values"]}
        if any(not isinstance(value, str) or not REFERENCE.fullmatch(value) for value in values.values()):
            raise SetupError("invalid_credential_projection")
        # No ordinary configuration values, guide text or validator diagnostics
        # leave this metadata projection. Missing references remain unknown.
        projected_help = {"family": family, "profile": selection["profile"],
            "operations": [{key: row[key] for key in ("id", "tool") if key in row} for row in help_value["operations"]],
            "fields": [{key: field[key] for key in ("name", "kind", "credential_reference")} for field in help_value["fields"]]}
        if "authentication" in help_value:
            projected_help["authentication"] = help_value["authentication"]
        return {"kind": "library", "selection": {"family": family, "profile": selection["profile"],
            "operations": copy.deepcopy(selection["operations"]), "values": values},
            "help": projected_help, "configuration_check": references}

    def readback(self, intent):
        if intent.get("kind") == "selected":
            return self.selected_readback(intent)
        anchor = intent.get("recovery_anchor")
        if anchor is not None:
            canonical_directory(anchor["root"])
            canonical_directory(intent["destination"].parent)
            if digest(read_file(anchor["root"], "requests/" + anchor["id"] + ".json", MAX_BODY * 2)) != anchor["sha256"]:
                raise SetupError("reconciliation_unconfirmed", 500)
        family, destination, configuration = intent["family"], intent["destination"], intent["configuration"]
        if (digest(read_file(self.workspace, "verifier.py")) != self.verifier_sha256
                or digest(read_file(configuration["path"].parent, configuration["path"].name, MAX_BODY)) != configuration["sha256"]):
            raise SetupError("reconciliation_unconfirmed", 500)
        receipt_before = read_file(destination, "ready/installed.json", 1024 * 1024)
        common = ["--verify-existing", "--destination", str(destination), "--config", str(configuration["path"]),
                  "--config-sha256", configuration["sha256"], "--server-name", configuration["server_name"]]
        if family in OFFICIAL_FAMILIES:
            argv = self.official_command(common, verifier=True, family=family)
        else:
            if (digest(read_file(self.bundle, "CONTENTS.json")) != self.contents_sha256
                    or digest(read_file(self.bundle, "install-library-consumer.py")) != self.installer_sha256):
                raise SetupError("reconciliation_unconfirmed", 500)
            argv = [self.python, "-I", "-B", str(self.verifier), "--bundle", str(self.bundle), "--contents-sha256", self.contents_sha256,
                    "--family", family, *common]
            if family == "activepieces":
                argv += ["--node", self.node, "--npm-cli", self.npm_cli]
            else:
                argv += ["--python", self.python]
        code, raw = self._run(argv, capture=True)
        if code:
            raise SetupError("reconciliation_unconfirmed", 500)
        proof = decode(raw)
        command = self.expected_command(family, destination)
        if family not in OFFICIAL_FAMILIES:
            command += ["serve", "--config", str(destination / "configuration/selected.json")]
        expected = {"schema_version": 1, "scope": READBACK_SCOPE, "verified": True,
                    "family": family, "destination": str(destination), "contents_sha256": self.input_binding(family)[0],
                    "configuration_sha256": configuration["sha256"], "command": command,
                    "historical_receipt": {"sha256": digest(receipt_before), "recorded_scope": "observed_software_installation_only"}}
        if family not in OFFICIAL_FAMILIES:
            expected["selected_runtime_path"] = self.node if family == "activepieces" else self.python
        else:
            expected["component"] = "playwright" if family == PLAYWRIGHT_FAMILY else "github"
            if family == PLAYWRIGHT_FAMILY:
                expected["host_profile_sha256"] = self.playwright_host_profile_sha256
        if (not isinstance(proof, dict) or type(proof.get("schema_version")) is not int
                or proof.get("verified") is not True or any(proof.get(key) != value for key, value in expected.items())
                or not isinstance(proof.get("retained_payloads"), dict)):
            raise SetupError("reconciliation_unconfirmed", 500)
        # Catch ordinary publication/input changes across the check. Exclusive
        # host custody is required; these reads are not an atomic filesystem snapshot.
        receipt = self.receipt(family, destination, configuration)
        target = target_projection(family, proof)
        if target != target_projection(family, receipt):
            raise SetupError("reconciliation_unconfirmed", 500)
        if (read_file(destination, "ready/installed.json", 1024 * 1024) != receipt_before
                or digest(read_file(configuration["path"].parent, configuration["path"].name, MAX_BODY)) != configuration["sha256"]
                or digest(read_file(self.workspace, "verifier.py")) != self.verifier_sha256):
            raise SetupError("reconciliation_unconfirmed", 500)
        if anchor is not None and digest(read_file(anchor["root"], "requests/" + anchor["id"] + ".json", MAX_BODY * 2)) != anchor["sha256"]:
            raise SetupError("reconciliation_unconfirmed", 500)
        return {"mcp_path": str(destination / "ready/mcp.json"), "receipt_path": str(destination / "ready/installed.json"),
                "mcp_configuration": receipt["mcp_configuration"], "receipt_sha256": digest(receipt_before),
                "configuration_sha256": configuration["sha256"], "verifier_sha256": self.verifier_sha256,
                "family": family, "profile": intent["profile"], "scope": READBACK_SCOPE, **target}

    def recovery_manifest(self, request):
        """Export only explicitly selected, currently intact retained requests."""
        keys(request, ("job_ids",))
        identifiers = request["job_ids"]
        if (not isinstance(identifiers, list) or not 1 <= len(identifiers) <= 256
                or any(not isinstance(identifier, str) or not REQUEST_ID.fullmatch(identifier)
                       for identifier in identifiers) or len(set(identifiers)) != len(identifiers)):
            raise SetupError("invalid_request")
        # Like state/job reads, this remains available while a worker is running
        # or admission is closing. No verifier, worker or output file is read.
        with self.lock:
            root, entries = None, []
            for identifier in identifiers:
                if identifier not in self.jobs:
                    raise SetupError("unknown_job", 404)
                intents = [owner[identifier] for owner in
                           (self.install_intents, self.acquisition_intents, self.export_intents)
                           if identifier in owner]
                anchor = intents[0].get("recovery_anchor") if len(intents) == 1 else None
                if (not isinstance(anchor, dict) or anchor.get("id") != identifier
                        or not isinstance(anchor.get("root"), Path)
                        or not selected_hash(anchor.get("sha256"))
                        or self.jobs[identifier].get("recovery_request") !=
                        {"id": identifier, "sha256": anchor["sha256"]}):
                    raise SetupError("recovery_manifest_unavailable", 409)
                if root is not None and root != anchor["root"]:
                    raise SetupError("recovery_manifest_unavailable", 409)
                root = anchor["root"]
                entries.append({"id": identifier, "sha256": anchor["sha256"]})
            raw = encoded({"schema_version": 1, "workspace": str(root), "requests": entries})
            read_recovery_selection(raw)
            return {"scope": "retained_setup_requests_only", "filename": "toolkit-recovery.json",
                    "media_type": "application/json", "text": raw.decode("utf-8"),
                    "bytes": len(raw), "sha256": digest(raw)}

    def dispatch(self, path, value):
        if path == "/api/state":
            keys(value, ())
            return self.state()
        if path in {"/api/compose_" + action for action in ("prepare", "start", "observe", "resume", "stop", "remove",
                    "prepare_successor", "adopt_successor", "reconcile_successor", "rebind")}:
            return self.compose_action(path.removeprefix("/api/compose_"), value)
        routes = {"/api/catalog": self.catalog, "/api/catalog_entry": self.catalog_entry,
                  "/api/jobs": self.job_summaries, "/api/job": self.job_detail,
                  "/api/recovery_manifest": self.recovery_manifest,
                  "/api/stop": self.stop,
                  "/api/recipes": self.recipes, "/api/recipe": self.recipe, "/api/remember": self.remember,
                  "/api/remember_remote": self.remember_remote, "/api/export_recipe": self.export_recipe,
                  "/api/retry_export": self.retry_export,
                  "/api/catalog_route": self.catalog_route, "/api/successor": self.successor,
                  "/api/prepare": self.prepare, "/api/retry_prepare": self.retry_prepare, "/api/help": self.help,
                  "/api/check": self.check, "/api/install": self.install, "/api/reconcile": self.reconcile,
                  "/api/inspect_package": self.inspect_package,
                  "/api/inspect_schema": self.inspect_schema,
                  "/api/inspect_plugin": self.inspect_plugin,
                  "/api/inspect_mcpb": self.inspect_mcpb,
                  "/api/plugin_source": self.plugin_source,
                  "/api/inspect_acquired": self.inspect_acquired,
                  "/api/selected_check": self.selected_check, "/api/selected_install": self.selected_install,
                  "/api/selected_retry": self.selected_retry,
                  "/api/acquire_python": self.acquire_python, "/api/acquire_node": self.acquire_node,
                  "/api/acquire_node_prebuilt": self.acquire_node_prebuilt,
                  "/api/search_node": self.search_node,
                  "/api/search_github": self.search_github, "/api/read_github": self.read_github,
                  "/api/acquire_compose": self.acquire_compose, "/api/acquire_mcpb": self.acquire_mcpb}
        if path not in routes:
            raise SetupError("not_found", 404)
        return routes[path](value)

    def join(self):
        for thread in self.threads:
            thread.join()


class Server(ThreadingHTTPServer):
    daemon_threads = False
    block_on_close = True
    allow_reuse_address = False

    def __init__(self, controller: Controller, port: int = 0, assets: Path | None = None):
        self.controller = controller
        self.capability = secrets.token_urlsafe(32)
        self.assets = assets or Path(__file__).parent / "static"
        self.slots = threading.BoundedSemaphore(8)
        super().__init__(("127.0.0.1", port), Handler)
        self.host = "127.0.0.1:" + str(self.server_address[1])
        self.origin = "http://" + self.host

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()

    def handle_error(self, request, client_address):
        # Base implementation prints a traceback and may include request data.
        pass


def write_launcher(server):
    """Publish the private browser capability without logging its value."""
    launcher = server.controller.workspace / "start.html"
    url = server.origin + "/#session=" + server.capability
    write_new(launcher, ("<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer>"
                         '<title>Toolkit local setup</title><a rel=noreferrer href="' + url + '">Open local setup</a>\n').encode("ascii"))
    return launcher


class BrowserCompanion:
    """Own HTTP resources only; borrow the setup session's existing Controller."""

    def __init__(self, controller: Controller, port: int = 0, assets: Path | None = None):
        if type(port) is not int or not 0 <= port <= 65535:
            raise SetupError("invalid_startup")
        self.server = None
        self.thread = None
        self._stop = threading.Event()
        self._close_lock = threading.Lock()
        self._closed = False
        try:
            self.server = Server(controller, port, assets)
            self.server.timeout = 0.2
            self.launcher = write_launcher(self.server)
            self.metadata = {"origin": self.server.origin, "launcher": str(self.launcher), "scope": controller.scope()}
            self.thread = threading.Thread(target=self._serve, name="consumer-setup-browser", daemon=False)
            self.thread.start()
        except BaseException:
            self.close()
            raise

    def _serve(self):
        try:
            # handle_request has a bounded idle timeout and needs no separate
            # serve_forever/shutdown startup handshake. The existing Server
            # still owns request parsing, bounds, authentication and dispatch.
            while not self._stop.is_set():
                self.server.handle_request()
        except BaseException:
            self._stop.set()
            print("consumer setup browser unavailable; preserve the workspace", file=sys.stderr, flush=True)
        finally:
            # An unexpected accept-loop failure closes its listener too. This
            # joins HTTP handlers only, never Controller installation workers.
            self.server.server_close()

    def close(self):
        with self._close_lock:
            if self._closed:
                return
            self._stop.set()
            try:
                # start() may fail before or after a thread actually begins.
                if self.thread is not None and self.thread.ident is not None:
                    self.thread.join()
            finally:
                if self.server is not None:
                    self.server.server_close()
            self._closed = True


class Handler(BaseHTTPRequestHandler):
    server_version = "ToolkitLocalSetup"
    sys_version = ""
    protocol_version = "HTTP/1.0"

    def setup(self):
        super().setup()
        self.connection.settimeout(5)

    def log_message(self, format, *args):
        pass

    def send_error(self, code, message=None, explain=None):
        self.respond(code, encoded({"error": "invalid_request"}))

    def respond(self, status, data, content_type="application/json; charset=utf-8"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'")
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def boundary(self, api=False):
        if len(self.path) > 2048 or sum(len(key) + len(value) for key, value in self.headers.items()) > 8192:
            raise SetupError("invalid_request")
        if self.headers.get_all("Host") != [self.server.host]:
            raise SetupError("request_not_allowed", 403)
        origins = self.headers.get_all("Origin")
        if (api and origins != [self.server.origin]) or (not api and origins not in (None, [self.server.origin])):
            raise SetupError("request_not_allowed", 403)
        if api and self.headers.get("Sec-Fetch-Site") not in (None, "none", "same-origin"):
            raise SetupError("request_not_allowed", 403)
        if self.headers.get_all("Transfer-Encoding"):
            raise SetupError("invalid_request")
        if api:
            tokens = self.headers.get_all("X-Toolkit-Session")
            if not tokens or len(tokens) != 1 or not hmac.compare_digest(tokens[0], self.server.capability):
                raise SetupError("request_not_allowed", 403)

    def do_GET(self):
        try:
            self.boundary()
            assets = {"/": ("index.html", "text/html"), "/index.html": ("index.html", "text/html"),
                      "/app.js": ("app.js", "text/javascript"), "/form.js": ("form.js", "text/javascript"),
                      "/style.css": ("style.css", "text/css")}
            if self.path not in assets:
                raise SetupError("not_found", 404)
            name, kind = assets[self.path]
            data = read_file(self.server.assets, name, 512 * 1024)
            self.respond(200, data, kind + "; charset=utf-8")
        except SetupError as error:
            self.respond(error.status, encoded(setup_error_payload(error)))
        except Exception:
            self.respond(500, encoded({"error": "request_failed"}))

    def do_POST(self):
        try:
            self.boundary(api=True)
            lengths = self.headers.get_all("Content-Length")
            if not lengths or len(lengths) != 1 or not re.fullmatch(r"[0-9]{1,6}", lengths[0]):
                raise SetupError("invalid_request")
            length = int(lengths[0])
            if not 1 <= length <= MAX_BODY:
                raise SetupError("request_too_large", 413)
            types = self.headers.get_all("Content-Type")
            if not types or len(types) != 1 or types[0].lower() not in ("application/json", "application/json; charset=utf-8"):
                raise SetupError("invalid_request")
            data = self.rfile.read(length)
            if len(data) != length:
                raise SetupError("invalid_request")
            result = self.server.controller.dispatch(self.path, decode(data))
            self.respond(200, encoded(result))
        except SetupError as error:
            self.respond(error.status, encoded(setup_error_payload(error)))
        except Exception:
            self.respond(500, encoded({"error": "request_failed"}))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--bundle")
    parser.add_argument("--contents-sha256")
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--families", default="agno,services,activepieces")
    parser.add_argument("--python", required=True)
    parser.add_argument("--node")
    parser.add_argument("--npm-cli")
    parser.add_argument("--selected-node", help="explicit selected Node/MCPB runtime; npm required for Node packages, optional for MCPB-only profiles")
    parser.add_argument("--selected-npm-cli", help="npm CLI for selected Node installation; requires --selected-node")
    parser.add_argument("--verifier", type=Path, help="explicit trusted installer script supporting --verify-existing; reconciliation is disabled when omitted")
    parser.add_argument("--verifier-sha256", help="SHA-256 of the exact explicitly selected verifier script")
    parser.add_argument("--selected-install-profile", type=Path, help="host-pinned standing local software installation profile")
    parser.add_argument("--selected-install-profile-sha256")
    parser.add_argument("--python-acquisition-profile", type=Path,
                        help="explicit trusted public PyPI acquisition scope and pinned runtime/pip")
    parser.add_argument("--python-acquisition-profile-sha256")
    parser.add_argument("--node-acquisition-profile", type=Path,
                        help="explicit trusted public npm acquisition scope and pinned runtime/npm")
    parser.add_argument("--node-acquisition-profile-sha256")
    parser.add_argument("--node-prebuilt-profile", type=Path)
    parser.add_argument("--node-prebuilt-profile-sha256")
    parser.add_argument("--mcpb-acquisition-profile", type=Path,
                        help="explicit trusted public MCPB download origins and pinned Python")
    parser.add_argument("--mcpb-acquisition-profile-sha256")
    parser.add_argument("--compose-acquisition-profile", type=Path,
                        help="host-pinned image acquisition and proposal preparation")
    parser.add_argument("--compose-acquisition-profile-sha256")
    parser.add_argument("--compose-profile", type=Path, help="explicit standing local container execution scope and exact backend profile")
    parser.add_argument("--compose-profile-sha256")
    parser.add_argument("--local-recipes-profile", type=Path, help="host-designated scope for nonsecret recipe disclosure and optional retention")
    parser.add_argument("--local-recipes-profile-sha256")
    parser.add_argument("--allow-remote-recipes", action="store_true", help="explicitly allow remote recipe retention and local MCP configuration export with a retention profile")
    parser.add_argument("--allow-acquired-inspection", action="store_true", help="allow bounded inert primary archive research for completed live Python/Node acquisitions")
    parser.add_argument("--allow-node-search", action="store_true", help="explicit bounded public npm search using the pinned Node/npm profile")
    parser.add_argument("--allow-public-github-research", action="store_true",
                        help="explicit anonymous public GitHub repository search and bounded text reads")
    parser.add_argument("--prior-request", type=Path, help="host-designated nonsecret v1 historical configured request; never executed or treated as installation proof")
    parser.add_argument("--prior-request-sha256", help="exact digest of the host-selected historical input")
    parser.add_argument("--recover-workspace", type=Path, help="canonical prior workspace under exclusive quiescent host custody")
    parser.add_argument("--recover-request", help="one original install request ID; readback only")
    parser.add_argument("--recover-request-sha256", help="independently host-selected original request SHA-256")
    parser.add_argument("--recover-manifest", type=Path, help="host-selected request list from one quiescent prior workspace; readback only")
    parser.add_argument("--recover-manifest-sha256", help="SHA-256 of the exact host-selected recovery manifest")
    parser.add_argument("--playwright-host-profile", type=Path)
    parser.add_argument("--playwright-host-profile-sha256")
    parser.add_argument("--official-bundle", type=Path)
    parser.add_argument("--official-contents-sha256")
    parser.add_argument("--official-installer", type=Path)
    parser.add_argument("--official-installer-sha256")
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args(argv)
    controller = server = None
    try:
        if not 0 <= args.port <= 65535:
            raise SetupError("invalid_startup")
        controller = Controller(Path(args.bundle) if args.bundle else None, args.contents_sha256, Path(args.workspace),
                                tuple(args.families.split(",")) if args.families else (), args.python, args.node, args.npm_cli,
                                selected_node=args.selected_node, selected_npm_cli=args.selected_npm_cli,
                                verifier=args.verifier, verifier_sha256=args.verifier_sha256,
                                recover_workspace=args.recover_workspace, recover_request=args.recover_request,
                                recover_request_sha256=args.recover_request_sha256,
                                recover_manifest=args.recover_manifest, recover_manifest_sha256=args.recover_manifest_sha256,
                                official_bundle=args.official_bundle, official_contents_sha256=args.official_contents_sha256,
                                official_installer=args.official_installer, official_installer_sha256=args.official_installer_sha256,
                                playwright_host_profile=args.playwright_host_profile, playwright_host_profile_sha256=args.playwright_host_profile_sha256,
                                prior_request_path=args.prior_request, prior_request_sha256=args.prior_request_sha256,
                                selected_install_profile=args.selected_install_profile,
                                selected_install_profile_sha256=args.selected_install_profile_sha256,
                                python_acquisition_profile=args.python_acquisition_profile,
                                python_acquisition_profile_sha256=args.python_acquisition_profile_sha256,
                                node_acquisition_profile=args.node_acquisition_profile,
                                node_acquisition_profile_sha256=args.node_acquisition_profile_sha256,
                                node_prebuilt_profile=args.node_prebuilt_profile,
                                node_prebuilt_profile_sha256=args.node_prebuilt_profile_sha256,
                                mcpb_acquisition_profile=args.mcpb_acquisition_profile,
                                mcpb_acquisition_profile_sha256=args.mcpb_acquisition_profile_sha256,
                                compose_acquisition_profile=args.compose_acquisition_profile,
                                compose_acquisition_profile_sha256=args.compose_acquisition_profile_sha256,
                                compose_profile=args.compose_profile, compose_profile_sha256=args.compose_profile_sha256,
                                local_recipes_profile=args.local_recipes_profile,
                                local_recipes_profile_sha256=args.local_recipes_profile_sha256,
                                allow_remote_recipes=args.allow_remote_recipes,
                                allow_acquired_inspection=args.allow_acquired_inspection, allow_node_search=args.allow_node_search,
            allow_public_github_research=args.allow_public_github_research)
        server = Server(controller, args.port)
        launcher = write_launcher(server)
        print(json.dumps({"origin": server.origin, "launcher": str(launcher), "scope": controller.scope()}), flush=True)
        # A terminal signal stops HTTP acceptance but joins an already admitted
        # installer. A forcibly killed host has no automatic restart/replay.
        def stop(signum, frame):
            controller.close_admission()
            raise KeyboardInterrupt
        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGINT, stop)
        try:
            server.serve_forever(poll_interval=0.2)
        except KeyboardInterrupt:
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            signal.signal(signal.SIGINT, signal.SIG_IGN)
        return 0
    except SetupError as error:
        print(error.code + "; preserve the selected workspace and start with a new directory", file=sys.stderr)
        return 2
    except Exception:
        print("consumer setup unavailable; preserve the selected workspace and start with a new directory", file=sys.stderr)
        return 2
    finally:
        if controller:
            controller.close_admission()
        if server:
            server.server_close()
        if controller:
            controller.join()


if __name__ == "__main__":
    raise SystemExit(main())
