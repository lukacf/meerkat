"""Declared local Console authentication rules, without private I/O or runtime work."""
from __future__ import annotations

import json
from pathlib import Path
import re

CONSOLE_FIELDS = (
    {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
    {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True},
)


def console_authentication(value, fields):
    """Admit explicit source metadata. Field names alone never enable login rules."""
    if value != {"authentication": "local-password"}:
        raise ValueError("Console authentication supports local-password only.")
    declared = {field["key"]: field for field in fields}
    for expected in CONSOLE_FIELDS:
        field = declared.get(expected["key"], {})
        if (field.get("secret") is not expected["secret"] or field.get("required") is not True
                or field.get("default")):
            raise ValueError(f"Declare required private setup field {expected['key']} without a source default.")
    return "local-password"


def console_configuration(root, fields):
    """Read bounded source metadata only, without reading private values."""
    path = Path(root) / "application/console.json"
    if not path.exists() and not path.is_symlink():
        return False
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 4096:
        raise ValueError("Console authentication metadata must be a bounded ordinary JSON file.")
    try:
        value = json.loads(path.read_text())
    except (ValueError, UnicodeError):
        raise ValueError("Console authentication metadata must contain valid UTF-8 JSON.") from None
    console_authentication(value, fields)
    return True


def console_credential_issue(field, value):
    """Safe intrinsic diagnostics, called only after declared Console admission."""
    key = field["key"]
    message = None
    if key == "CONSOLE_USER_ID":
        if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_.@+-]{1,200}", value):
            message = "Use 1-200 ASCII letters, digits, or . _ @ + - for the Console user identity."
    elif key == "CONSOLE_LOGIN_PASSWORD":
        if not isinstance(value, str) or not 12 <= len(value) <= 4096:
            message = "Use 12-4096 characters for the Console login password."
        else:
            try:
                value.encode("utf-8")
            except UnicodeEncodeError:
                message = "Use valid UTF-8 text for the Console login password."
    if message is None:
        return None
    return {"key": key, "label": field["label"], "message": message}


def console_credentials_issues(fields, values):
    """Validate exact text without normalizing, echoing or retaining private values."""
    return [issue for field in fields
            if (issue := console_credential_issue(field, values.get(field["key"], ""))) is not None]


def validate_console_configuration_values(fields, values):
    for issue in console_credentials_issues(fields, values):
        raise ValueError(f"{issue['label']} ({issue['key']}): {issue['message']}")


def validate_console_credentials(principal, password):
    validate_console_configuration_values(CONSOLE_FIELDS, {
        "CONSOLE_USER_ID": principal, "CONSOLE_LOGIN_PASSWORD": password,
    })
