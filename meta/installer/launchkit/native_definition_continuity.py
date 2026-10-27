"""Pure projections of one native-owned model registry transition.

This module is exported unchanged into generated applications. It neither
admits runtime versions nor writes, migrates or substitutes native state.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import json
import re

NATIVE_VERSION = "0.8.47"
SUPPORTED_NATIVE_VERSIONS = {"0.8.46", "0.8.47", "0.8.48"}
CORE_VERSION = "0.8.51"
LEGACY_MODEL_ID = "gpt-6.1-sol"
LEGACY_MODEL_METADATA = {
    "provider": "openai",
    "display_name": "GPT-6.1 Sol",
    "context_window": 1050000,
    "max_output_tokens": 128000,
    "vision": True,
    "call_timeout_secs": 600,
}
CONTINUITY_SOURCE = "native exact legacy registry removal"
MAX_DEFINITION_BYTES = 2 * 1024 * 1024
_DEFINITION_FIELDS = {
    "runtime", "id", "orchestrator", "profiles", "models",
    "image_generation_provider", "wiring", "skills", "backend", "flows",
    "topology", "supervisor", "limits", "spawn_policy", "event_router",
}
_PROOF_FIELDS = {
    "source", "mob_id", "definition_revision", "native_version", "core_version",
    "creation_cursor", "update_cursor", "previous_definition_sha256",
    "current_definition_sha256",
}


def _exact(value):
    # JSON spellings distinguish integer metadata from booleans and floats.
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)


def legacy_registry_successor(previous, *, native_version=NATIVE_VERSION, core_version=CORE_VERSION):
    """Remove only the exact former compiler entry now owned by Core 0.8.51."""
    if native_version not in SUPPORTED_NATIVE_VERSIONS or core_version != CORE_VERSION:
        raise ValueError("Legacy model removal requires qualified MobKit 0.8.46/0.8.47/0.8.48 with Core 0.8.51.")
    expected = {LEGACY_MODEL_ID: LEGACY_MODEL_METADATA}
    if type(previous) is not dict or _exact(previous.get("models")) != _exact(expected):
        raise ValueError("The saved custom model is not the exact legacy compiler declaration.")
    successor = deepcopy(previous)
    successor["models"] = {}
    return successor


def is_legacy_registry_removal(previous, current):
    """Require every field except the redundant legacy declaration unchanged."""
    try:
        successor = legacy_registry_successor(previous)
        if type(current) is not dict:
            return False
        comparable = deepcopy(current)
        comparable.setdefault("models", {})
        return _exact(successor) == _exact(comparable)
    except (ValueError, TypeError, RecursionError):
        return False


def continuity_query(mob_id):
    """Read from creation; a third matching event refuses this exact pair."""
    if type(mob_id) is not str or not mob_id:
        raise ValueError("Native continuity needs an exact mob identity.")
    return {"mob_id": mob_id, "event_types": [
        "mob_created", "mob_definition_updated", "mob_reset"],
        "after_seq": 0, "limit": 3}


def _definition(value, mob_id):
    if (type(value) is not dict or value.get("id") != mob_id
            or set(value) not in (_DEFINITION_FIELDS, _DEFINITION_FIELDS | {"source_identity"})
            or any(type(value.get(name)) is not dict for name in (
                "runtime", "profiles", "models", "wiring", "skills", "backend", "flows"))):
        raise ValueError("Native continuity needs a complete canonical definition.")
    encoded = _exact(value).encode("utf-8")
    if len(encoded) > MAX_DEFINITION_BYTES:
        raise ValueError("The native definition is too large for bounded continuity observation.")
    return encoded


def registry_role_continuity(page, mob_id, *, native_version=NATIVE_VERSION):
    """Project a full native creation/update pair; never infer a role update.

    The caller must use continuity_query through the qualified native runtime.
    Exactly two matches below its limit prove there is no third definition or
    reset in this forward scan. Other event kinds can lie between the cursors.
    """
    continuity_query(mob_id)
    if native_version not in SUPPORTED_NATIVE_VERSIONS:
        raise ValueError("Native role continuity needs a qualified release tuple.")
    if type(page) is not dict or set(page) != {"events", "next_after_seq"}:
        raise ValueError("Native continuity needs the complete structural query result.")
    events = page["events"]
    if type(events) is not list or len(events) != 2:
        raise ValueError("Native continuity needs exactly one creation and its registry successor.")
    previous_cursor = 0
    definitions = []
    for index, (event, kind) in enumerate(zip(events, ("mob_created", "mob_definition_updated"))):
        if (type(event) is not dict or event.get("mob_id") != mob_id
                or event.get("kind") != kind or type(event.get("cursor")) is not int
                or event["cursor"] <= previous_cursor):
            raise ValueError("Native continuity returned a foreign or unordered structural event.")
        data = event.get("data")
        keys = {"type", "definition"} | ({"epoch"} if index else set())
        if (type(data) is not dict or set(data) != keys or data.get("type") != kind
                or index and (type(data.get("epoch")) is not int or data["epoch"] != 2)):
            raise ValueError("Native continuity returned an invalid definition epoch or event.")
        definitions.append(_definition(data["definition"], mob_id))
        previous_cursor = event["cursor"]
    if type(page["next_after_seq"]) is not int or page["next_after_seq"] != previous_cursor:
        raise ValueError("Native continuity returned an invalid query cursor.")
    if not is_legacy_registry_removal(events[0]["data"]["definition"], events[1]["data"]["definition"]):
        raise ValueError("Native history changed more than the exact legacy model declaration.")
    return {"source": CONTINUITY_SOURCE, "mob_id": mob_id, "definition_revision": 2,
        "native_version": native_version, "core_version": CORE_VERSION,
        "creation_cursor": events[0]["cursor"], "update_cursor": events[1]["cursor"],
        "previous_definition_sha256": hashlib.sha256(definitions[0]).hexdigest(),
        "current_definition_sha256": hashlib.sha256(definitions[1]).hexdigest()}


def valid_continuity_projection(value, mob_id, revision):
    """Validate the bounded projection emitted by the owned native reader.

    This validates an observation's shape, not native authority by itself.
    The Installer separately binds its owned reader, package and official
    runtime before it can use this projection.
    """
    return (type(value) is dict and set(value) == _PROOF_FIELDS
        and value.get("source") == CONTINUITY_SOURCE and value.get("mob_id") == mob_id
        and type(revision) is int and revision == 2
        and type(value.get("definition_revision")) is int and value["definition_revision"] == revision
        and value.get("native_version") in SUPPORTED_NATIVE_VERSIONS and value.get("core_version") == CORE_VERSION
        and type(value.get("creation_cursor")) is int and value["creation_cursor"] > 0
        and type(value.get("update_cursor")) is int and value["update_cursor"] > value["creation_cursor"]
        and all(type(value.get(key)) is str and re.fullmatch("[0-9a-f]{64}", value[key])
            for key in ("previous_definition_sha256", "current_definition_sha256")))


def verify_continuity_runtime(root, gateway, *, native_version):
    """Use the existing release receipt verifier before inherited-role use.

    Core 0.8.51 is the fixed dependency of these supported MobKit tuples.
    Verification still requires matching SDK and owned official-release receipts.
    No binary is executed, installed or downloaded here.
    """
    import importlib.metadata
    import importlib.util
    import os
    from pathlib import Path

    root = Path(root)
    expected = root / ".bin/rpc_gateway"
    if (native_version not in SUPPORTED_NATIVE_VERSIONS or os.environ.get("MOBKIT_RPC_GATEWAY_BIN")
            or Path(gateway).absolute() != expected.absolute()
            or importlib.metadata.version("meerkat-mobkit") != native_version):
        raise RuntimeError("Inherited role continuity needs the qualified official MobKit gateway and matching SDK version without gateway overrides; no turn was sent.")
    spec = importlib.util.spec_from_file_location("_mobkit_continuity_release", root / "install_gateway.py")
    if spec is None or spec.loader is None:
        raise RuntimeError("The installed official release verifier is unavailable; no turn was sent.")
    release = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(release)
    if (release.VERSION != native_version or release.ROOT != root
            or not release.verified_cache(root / ".bin", "rpc_gateway", release.platform_target())):
        raise RuntimeError("Inherited role continuity needs the owned pinned official RPC receipt and unchanged executable; no turn was sent.")
