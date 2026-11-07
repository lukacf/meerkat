"""Declared local Console authentication rules, without private I/O or runtime work."""
from __future__ import annotations

import ipaddress
import json
from pathlib import Path
import re
from urllib.parse import urlsplit

CONSOLE_FIELDS = (
    {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
    {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True},
)
# `local-password` is one account that everyone who has its password shares.
# `local-accounts` adds named accounts, each one person's, kept as password
# hashes on the deployment computer. Both declare the installation's own
# account through the same two private fields. `oidc` signs people in at an
# identity provider; its installation's own account is the operator's email.
CONSOLE_AUTHENTICATIONS = ("local-password", "local-accounts", "oidc")
CONSOLE_ACCOUNT = re.compile(r"[A-Za-z0-9_.@+-]{1,200}")
CONSOLE_EMAIL = re.compile(r"[A-Za-z0-9_.+-]{1,64}@[A-Za-z0-9-]{1,63}(?:\.[A-Za-z0-9-]{1,63})+")
# The optional private field that lists further people an OIDC Console admits.
CONSOLE_ALLOWED_EMAILS = "CONSOLE_ALLOWED_EMAILS"
MAX_ALLOWED_EMAILS = 100
OIDC_SOURCE_KEYS = frozenset({"authentication", "issuer", "client_id", "client_secret"})
OIDC_LOOPBACK_HOSTS = ("127.0.0.1", "localhost", "::1")
_OIDC_HOST = re.compile(r"[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*")
# The console's live access file: its Access panel writes here, in private
# state, never to the package's config/access.toml.
CONSOLE_ACCESS_FILE = ".mobkit/console-access.toml"


def oidc_url_issue(value, *, loopback_http=True, query=False):
    """Why an identity provider URL is refused, or None.

    https, or plain http to this computer's loopback for a provider that runs
    here; never credentials or a fragment, and a query only where allowed.
    The address is a host name or an IP address, with a port only as its
    number, written as the URL reaches it: so the URL is usable as written,
    and its origin can be named in the sign-in page's policy.
    """
    exact = "Use the identity provider's exact https URL."
    if (not isinstance(value, str) or not value.isascii() or not 8 <= len(value) <= 512
            or any(ord(char) < 33 or ord(char) == 127 for char in value)):
        return exact
    try:
        parsed = urlsplit(value)
        host, port = parsed.hostname, parsed.port
    except ValueError:
        return exact + " Its port must be a number from 1 to 65535."
    if (not host or parsed.username is not None or parsed.password is not None
            or parsed.query and not query or parsed.fragment
            or parsed.scheme != "https" and not (loopback_http and parsed.scheme == "http" and host in OIDC_LOOPBACK_HOSTS)):
        return "Use the identity provider's exact https URL, without a query, fragment or credentials."
    if ":" in host:
        try:
            named = "%" not in host and ipaddress.IPv6Address(host) is not None
        except ValueError:
            named = False
        address = "[" + host + "]"
    else:
        named, address = _OIDC_HOST.fullmatch(host) is not None, host
    if not named or port == 0 or parsed.netloc.lower() != address + ("" if port is None else f":{port}"):
        return exact + " Its address is a host name or IP address, with a port only as a number from 1 to 65535."
    return None


def console_oidc_source(value, fields):
    """Check OIDC sign-in metadata. The client secret is only ever referenced."""
    if not isinstance(value, dict) or set(value) != OIDC_SOURCE_KEYS or value.get("authentication") != "oidc":
        raise ValueError("OIDC Console sign-in declares exactly authentication, issuer, client_id and client_secret.")
    issue = oidc_url_issue(value["issuer"])
    if issue is not None:
        raise ValueError("OIDC issuer: " + issue)
    client = value["client_id"]
    if not isinstance(client, str) or not 1 <= len(client) <= 512 or any(ord(char) < 33 or ord(char) == 127 for char in client):
        raise ValueError("Declare the OIDC client_id as the identity provider issued it: 1-512 visible characters.")
    declared = {field["key"]: field for field in fields}
    secret = value["client_secret"]
    if isinstance(secret, dict) and set(secret) == {"env"}:
        name = secret["env"]
        field = declared.get(name, {}) if isinstance(name, str) else {}
        if (not isinstance(name, str) or not re.fullmatch(r"[A-Z][A-Z0-9_]{0,127}", name)
                or field.get("secret") is not True or field.get("required") is not True or field.get("default")):
            raise ValueError("Reference the OIDC client secret by a declared required secret private field without a default.")
    elif isinstance(secret, dict) and set(secret) == {"file"}:
        path = secret["file"]
        if (not isinstance(path, str) or not path.startswith("/") or len(path) > 1024 or "\x00" in path
                or any(part in (".", "..") for part in path.split("/"))):
            raise ValueError("Reference the OIDC client secret file by its absolute path.")
    else:
        raise ValueError('Reference the OIDC client secret as {"env": NAME} or {"file": PATH}; '
                         "never put the secret itself in source.")
    operator = declared.get("CONSOLE_USER_ID", {})
    if operator.get("secret") is not False or operator.get("required") is not True or operator.get("default"):
        raise ValueError("Declare required private setup field CONSOLE_USER_ID, the operator's email, without a source default.")
    allowed = declared.get(CONSOLE_ALLOWED_EMAILS)
    if allowed is not None and (allowed.get("secret") is not False or allowed.get("default")):
        raise ValueError(f"Declare {CONSOLE_ALLOWED_EMAILS} as a nonsecret private field without a source default.")
    return value


def console_authentication(value, fields):
    """Admit explicit source metadata. Field names alone never enable login rules."""
    if (not isinstance(value, dict) or value.get("authentication") not in CONSOLE_AUTHENTICATIONS
            or value["authentication"] != "oidc" and set(value) != {"authentication"}):
        raise ValueError("Console authentication supports local-password, local-accounts or oidc only.")
    if value["authentication"] == "oidc":
        console_oidc_source(value, fields)
        return "oidc"
    declared = {field["key"]: field for field in fields}
    for expected in CONSOLE_FIELDS:
        field = declared.get(expected["key"], {})
        if (field.get("secret") is not expected["secret"] or field.get("required") is not True
                or field.get("default")):
            raise ValueError(f"Declare required private setup field {expected['key']} without a source default.")
    return value["authentication"]


def console_authentication_source(root, fields):
    """The declared Console authentication metadata, or None without it."""
    path = Path(root) / "application/console.json"
    if not path.exists() and not path.is_symlink():
        return None
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 4096:
        raise ValueError("Console authentication metadata must be a bounded ordinary JSON file.")
    try:
        value = json.loads(path.read_text())
    except (ValueError, UnicodeError):
        raise ValueError("Console authentication metadata must contain valid UTF-8 JSON.") from None
    console_authentication(value, fields)
    return value


def console_authentication_choice(root, fields):
    """The declared Console authentication, or None without metadata."""
    value = console_authentication_source(root, fields)
    return None if value is None else value["authentication"]


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


ALLOWED_EMAILS_ISSUE = f"List up to {MAX_ALLOWED_EMAILS} email addresses, separated by commas or spaces."


def console_allowed_emails(value):
    """The emails a CONSOLE_ALLOWED_EMAILS value lists, in order and once each."""
    if not isinstance(value, str) or len(value) > 32768:
        raise ValueError(ALLOWED_EMAILS_ISSUE)
    emails = [item for item in re.split(r"[\s,]+", value.strip()) if item]
    if len(emails) > MAX_ALLOWED_EMAILS or any(not CONSOLE_EMAIL.fullmatch(item) for item in emails):
        raise ValueError(ALLOWED_EMAILS_ISSUE)
    return tuple(dict.fromkeys(emails))


def console_credential_issue(field, value, authentication=None):
    """Safe intrinsic diagnostics, called only after declared Console admission."""
    key = field["key"]
    message = None
    if key == "CONSOLE_USER_ID":
        if not isinstance(value, str) or not CONSOLE_ACCOUNT.fullmatch(value):
            message = "Use 1-200 ASCII letters, digits, or . _ @ + - for the Console user identity."
        elif authentication == "oidc" and not CONSOLE_EMAIL.fullmatch(value):
            message = "Use the operator's email address, as the identity provider reports it."
    elif key == "CONSOLE_LOGIN_PASSWORD":
        if not isinstance(value, str) or not 12 <= len(value) <= 4096:
            message = "Use 12-4096 characters for the Console login password."
        else:
            try:
                value.encode("utf-8")
            except UnicodeEncodeError:
                message = "Use valid UTF-8 text for the Console login password."
    elif key == CONSOLE_ALLOWED_EMAILS and authentication == "oidc":
        try:
            console_allowed_emails(value)
        except ValueError:
            message = ALLOWED_EMAILS_ISSUE
    if message is None:
        return None
    return {"key": key, "label": field["label"], "message": message}


def console_credentials_issues(fields, values, authentication=None):
    """Validate exact text without normalizing, echoing or retaining private values."""
    return [issue for field in fields
            if (issue := console_credential_issue(field, values.get(field["key"], ""), authentication)) is not None]


def validate_console_configuration_values(fields, values, authentication=None):
    for issue in console_credentials_issues(fields, values, authentication):
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
