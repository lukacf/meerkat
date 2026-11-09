"""Join remote interface observations to the exact accepted declarations."""
from __future__ import annotations

from copy import deepcopy
import re

from .application_experiences import declared_application_experiences

HASH = re.compile(r"[a-f0-9]{64}")
ENTRY = re.compile(r"application:[a-z][a-z0-9_-]{0,63}")


def application_observation(files, receipt, observed, console_port):
    """Return only source-joined, bounded, nonsecret destination observations."""
    unavailable = {"protocol": 1, "supported": False, "status": "unavailable",
        "source_digest": receipt["accepted_source_digest"],
        "installed_source_digest": receipt["transferred_source_digest"],
        "metadata_sha256": None, "entries": []}
    if observed is None:
        return unavailable
    try:
        import json
        declarations = declared_application_experiences(files, json.loads(files["deployment.json"][0]))
        if (not isinstance(observed, dict) or observed.get("protocol") != 1
                or observed.get("supported") is not True
                or observed.get("status") not in {"resolved", "none", "unavailable"}
                or observed.get("source_digest") != receipt["accepted_source_digest"]
                or observed.get("installed_source_digest") != receipt["transferred_source_digest"]
                or observed.get("metadata_sha256") != declarations["metadata_sha256"]
                or not isinstance(observed.get("entries"), list)):
            return unavailable
        if observed["status"] == "unavailable":
            return {**unavailable, "supported": True, "metadata_sha256": declarations["metadata_sha256"]}
        rows = observed["entries"]
        if len(rows) != len(declarations["entries"]) or observed["status"] != ("resolved" if rows else "none"):
            return unavailable
        result = {**unavailable, "supported": True, "status": observed["status"],
            "metadata_sha256": declarations["metadata_sha256"], "entries": []}
        for declared, row in zip(declarations["entries"], rows):
            if (not isinstance(row, dict) or any(row.get(key) != declared[key] for key in ("id", "label", "path"))
                    or row.get("verified") is not False or type(row.get("listener_owned")) is not bool):
                return unavailable
            port = row.get("destination_port")
            if port is not None and (type(port) is not int or not 1024 <= port <= 65535):
                return unavailable
            if "port" in declared and port != declared["port"]:
                return unavailable
            if "port" not in declared and "port_field" not in declared and port != console_port:
                return unavailable
            token = row.get("listener_token")
            if row["listener_owned"] and (port is None or not isinstance(token, str) or not HASH.fullmatch(token)):
                return unavailable
            if not row["listener_owned"] and token is not None:
                return unavailable
            # Destination diagnostics are never copied: fixed text cannot echo
            # environment values or private platform error output.
            reason = None if row["listener_owned"] else (
                "Configure the declared application port, then recheck." if port is None else
                "The installed application has not confirmed ownership of this interface listener. Recheck it.")
            result["entries"].append({**{key: declared[key] for key in ("id", "label", "path")},
                "destination_port": port, "listener_owned": row["listener_owned"],
                "listener_token": token, "reason": reason, "verified": False})
        return result
    except (ValueError, TypeError, KeyError, UnicodeError, RecursionError):
        return unavailable


def selected_application_entry(receipt, entry_id):
    if not isinstance(entry_id, str) or not ENTRY.fullmatch(entry_id):
        return None
    observation = receipt.get("application_experiences")
    if (not isinstance(observation, dict) or observation.get("protocol") != 1
            or observation.get("supported") is not True or observation.get("status") != "resolved"
            or observation.get("source_digest") != receipt.get("accepted_source_digest")
            or observation.get("installed_source_digest") != receipt.get("transferred_source_digest")
            or not isinstance(observation.get("metadata_sha256"), str)
            or not HASH.fullmatch(observation["metadata_sha256"])):
        return None
    rows = [row for row in observation.get("entries", []) if row.get("id") == entry_id.partition(":")[2]]
    if len(rows) != 1:
        return None
    return {**deepcopy(rows[0]), "metadata_sha256": observation["metadata_sha256"]}


def application_available(receipt):
    observation = receipt.get("application_experiences")
    return bool(isinstance(observation, dict) and observation.get("protocol") == 1
        and observation.get("supported") is True and observation.get("status") == "resolved"
        and observation.get("source_digest") == receipt.get("accepted_source_digest")
        and observation.get("installed_source_digest") == receipt.get("transferred_source_digest")
        and isinstance(observation.get("metadata_sha256"), str)
        and HASH.fullmatch(observation["metadata_sha256"]) and observation.get("entries"))
