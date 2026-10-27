"""Read application-authored native inventory from exact accepted source.

This carries Connect's existing schema1 inventory, not a tool discovery format.
The implementation agent supplies native class declarations; native still owns
their validation and admission. Reading this module executes no application.
"""
from __future__ import annotations

import math

from .connect_installer_caller import CustodyError, _inventory_input, _json, _same_json_value
from .connect_policy_input import (APPLICATION_INVENTORY_PATH, APPLICATION_NATIVE_POLICY_PATH,
    application_tool_policy_path)


INVENTORY_PATH = APPLICATION_INVENTORY_PATH
INVENTORY_FIELD = "applicationInventory"
POLICY_PATH = APPLICATION_NATIVE_POLICY_PATH


def selects_application_inventory(files):
    """The compiler-owned deployment source selects the new route explicitly.

    Older accepted packages retain their original startup route. Presence of
    an application file alone cannot change that route.
    """
    if "deployment.json" not in files:
        return False
    deployment = _json(files["deployment.json"])
    return application_tool_policy_path(deployment) == POLICY_PATH


def _native_numbers(value):
    # serde_json's default Number stores signed i64, unsigned u64 or finite
    # f64. Reject integers it would silently convert to floating point. Never
    # normalize a callback's schema or rewrite the application source bytes.
    if type(value) is int and not -(1 << 63) <= value < (1 << 64):
        raise CustodyError("Native callback schemas require integers representable by signed i64 or unsigned u64.")
    if type(value) is float and not math.isfinite(value):
        raise CustodyError("Native callback schemas require finite JSON numbers.")
    if isinstance(value, dict):
        for item in value.values():
            _native_numbers(item)
    elif isinstance(value, list):
        for item in value:
            _native_numbers(item)


def application_inventory_source(files):
    """Return original bytes after joining them to the exact compiled roster.

    Callers supply a complete source snapshot already bound by the existing
    workspace/acceptance or package owner. No paths come from application JSON.
    """
    if not selects_application_inventory(files):
        return None
    from .blueprint import Blueprint
    from .compiler import application_inventory_input, application_profiles
    try:
        content = files[INVENTORY_PATH]
        inventory = _inventory_input(content)
        spec = Blueprint.model_validate(_json(files["blueprint.json"]))
    except KeyError as error:
        raise CustodyError("The accepted application is missing its native inventory source. Create application/native-inventory.json from native-inventory.example.json and the native class contract in README.md; declare every zero-instance class from its implementation.") from error
    if "member_classes" not in inventory:
        raise CustodyError("This compiled application requires an explicit native class list.")
    classes = inventory["member_classes"]
    expected = _inventory_input(application_inventory_input(spec, member_classes=classes))
    if not _same_json_value(inventory, expected):
        raise CustodyError("Application-authored classes cannot change the compiled native roster.")
    _native_numbers(inventory)
    profiles = application_profiles(spec)
    selected = set()
    for row in classes:
        class_id = row.get("id")
        if (not isinstance(class_id, str) or class_id in selected or class_id not in profiles
                or row.get("profile") != profiles[class_id]):
            raise CustodyError("Native class declarations must retain their compiled class and reusable profile.")
        selected.add(class_id)
    missing = sorted(row.id for row in spec.agent_classes if row.instances == 0 and row.id not in selected)
    if missing:
        raise CustodyError("Declare native member classes before accepting zero-instance roles: " + ", ".join(missing))
    application_policy_source(files, inventory)
    return content


def application_policy_source(files, inventory):
    """Read explicit fixed-seed grants, independently of later class grants.

    Native class templates apply to admitted class instances. They do not
    confer grants on the compiled seed roster. The accepted application must
    select those grants in the existing application tool-policy input format.
    """
    from .connect_policy_input import application_tool_policy_selection
    from .connect_installer_caller import MAX_INVENTORY_BYTES
    try:
        content = files[POLICY_PATH]
        baseline = files["application/tool-policy.json"]
    except KeyError as error:
        raise CustodyError("The accepted application is missing its explicit native seed policy.") from error
    if len(content) > MAX_INVENTORY_BYTES:
        raise CustodyError("The native seed policy exceeds the preparation artifact bound.")
    identities = tuple(row["identity"] for row in inventory["roster"])
    selected = application_tool_policy_selection(content, selected_member_identities=identities)
    original = application_tool_policy_selection(baseline, selected_member_identities=identities)
    if any(not _same_json_value(selected[name], original[name])
            for name in ("provider_id", "policy_id", "revision")):
        raise CustodyError("The accepted seed policy changed its original provider, policy or revision.")
    return content
