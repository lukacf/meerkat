"""Validate application intent and retain input for the native policy compiler.

This module does not compile policy, construct ToolIdentity values or enroll an
operator. The existing native compiler owns canonicalization and validation.
The trusted application caller supplies the real member identities; browser
claims, display labels and class names are not a member inventory.
"""
from __future__ import annotations

import hashlib
import json
from collections.abc import Sequence


MAX_POLICY_SELECTION_BYTES = 16 * 1024 * 1024
APPLICATION_INVENTORY_PATH = "application/native-inventory.json"
APPLICATION_NATIVE_POLICY_PATH = "application/native-tool-policy.json"


def application_tool_policy_path(deployment):
    """Select the policy source named by the compiler's existing route.

    Application-authored filenames and file presence cannot select a route.
    Older accepted packages continue reading their original policy source.
    """
    if not isinstance(deployment, dict):
        raise ValueError("Application policy selection needs its compiled deployment source.")
    if "applicationInventory" not in deployment:
        return "application/tool-policy.json"
    if deployment["applicationInventory"] != APPLICATION_INVENTORY_PATH:
        raise ValueError("Preserve the compiled application inventory source path.")
    return APPLICATION_NATIVE_POLICY_PATH


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Repeated policy selection field: {key}")
        result[key] = value
    return result


def _text(value, field):
    if not isinstance(value, str) or not value or value.strip() != value:
        raise ValueError(f"{field} must be exact nonempty text.")
    try:
        value.encode("utf-8")
    except UnicodeError as error:
        raise ValueError(f"{field} must be valid Unicode text.") from error
    return value


def application_tool_policy_selection(
    selection_bytes: bytes,
    *,
    selected_member_identities: Sequence[str],
) -> dict:
    """Read one bounded application selection without compiling or granting it.

    The application source uses its actual durable identities. Every selected
    identity appears once, including identities with empty grants. The native
    compiler input instead requires native member identities, joined through
    the actual native inventory by the trusted caller. This parser provides
    neither that mapping nor a compiled policy or execution decision.
    """
    if not isinstance(selection_bytes, bytes):
        raise TypeError("Policy selection must be retained bytes.")
    if not 0 < len(selection_bytes) <= MAX_POLICY_SELECTION_BYTES:
        raise ValueError("Policy selection must be nonempty and at most 16 MiB.")
    if (not isinstance(selected_member_identities, Sequence)
            or isinstance(selected_member_identities, (str, bytes))):
        raise TypeError("The selected member inventory must be a sequence.")
    members = tuple(_text(value, "member identity") for value in selected_member_identities)
    inventory = set(members)
    if len(inventory) != len(members):
        raise ValueError("The selected member inventory contains duplicate identities.")
    try:
        selected = json.loads(selection_bytes.decode("utf-8"), object_pairs_hook=_object,
                              parse_constant=lambda value: _invalid_constant(value))
    except (UnicodeError, json.JSONDecodeError, RecursionError) as error:
        raise ValueError("Policy selection is not a JSON document.") from error
    if not isinstance(selected, dict) or set(selected) != {
        "provider_id", "policy_id", "revision", "members"
    }:
        raise ValueError("Policy selection must contain only explicit native input fields.")
    for name in ("provider_id", "policy_id"):
        _text(selected[name], name)
    if type(selected["revision"]) is not int or not 1 <= selected["revision"] <= 2**64 - 1:
        raise ValueError("Policy revision must be a positive u64 integer.")
    if not isinstance(selected["members"], list):
        raise ValueError("Policy members must be an explicit list.")
    seen = set()
    for member in selected["members"]:
        if not isinstance(member, dict) or set(member) != {"member_identity", "grants"}:
            raise ValueError("Each member requires its exact identity and explicit grants.")
        identity = _text(member["member_identity"], "member identity")
        if identity not in inventory or identity in seen:
            raise ValueError("Policy members must match the selected member inventory once.")
        seen.add(identity)
        if not isinstance(member["grants"], list):
            raise ValueError("Member grants must be an explicit list.")
        tools = set()
        for grant in member["grants"]:
            if not isinstance(grant, dict) or set(grant) != {"tool_name", "action", "consequence"}:
                raise ValueError("Each grant requires exact tool, action and consequence fields.")
            name = _text(grant["tool_name"], "tool name")
            if name in tools:
                raise ValueError("Member grants contain a repeated exact tool name.")
            tools.add(name)
            # These are the current native wire values, checked early so bad
            # source refuses before application effects. Native compilation
            # remains mandatory and independently validates the contract.
            if grant["action"] != "invoke":
                raise ValueError("A policy action must be the exact native invoke value.")
            if not isinstance(grant["consequence"], str) or grant["consequence"] not in {"r0", "r1", "r2", "r3"}:
                raise ValueError("A policy consequence must be an exact native r0-r3 value.")
    if seen != inventory:
        raise ValueError("Policy selection omitted a selected member.")
    return selected


def application_tool_policy_input(
    selection_bytes: bytes,
    *,
    source_id: str,
    selected_member_identities: Sequence[str],
) -> bytes:
    """Return pre-digest input using an already selected native member inventory.

    This adapter requires selection rows to use actual native member identities,
    not the application's durable roster identities. It does not derive their
    spelling. Native code still owns canonical ordering, digest, schema version
    and default-deny validation. The original bytes provide source provenance;
    output from this function is not a compiled policy artifact.
    """
    source_id = _text(source_id, "source_id")
    selected = application_tool_policy_selection(selection_bytes,
        selected_member_identities=selected_member_identities)
    return _compiler_input(selected, selection_bytes, source_id)


def _compiler_input(selected, selection_bytes, source_id):
    """Attach original source provenance without producing canonical policy."""
    selected["source"] = {
        "source_id": source_id,
        "source_digest": "sha256:" + hashlib.sha256(selection_bytes).hexdigest(),
    }
    return (json.dumps(selected, ensure_ascii=True, allow_nan=False, separators=(",", ":"))
            + "\n").encode("utf-8")


def _invalid_constant(value):
    raise ValueError(f"Policy selection contains invalid JSON constant: {value}")
