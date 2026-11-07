"""Declared local Console authentication rules, without private I/O or runtime work."""
from __future__ import annotations

import json
from pathlib import Path
import re

CONSOLE_FIELDS = (
    {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
    {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True},
)
# `local-password` is one account that everyone who has its password shares.
# `local-accounts` adds named accounts, each one person's, kept as password
# hashes on the deployment computer. Both declare the installation's own
# account through the same two private fields.
CONSOLE_AUTHENTICATIONS = ("local-password", "local-accounts")
CONSOLE_ACCOUNT = re.compile(r"[A-Za-z0-9_.@+-]{1,200}")
# The console's live access file: its Access panel writes here, in private
# state, never to the package's config/access.toml.
CONSOLE_ACCESS_FILE = ".mobkit/console-access.toml"


def console_authentication(value, fields):
    """Admit explicit source metadata. Field names alone never enable login rules."""
    if (not isinstance(value, dict) or set(value) != {"authentication"}
            or value["authentication"] not in CONSOLE_AUTHENTICATIONS):
        raise ValueError("Console authentication supports local-password or local-accounts only.")
    declared = {field["key"]: field for field in fields}
    for expected in CONSOLE_FIELDS:
        field = declared.get(expected["key"], {})
        if (field.get("secret") is not expected["secret"] or field.get("required") is not True
                or field.get("default")):
            raise ValueError(f"Declare required private setup field {expected['key']} without a source default.")
    return value["authentication"]


def console_authentication_choice(root, fields):
    """The declared Console authentication, or None without metadata."""
    path = Path(root) / "application/console.json"
    if not path.exists() and not path.is_symlink():
        return None
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 4096:
        raise ValueError("Console authentication metadata must be a bounded ordinary JSON file.")
    try:
        value = json.loads(path.read_text())
    except (ValueError, UnicodeError):
        raise ValueError("Console authentication metadata must contain valid UTF-8 JSON.") from None
    return console_authentication(value, fields)


def console_configuration(root, fields):
    """Read bounded source metadata only, without reading private values."""
    return console_authentication_choice(root, fields) is not None


def console_operator(access):
    """The installation's own account in a parsed `config/access.toml`.

    It is the first entry of `admins`; further distinct entries are other
    people's accounts, which the console's access rules name. The selection
    is the source's protected operator, not an authenticated person.
    """
    admins = access.get("admins") if isinstance(access, dict) else None
    if (not isinstance(access, dict) or access.get("enabled") is not True or not isinstance(admins, list)
            or not admins or len(set(map(repr, admins))) != len(admins)
            or any(type(admin) is not str or not admin or admin.strip() != admin
                   or any(ord(char) < 32 or ord(char) == 127 for char in admin) for admin in admins)):
        raise ValueError("Console access needs enabled admins: the installation's own account first, then distinct others.")
    return admins[0]


def console_credential_issue(field, value):
    """Safe intrinsic diagnostics, called only after declared Console admission."""
    key = field["key"]
    message = None
    if key == "CONSOLE_USER_ID":
        if not isinstance(value, str) or not CONSOLE_ACCOUNT.fullmatch(value):
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


def console_account_issue(name, installation_account):
    """Why a name cannot be a named Console account, or None."""
    if not isinstance(name, str) or not CONSOLE_ACCOUNT.fullmatch(name):
        return "Use 1-200 ASCII letters, digits, or . _ @ + - for an account name."
    if name == installation_account:
        return ("This is the installation's own account. Change its password with the Console password "
                "setting instead.")
    return None


def console_password_issue(password):
    """Why a named account's password is refused, or None. Never echoes it."""
    issue = console_credential_issue(CONSOLE_FIELDS[1], password)
    return None if issue is None else issue["message"].replace("Console login password", "account password")
