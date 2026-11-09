"""Source-bound, optional human entry links without executing application code.

Declarations describe where authored interfaces expect to be served. They are
not observations of availability, authentication or a working user transaction.
Callers supply one exact accepted source snapshot and its checked source digest.
"""
from __future__ import annotations

import hashlib
import json
import re
from urllib.parse import unquote, urlsplit

from .templates_v2.application_configuration import MAX_VALUE_LENGTH, validate_application_fields

EXPERIENCE_PATH = "application/experience.json"
MAX_METADATA_BYTES = 64_000
MAX_ENTRIES = 12
LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1"}


def _content(files, path):
    if isinstance(files, dict):
        item = files.get(path)
        value = item[0] if isinstance(item, tuple) else item
    else:
        items = [item for item in files if item.path == path]
        if len(items) > 1:
            raise ValueError("The accepted source contains a duplicate interface metadata path.")
        value = items[0].content if items else None
    if value is None:
        return None
    if isinstance(value, str):
        value = value.encode("utf-8")
    if not isinstance(value, bytes):
        raise ValueError("Interface metadata needs ordinary UTF-8 source bytes.")
    return value


def _json(content, label, maximum=MAX_METADATA_BYTES):
    if len(content) > maximum:
        raise ValueError(f"{label} exceeds its source metadata size limit.")
    def finite(_):
        raise ValueError(f"{label} must contain finite ordinary JSON.")
    def unique(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError(f"{label} contains duplicate fields.")
            value[key] = item
        return value
    try:
        return json.loads(content.decode("utf-8"), parse_constant=finite, object_pairs_hook=unique)
    except (UnicodeError, json.JSONDecodeError, RecursionError):
        raise ValueError(f"{label} must contain bounded ordinary UTF-8 JSON.") from None


def _port(value):
    if type(value) is not int or not 1024 <= value <= 65535:
        raise ValueError("An interface port must be an integer from 1024 to 65535.")
    return value


def _path(value):
    if (not isinstance(value, str) or not value.startswith("/") or value.startswith("//")
            or len(value) > 1000 or any(ord(char) < 33 or ord(char) == 127 for char in value)):
        raise ValueError("An interface path must be a visible absolute loopback URL path.")
    decoded = value
    for _ in range(4):
        if (urlsplit(decoded).scheme or urlsplit(decoded).netloc or any(char in decoded for char in "\\?#")
                or any(part in {".", ".."} for part in decoded.split("/"))
                or decoded.startswith("//") or any(ord(char) < 33 or ord(char) == 127 for char in decoded)):
            raise ValueError("Interface paths cannot contain external URLs, credentials, queries or traversal.")
        try:
            updated = unquote(decoded, errors="strict")
        except UnicodeError:
            raise ValueError("An interface path must use ordinary UTF-8 escaping.") from None
        if updated == decoded:
            break
        decoded = updated
    else:
        raise ValueError("An interface path has excessive nested escaping.")
    return value


def _fields(files, definition):
    rows = definition.get("environment", [])
    fields = {row["key"]: row for row in rows if isinstance(row, dict) and isinstance(row.get("key"), str)}
    content = _content(files, "application/configuration.json")
    if content is not None:
        fields.update({row["key"]: row for row in validate_application_fields(
            _json(content, "Application configuration", 256_000), fields)})
    fields["MOBKIT_PORT"] = {"key": "MOBKIT_PORT", "secret": False}
    return fields


def declared_application_experiences(files, definition):
    """Read an optional declaration from exact package files, without imports.

    ``files`` is a compiler PackageFile list or deployment.package_files mapping.
    This helper never reads installed paths, environment values or credentials.
    Invalid metadata raises before any partial list can become clickable.
    """
    content = _content(files, EXPERIENCE_PATH)
    result = {"metadata_sha256": None, "entries": []}
    if content is None:
        return result
    value = _json(content, "Application interface metadata")
    if not isinstance(value, dict) or set(value) != {"entries"}:
        raise ValueError("Application interface metadata declares only an entries list.")
    entries = value["entries"]
    if not isinstance(entries, list) or not 1 <= len(entries) <= MAX_ENTRIES:
        raise ValueError("Application interface metadata needs 1-12 entry links.")
    fields, seen = _fields(files, definition), set()
    for index, item in enumerate(entries, 1):
        if not isinstance(item, dict) or set(item) - {"id", "label", "path", "port", "port_field"}:
            raise ValueError("An application entry link has unsupported fields.")
        identity = item.get("id", f"interface-{index}")
        if not isinstance(identity, str) or not re.fullmatch(r"[a-z][a-z0-9_-]{0,63}", identity) or identity in seen:
            raise ValueError("Application entry links need distinct stable identifiers.")
        seen.add(identity)
        label = item.get("label")
        if (not isinstance(label, str) or not label.strip() or len(label) > 160
                or any(ord(char) < 32 or ord(char) == 127 for char in label)):
            raise ValueError("Each application entry link needs a visible label of 1-160 characters.")
        row = {"id": identity, "label": label.strip(), "path": _path(item.get("path"))}
        if "port" in item and "port_field" in item:
            raise ValueError("Choose one literal port or a declared nonsecret port field.")
        if "port" in item:
            row["port"] = _port(item["port"])
        elif "port_field" in item:
            key = item["port_field"]
            if not isinstance(key, str) or key not in fields or fields[key].get("secret") is not False:
                raise ValueError("An interface port field must be declared and explicitly nonsecret.")
            row["port_field"] = key
        result["entries"].append(row)
    result["metadata_sha256"] = hashlib.sha256(content).hexdigest()
    return result


def _console_origin(value):
    if value is None:
        return None
    try:
        url = urlsplit(value)
        port = _port(url.port)
    except (TypeError, ValueError):
        raise ValueError("Installed interface links need an observed loopback Console URL.") from None
    if (url.scheme != "http" or url.hostname not in LOOPBACK_HOSTS or url.username or url.password
            or url.query or url.fragment or url.path not in {"/console", "/console/"}):
        raise ValueError("Installed interface links need an observed loopback Console URL.")
    return url.hostname, port


def resolve_application_experiences(files, definition, *, source_digest, console_url=None,
                                    configuration_values=None):
    """Resolve source-declared ports without creating links or claiming readiness.

    An omitted port uses the actual observed Console, never a compiled default.
    Values are supplied by the caller from declared nonsecret destination fields.
    """
    if not isinstance(source_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", source_digest):
        raise ValueError("Application entry links need the exact accepted source digest.")
    declared = declared_application_experiences(files, definition)
    origin = _console_origin(console_url)
    if configuration_values is not None and not isinstance(configuration_values, dict):
        raise ValueError("Interface port configuration must be a host-owned mapping.")
    values = configuration_values or {}
    result = {**declared, "source_digest": source_digest, "scope": "declared-source",
              "verified": False, "entries": []}
    for item in declared["entries"]:
        port, reason = item.get("port"), None
        if "port_field" in item:
            raw = values.get(item["port_field"])
            if isinstance(raw, str) and len(raw) <= MAX_VALUE_LENGTH and re.fullmatch(r"[+-]?[0-9]+", raw):
                digits = raw[1:] if raw[0] in "+-" else raw
                significant = digits.lstrip("0") or "0"
                # Match configuration integer text without changing saved values.
                if len(significant) <= 16:
                    raw = int(significant) * (-1 if raw[0] == "-" else 1)
            try:
                port = _port(raw)
            except ValueError:
                reason = "Configure this declared nonsecret port through private setup."
        elif port is None:
            if origin:
                port = origin[1]
            else:
                reason = "Start and verify this installed application's native Console before opening its declared interfaces."
        result["entries"].append({**item, "destination_port": port, "reason": reason, "verified": False})
    return result


def project_application_experiences(files, definition, *, source_digest, console_url=None,
                                    configuration_values=None, remote=False, tunnel_mappings=None):
    """Project declared links; supplied observations do not attest the UI.

    A local link requires the caller's current installed Console observation.
    For remote destinations, mappings must explicitly name the resolved port.
    """
    resolved = resolve_application_experiences(files, definition, source_digest=source_digest,
        console_url=console_url, configuration_values=configuration_values)
    origin = _console_origin(console_url)
    if tunnel_mappings is not None and not isinstance(tunnel_mappings, dict):
        raise ValueError("Remote entry links need explicit port mappings.")
    mappings = {}
    for destination, local in (tunnel_mappings or {}).items():
        if isinstance(destination, str) and re.fullmatch(r"[0-9]{4,5}", destination):
            destination = int(destination)
        mappings[_port(destination)] = _port(local)
    result = {**resolved,
              "detail": "Authored entry links are unverified. They do not prove interface availability, login or a working user transaction.",
              "entries": []}
    for item in resolved["entries"]:
        row = {**item, "status": "declared", "verified": False, "url": None}
        port = item["destination_port"]
        if not row["reason"]:
            if remote:
                local = mappings.get(port)
                if local is None:
                    row["reason"] = "Create an explicit authenticated tunnel mapping for this interface port."
                else:
                    row["url"] = f"http://127.0.0.1:{local}{item['path']}"
                    row["tunnel_mapping"] = {"destination_port": port, "local_port": local}
            elif not origin:
                row["reason"] = "Start and verify this installed application's native Console before opening its declared interfaces."
            else:
                host = f"[{origin[0]}]" if origin[0] == "::1" else origin[0]
                row["url"] = f"http://{host}:{port}{item['path']}"
        result["entries"].append(row)
    return result
