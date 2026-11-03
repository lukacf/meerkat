"""Canonical application scope bindings, separate from transcript provenance."""
from __future__ import annotations

import re

from .projects import fingerprint
from .blueprint import Blueprint


def proposal_hash(value):
    # Compiler input is the validated full blueprint, including defaults.
    # Invalid values remain distinct; they never gain a validated receipt here.
    try:
        value = Blueprint.model_validate(value).model_dump(mode="json")
    except ValueError:
        pass
    return fingerprint(value)


def inputs(state):
    return {"blueprint_hash": proposal_hash(state.get("proposal") or state.get("blueprint")),
            "brief_hash": fingerprint(state.get("brief") or {}),
            **({"organization_hash": fingerprint(state["organization"])}
               if state.get("organization") is not None else {})}


def matches(receipt, state, *, brief_hash=None):
    """Exact plan and full brief equality; a chat revision alone is irrelevant."""
    scope = inputs(state)
    return bool(state.get("proposal") or state.get("blueprint")) and bool(
        receipt.get("guide_id") == state.get("id")
        and receipt.get("blueprint_hash") == scope["blueprint_hash"]
        and ("organization_hash" in receipt) == ("organization_hash" in scope)
        and receipt.get("organization_hash") == scope.get("organization_hash")
        and (brief_hash if brief_hash is not None else receipt.get("brief_hash")) == scope["brief_hash"])


def frozen_matches(receipt, frozen, state):
    """Check canonical inputs while retaining the original frozen provenance."""
    if (receipt.get("guide_id") != frozen.get("guide_id")
            or type(receipt.get("guide_version")) is not int
            or type(frozen.get("guide_version")) is not int
            or receipt.get("guide_version") != frozen["guide_version"]):
        return False
    scope = {"guide_id": frozen["guide_id"],
             "blueprint_hash": proposal_hash(frozen.get("blueprint")),
             "brief_hash": fingerprint(frozen.get("brief") or {})}
    if frozen.get("organization") is not None:
        scope["organization_hash"] = fingerprint(frozen["organization"])
        if receipt.get("organization_hash") != scope["organization_hash"]:
            return False
    elif "organization_hash" in receipt:
        return False
    if not isinstance(frozen.get("brief"), dict) and frozen["guide_version"] != state.get("version"):
        # Missing legacy inputs cannot prove unchanged application scope.
        return False
    return bool(receipt.get("blueprint_hash") == scope["blueprint_hash"]
        and ("brief_hash" not in receipt or receipt["brief_hash"] == scope["brief_hash"])
        and matches(scope, state))


def current_version(receipt, state):
    """Trust a host projection only for this exact current transcript snapshot."""
    version = receipt.get("current_guide_version", receipt.get("guide_version"))
    return bool(receipt.get("current") is True and receipt.get("guide_id") == state.get("id")
                and type(receipt.get("guide_version")) is int and type(version) is int and version == state.get("version"))


def report_brief_hash(report, *, strict=False):
    """Read a digest; admission must distinguish malformed from absent legacy data."""
    acceptance = report.get("acceptance")
    if strict and acceptance is not None and not isinstance(acceptance, dict):
        raise ValueError("The application brief binding is malformed.")
    value = report.get("brief_hash")
    if value is None:
        value = acceptance.get("brief_digest") if isinstance(acceptance, dict) else None
    if value is None or isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value):
        return value
    if strict:
        raise ValueError("The application brief binding is malformed.")
    return None


def package_matches(report, state):
    try:
        brief_hash = report_brief_hash(report, strict=True)
    except ValueError:
        return False
    scope = inputs(state)
    if (("organization_hash" in report) != ("organization_hash" in scope)
            or report.get("organization_hash") != scope.get("organization_hash")):
        return False
    if type(report.get("guide_version")) is not int or report.get("blueprint_hash") != scope["blueprint_hash"]:
        return False
    # Legacy packages without a full brief binding cannot cross transcript
    # revisions. Current native acceptance is independently required by callers.
    return brief_hash == scope["brief_hash"] if brief_hash else (
        type(report.get("guide_version")) is int and report["guide_version"] == state.get("version"))
