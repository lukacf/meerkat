"""Local login front for the unchanged native Console and admitted human input.

The host authenticates this channel; native admission remains native authority.
Native timeline frames do not attest the authenticated human principal.
"""
from __future__ import annotations

import asyncio
import base64
import copy
from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import Enum
import hashlib
import hmac
import html
import inspect
import json
import math
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import sys
import tempfile
import time
from types import MappingProxyType
from urllib.parse import parse_qsl, quote_plus, urlencode, urlsplit

try:
    from .console_configuration import (CONSOLE_ACCESS_FILE, CONSOLE_ALLOWED_EMAILS, CONSOLE_EMAIL,
        MAX_ALLOWED_EMAILS, console_account_issue, console_allowed_emails, console_authentication_source,
        console_configuration, console_password_issue, oidc_url_issue, validate_console_credentials)
except ImportError:
    from console_configuration import (CONSOLE_ACCESS_FILE, CONSOLE_ALLOWED_EMAILS, CONSOLE_EMAIL,
        MAX_ALLOWED_EMAILS, console_account_issue, console_allowed_emails, console_authentication_source,
        console_configuration, console_password_issue, oidc_url_issue, validate_console_credentials)

ISSUER = "http://127.0.0.1/mobkit-gateway"
AUDIENCE = "persistent-gateway"
HEALTH_AUDIENCE = "mobkit-launch-host-health"
# Every token the front mints for the console lives this long.
TOKEN_SECONDS = 120
SESSION_SECONDS = 8 * 60 * 60
HEALTH_PATHS = {"/console/experience", "/console/identities"}
AVAILABILITY_PATH = "/host/availability"
MAX_BODY = 101 * 1024 * 1024  # The pinned native multipart limit.
# A 4096-character password can use four UTF-8 bytes per character, each
# encoded as three form bytes. Leave bounded space for principal, nonce and keys.
MAX_LOGIN_BODY = 64 * 1024
FRONTEND_PATH = "/console/forge/frontends"
FRONTEND_DESCRIPTOR_PATH = FRONTEND_PATH + "/descriptor"
MAX_FRONTEND_BODY = 256 * 1024
FRONTEND_APPROVAL_SECONDS = 300
CONNECT_PATH = "/console/forge/connect"
CONTROLLER_REVIEW_PATH = "/console/forge/controller-review"
HOSTED_MCP_PATH = "/console/forge/mcp"
MAX_HOSTED_MCP_BODY = 64 * 1024
MAX_CONTROLLER_REVIEW_BODY = 4096
MAX_CONNECT_BODY = 64 * 1024 * 1024
CONSOLE_ASSET_PATH = "/console/forge/assets"
MAX_CONSOLE_ASSET = 16 * 1024 * 1024
TOOLKIT_SETUP_PATHS = frozenset({
    "/host/toolkit/setup", "/host/toolkit/setup/data",
    "/host/toolkit/setup/panel.js", "/host/toolkit/setup/view.js",
    "/host/toolkit/setup/projection.js", "/host/toolkit/setup/style.css",
    "/host/toolkit/setup/callback",
})
TOOLKIT_SETUP_POST_LIMITS = MappingProxyType({
    "/host/toolkit/setup/data": 256,
    "/host/toolkit/setup/start": 1024,
    "/host/toolkit/setup/callback": 64 * 1024,
    "/host/toolkit/setup/answer": 4 * 1024,
    "/host/toolkit/setup/credentials": 64 * 1024,
    "/host/toolkit/setup/cancel-sign-in": 256,
    "/host/toolkit/setup/access/revoke": 1024,
    # Native consent in the browser: the console decides as the signed-in
    # person and refuses a shared account itself.
    "/host/toolkit/consent/read": 4 * 1024,
    "/host/toolkit/consent/decide": 32 * 1024,
})
TOOLKIT_LANDING_CSP = re.compile(
    r"default-src 'none'; script-src 'sha256-[A-Za-z0-9+/]{43}='; connect-src 'self'; "
    r"frame-ancestors 'none'; base-uri 'none'; form-action 'none'")
# Named accounts (`local-accounts`): one private file on the deployment
# computer, a salted scrypt hash per account and never a password.
ACCOUNTS_FILE = "console-accounts.json"
ACCOUNTS_FORMAT = "console-accounts-v1"
MAX_ACCOUNTS = 100
MAX_ACCOUNTS_BYTES = 256 * 1024
# New hashes use scrypt at the OWASP floor (N=2^14, r=8, p=5: 16 MiB each).
# A stored hash names its parameters, which must be one of the floor's
# equivalent choices, so the cost can rise without a new format.
_SCRYPT = MappingProxyType({"n": 2**14, "r": 8, "p": 5})
_SCRYPT_CHOICES = frozenset({(2**14, 8, 5), (2**15, 8, 3), (2**16, 8, 2), (2**17, 8, 1)})
_PASSWORD_HASH = re.compile(r"scrypt:([0-9]{1,7}):([0-9]{1,2}):([0-9]{1,2}):([A-Za-z0-9_-]{22}):([A-Za-z0-9_-]{43})")
# Sign-in limits are per account and per client address, so one person's
# mistakes, or one client's guesses, do not lock everyone else out. Behind a
# shared ingress every client has the ingress's address, so the address
# limit sits above the account limit. The address is the connection's peer,
# never a header the client sets.
FAILURE_WINDOW_SECONDS = 60
FAILURES_PER_ACCOUNT = 10
FAILURES_PER_CLIENT = 30
FAILURE_NAMES = 1024
FAILURE_CLIENTS = 4096
LOGIN_FORMS_PER_CLIENT = 100
LOGIN_FORMS = 1000
SESSIONS_PER_ACCOUNT = 100
SESSIONS = 1000
# Password checks running at once, across every account. A check beyond it
# is refused before any hashing, so guesses cannot queue up memory or CPU.
PASSWORD_CHECKS = 4
# Every name that cannot be an account shares this one limiter key.
_INVALID_ACCOUNT = "\x00"
# OIDC sign-in (`oidc`): the front runs the authorization code flow with
# PKCE, state and nonce at the identity provider, verifies the provider's ID
# token and keeps none of the provider's tokens. It signs the console's
# tokens itself, under an issuer recorded once per installation and an
# ES256 key kept in private state; their subject encodes the provider's
# issuer and subject, so the person stays the same while the front's
# address changes and two providers' people never meet.
OIDC_CALLBACK_PATH = "/login/callback"
OIDC_SCOPES = "openid email"
OIDC_PENDING_SECONDS = 600
OIDC_SKEW_SECONDS = 60
OIDC_JWKS_REFRESH_SECONDS = 30
OIDC_DISCOVERY_SECONDS = 3600
OIDC_ALGORITHMS = frozenset({"RS256", "PS256", "ES256"})
OIDC_MAX_DOCUMENT = 256 * 1024
OIDC_MAX_KEYS = 64
OIDC_MAX_ID_TOKEN = 16 * 1024
OIDC_MAX_CALLBACK_QUERY = 8 * 1024
# Code exchanges at the provider running at once, across every sign-in.
OIDC_EXCHANGES = 8
# One sign-in step's calls to the provider (its discovery document, the code
# exchange and its keys) share this one deadline.
OIDC_SIGN_IN_SECONDS = 20
OIDC_ISSUER_FILE = "console-oidc-issuer"
OIDC_KEY_FILE = "console-oidc-signing.pem"
_FRONT_ISSUER = re.compile(r"urn:forge-console:[a-f0-9]{32}")
# The front's private state is written through private temporary files: the
# live access copy and the OIDC records by an exclusive link, the named
# accounts by a rename. A start or a setup killed before it removed its
# temporary file leaves it behind; a later one removes it (see
# _clear_stale_pending).
STALE_PENDING_SECONDS = 60
_PENDING_FILE = re.compile(
    r"\.(?:console-access|console-oidc-issuer|console-oidc-signing\.pem)\.[a-f0-9]{16}\.pending"
    r"|\.console-accounts\.[a-z0-9_]+\.pending")


def _console_assets(value):
    """Freeze explicitly selected public bytes, with no filesystem lookup."""
    if value is None:
        return MappingProxyType({})
    if not isinstance(value, Mapping):
        raise ValueError("Console assets must be an application-owned mapping.")
    assets = {}
    for path, entry in value.items():
        if (not isinstance(path, str)
                or not re.fullmatch(r"/console/forge/assets/[A-Za-z0-9._/-]+", path)
                or any(part in ("", ".", "..") for part in path[len(CONSOLE_ASSET_PATH) + 1:].split("/"))
                or not isinstance(entry, tuple) or len(entry) != 2):
            raise ValueError("The selected Console asset is invalid.")
        payload, content_type = entry
        if (not isinstance(payload, bytes) or len(payload) > MAX_CONSOLE_ASSET
                or content_type not in ("application/javascript", "text/javascript", "text/css")):
            raise ValueError("The selected Console asset is invalid.")
        assets[path] = (payload, content_type)
    return MappingProxyType(assets)


def _console_public_headers():
    return {"Cache-Control": "no-store", "X-Content-Type-Options": "nosniff",
            "Content-Security-Policy": "frame-ancestors 'none'", "Referrer-Policy": "no-referrer"}


def _method_not_found(value):
    """Whether a console's JSON-RPC answer is "method not found": the console predates the method."""
    error = value.get("error") if isinstance(value, dict) else None
    return isinstance(error, dict) and error.get("code") == -32601


# The console's answer about an administrator when it predates
# `meta/console/whoami`: it names none.
_CONSOLE_PREDATES_WHOAMI = object()


def _console_origin(value, local_origin):
    """Select the browser origin from trusted host construction, never headers.

    This checks the Console's HTTP address, not OAuth provider registration.
    Core separately validates the registered MCP callback base. The listener
    stays on loopback; the deployment owner supplies the actual TLS ingress.
    """
    if value is None:
        return local_origin
    try:
        if (type(value) is not str or not value.isascii()
                or any(ord(char) <= 32 or ord(char) == 127 for char in value)
                or any(char in value for char in ("\\", "%", "?", "#"))):
            raise ValueError
        parsed = urlsplit(value)
        if (parsed.scheme != "https" or not parsed.hostname or parsed.path
                or parsed.username is not None or parsed.password is not None):
            raise ValueError
        port = parsed.port
        if port is not None and port == 0:
            raise ValueError
        host = parsed.hostname
        if ":" in host:
            # urlsplit validates bracketed IPv6 addresses.
            host = "[" + host + "]"
        elif any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label)
                 for label in host.split(".")):
            raise ValueError
        authority = host + (":" + str(port) if port not in (None, 443) else "")
        if parsed.netloc.lower() not in (authority, authority + ":443" if port == 443 else authority):
            raise ValueError
        return "https://" + authority
    except (ValueError, TypeError):
        raise ValueError("The trusted Console origin must be an HTTPS origin without a path.") from None


def _frontend_descriptor_error(status):
    from starlette.responses import JSONResponse
    if status == 401:
        body = {"error": "Console login required."}
    else:
        body = {"detail": {
            409: "The loaded application source changed. No frontend command was admitted.",
            501: "This application has not declared frontend management support.",
            503: "The application frontend owner is unavailable. No activation is claimed.",
        }[status]}
    return JSONResponse(body, status_code=status, headers=_console_public_headers())


@dataclass(frozen=True)
class ConsoleConnectLogin:
    """Trusted login input to optional application-owned Console factories.

    The factory selects its own configured application/realm and returns one
    dormant ConsoleConnect. Its current-binding callback must also check those
    owners. No cookie, browser bootstrap or native selector is supplied here.
    """
    principal: str
    login_scope_id: str
    expires_at_ms: int
    origin: str
    is_current: object
    # The issuer and subject of this login's console tokens, which a native
    # enrollment for the login carries: the front's issuer and the account,
    # or with OIDC sign-in the front's recorded issuer and the provider's
    # person. None in a selection made without them.
    issuer: str | None = None
    subject: str | None = None


@dataclass(frozen=True)
class FrontendManagementRequest:
    """One authenticated host command, never exposed as an agent callback tool."""
    operation: str
    principal: str
    host: dict
    payload: dict
    approved: bool = False


class FrontendSubmissionDisposition(Enum):
    """Trusted owner's knowledge of this configuration command's disposition.

    KNOWN does not assert success, activation, close, domain-job terminality
    or completion of the response transport.
    """
    KNOWN = "known"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class FrontendSubmissionResult:
    """Explicit application result bound to one original frontend submission.

    Observation remains opaque. Only the trusted application owner chooses
    whether it knows this command's disposition; no status is interpreted.
    """
    observation: dict
    host: dict
    change_id: str
    disposition: FrontendSubmissionDisposition


def _frontend_json(value):
    """Canonical, bounded nonsecret protocol bytes for exact integrity binding."""
    def ordinary(item):
        if isinstance(item, dict):
            if any(not isinstance(key, str) for key in item):
                raise ValueError("JSON object keys must be strings.")
            for child in item.values():
                ordinary(child)
        elif isinstance(item, list):
            for child in item:
                ordinary(child)
        elif item is not None and not isinstance(item, (str, int, float, bool)):
            raise ValueError("Ordinary JSON is required.")
    ordinary(value)
    encoded = json.dumps(value, sort_keys=True, ensure_ascii=False,
                         allow_nan=False, separators=(",", ":")).encode("utf-8")
    if len(encoded) > MAX_FRONTEND_BODY:
        raise ValueError("Frontend JSON exceeds its limit.")
    return encoded


def _frontend_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON keys are refused.")
        result[key] = value
    return result


def _private_state_directory(root, *, create=False):
    directory = Path(root) / ".mobkit"
    if create:
        directory.mkdir(mode=0o700, exist_ok=True)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("The Console private state directory is invalid.")
    info = directory.stat()
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise ValueError("The Console state directory must be owned privately by this user.")
    return directory


def _clear_stale_pending(directory):
    """Remove the temporary files killed starts or setups left in private state.

    One that is a second link to the file it published goes at once: its
    writer had only its removal left, which tolerates a missing file. Any
    other goes once older than STALE_PENDING_SECONDS, longer than a write
    takes; a younger one may be another start's or setup's, and is left to it.
    """
    stale = time.time() - STALE_PENDING_SECONDS
    with os.scandir(directory) as entries:
        for entry in entries:
            if not _PENDING_FILE.fullmatch(entry.name):
                continue
            try:
                info = entry.stat(follow_symlinks=False)
                if (stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
                        and (info.st_nlink > 1 or info.st_mtime < stale)):
                    os.unlink(entry.path)
            except FileNotFoundError:
                # Another start or setup removed it first.
                pass


def _private_key(root, *, create=False):
    directory = _private_state_directory(root, create=create)
    path = directory / "console-signing.key"
    if create:
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            pass
        else:
            with os.fdopen(descriptor, "w") as channel:
                channel.write(secrets.token_hex(32))
                channel.flush()
                os.fsync(channel.fileno())
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "r") as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) & 0o077):
            raise ValueError("The Console signing key must be a private owned ordinary file.")
        key = channel.read(65)
    if not re.fullmatch(r"[a-f0-9]{64}", key):
        raise ValueError("The Console signing key is invalid.")
    return key


def _base64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def console_access_file(root):
    """The console's live access file, outside the package; None without one.

    The console's Access panel writes its edits here and never to the
    package's `config/access.toml`, which Forge keeps checking byte for byte.
    The first start copies that file once; later starts and updates leave the
    live copy as the console wrote it. The copy is published only complete:
    written to a private temporary file, synced, then linked into place. An
    empty live copy can only be a seed a crash cut short (the console's own
    saves always write), and it would load as access control off, so it is
    seeded again. A temporary copy a killed start left is removed by a later
    start.
    """
    root = Path(root)
    source = root / "config/access.toml"
    if source.is_symlink() or not source.is_file():
        return None
    directory = root / ".mobkit"
    directory.mkdir(mode=0o700, exist_ok=True)
    info = directory.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or stat.S_IMODE(info.st_mode) & 0o022):
        raise ValueError("The console's state directory must be owned by this user and writable by no one else.")
    _clear_stale_pending(directory)
    live = root / CONSOLE_ACCESS_FILE
    try:
        info = live.lstat()
    except FileNotFoundError:
        info = None
    if info is not None:
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
            raise ValueError("The console's live access file must be an owned ordinary file.")
        if info.st_size:
            return live
        print("WARN: The console's live access file was empty, left by a start that stopped while copying it. "
              "It is copied again from config/access.toml.", file=sys.stderr)
    content = source.read_bytes()
    temporary = directory / (".console-access." + secrets.token_hex(8) + ".pending")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        if info is None:
            try:
                os.link(temporary, live, follow_symlinks=False)
            except FileExistsError:
                # Another start published its complete copy first.
                pass
        else:
            os.replace(temporary, live)
    finally:
        temporary.unlink(missing_ok=True)
    handle = os.open(directory, os.O_RDONLY)
    try:
        os.fsync(handle)
    finally:
        os.close(handle)
    return live


def _stored_password(stored):
    match = _PASSWORD_HASH.fullmatch(stored) if isinstance(stored, str) else None
    if match is None:
        raise ValueError("A named account's password hash is invalid.")
    n, r, p = (int(part) for part in match.groups()[:3])
    if (n, r, p) not in _SCRYPT_CHOICES:
        raise ValueError("A named account's password hash uses scrypt parameters below the floor.")
    salt, digest = (base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)) for part in match.groups()[3:])
    if len(salt) != 16 or len(digest) != 32:
        raise ValueError("A named account's password hash is invalid.")
    return (n, r, p), salt, digest


def _scrypt(password, salt, n, r, p):
    # OpenSSL's scrypt needs about 128 * r * (n + p + 2) bytes; allow twice that.
    return hashlib.scrypt(password, salt=salt, n=n, r=r, p=p, dklen=32, maxmem=256 * r * (n + p + 2))


def hash_console_password(password):
    """A named account's stored password: a salted scrypt hash, never the text."""
    issue = console_password_issue(password)
    if issue is not None:
        raise ValueError(issue)
    salt = secrets.token_bytes(16)
    digest = _scrypt(password.encode("utf-8"), salt, **_SCRYPT)
    return "scrypt:{n}:{r}:{p}:".format(**_SCRYPT) + _base64(salt) + ":" + _base64(digest)


def verify_console_password(stored, password):
    """Whether a password opens the account this stored hash belongs to."""
    (n, r, p), salt, expected = _stored_password(stored)
    try:
        offered, valid = password.encode("utf-8"), True
    except (AttributeError, UnicodeError):
        # Refuse with the same work as a wrong password.
        offered, valid = b"", False
    digest = _scrypt(offered, salt, n, r, p)
    return hmac.compare_digest(digest, expected) and valid


def _console_accounts(value, installation_account):
    """Validated named accounts: name to stored password hash, in order."""
    if not isinstance(value, Mapping) or len(value) > MAX_ACCOUNTS:
        raise ValueError(f"Named Console accounts are at most {MAX_ACCOUNTS} names with password hashes.")
    accounts = {}
    for name, stored in value.items():
        issue = console_account_issue(name, installation_account)
        if issue is not None:
            raise ValueError(issue)
        _stored_password(stored)
        accounts[name] = stored
    return accounts


def _read_accounts_file(root):
    """Exact bytes of the private named-account file, or None while absent."""
    directory = Path(root) / ".mobkit"
    if not directory.exists() and not directory.is_symlink():
        return None
    path = _private_state_directory(root) / ACCOUNTS_FILE
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return None
    with os.fdopen(descriptor, "rb") as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) & 0o077 or info.st_size > MAX_ACCOUNTS_BYTES):
            raise ValueError("The named Console accounts must be a private owned ordinary file.")
        content = channel.read(MAX_ACCOUNTS_BYTES + 1)
    if len(content) > MAX_ACCOUNTS_BYTES:
        raise ValueError("The named Console accounts file exceeds its limit.")
    return content


def _parse_accounts(content, installation_account):
    try:
        value = json.loads(content.decode("utf-8"), object_pairs_hook=_frontend_object)
    except (ValueError, UnicodeError, RecursionError):
        raise ValueError("The named Console accounts file is not valid JSON.") from None
    if (not isinstance(value, dict) or set(value) != {"format", "accounts"}
            or value["format"] != ACCOUNTS_FORMAT or not isinstance(value["accounts"], list)):
        raise ValueError("The named Console accounts file has an unknown format.")
    rows = {}
    for row in value["accounts"]:
        if (not isinstance(row, dict) or set(row) != {"name", "password"}
                or not isinstance(row["name"], str) or row["name"] in rows):
            raise ValueError("The named Console accounts file lists an invalid or repeated account.")
        rows[row["name"]] = row["password"]
    return _console_accounts(rows, installation_account)


def load_console_accounts(root, installation_account):
    """Named accounts kept on the deployment computer: name to password hash."""
    content = _read_accounts_file(root)
    return {} if content is None else _parse_accounts(content, installation_account)


def console_accounts_for_change(root, installation_account):
    """The named accounts, and the exact bytes a later save must still find."""
    content = _read_accounts_file(root)
    return ({} if content is None else _parse_accounts(content, installation_account)), content


def save_console_accounts(root, accounts, original, installation_account):
    """Replace the named accounts, unless someone changed them meanwhile."""
    checked = _console_accounts(accounts, installation_account)
    directory = _private_state_directory(root, create=True)
    _clear_stale_pending(directory)
    changed = ("The named Console accounts changed during setup. Run setup again; "
               "the newer accounts were kept.")
    if _read_accounts_file(root) != original:
        raise ValueError(changed)
    content = (json.dumps({"format": ACCOUNTS_FORMAT, "accounts": [
        {"name": name, "password": stored} for name, stored in checked.items()]}, indent=2) + "\n").encode()
    descriptor, temporary = tempfile.mkstemp(prefix=".console-accounts.", suffix=".pending", dir=directory)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        if _read_accounts_file(root) != original:
            raise ValueError(changed)
        os.replace(temporary, directory / ACCOUNTS_FILE)
        handle = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(handle)
        finally:
            os.close(handle)
    finally:
        Path(temporary).unlink(missing_ok=True)


def _token(key, principal, *, scope="native-console", login_scope_id=None):
    import jwt
    now = int(time.time())
    claims = {"iss": ISSUER, "aud": HEALTH_AUDIENCE if scope == "host-health" else AUDIENCE,
              "sub": principal, "email": principal,
              "email_verified": True, "iat": now, "exp": now + TOKEN_SECONDS,
              "host_scope": scope}
    if login_scope_id is not None:
        claims["sid"] = login_scope_id
    return jwt.encode(claims, key, algorithm="HS256")


@dataclass(frozen=True)
class ConsoleOidc:
    """One identity provider the front signs people in at (`oidc`).

    `client_secret` is the value the application's source references (a
    declared secret private field, or a private file), read at start. It
    never reaches the console's configuration, a page or a log.
    `allowed_emails` are the people admitted besides the operator.
    `transport` replaces the network in tests.
    """
    issuer: str
    client_id: str
    client_secret: str = field(repr=False)
    allowed_emails: tuple = ()
    transport: object = field(default=None, repr=False, compare=False)

    def __post_init__(self):
        issue = oidc_url_issue(self.issuer)
        if issue is not None:
            raise ValueError("OIDC issuer: " + issue)
        if (not isinstance(self.client_id, str) or not 1 <= len(self.client_id) <= 512
                or any(ord(char) < 33 or ord(char) == 127 for char in self.client_id)):
            raise ValueError("The OIDC client_id must be 1-512 visible characters.")
        if (not isinstance(self.client_secret, str) or not 1 <= len(self.client_secret) <= 4096
                or "\x00" in self.client_secret):
            raise ValueError("The OIDC client secret is missing or invalid.")
        if (type(self.allowed_emails) is not tuple or len(self.allowed_emails) > MAX_ALLOWED_EMAILS
                or any(not isinstance(email, str) or not CONSOLE_EMAIL.fullmatch(email)
                       for email in self.allowed_emails)):
            raise ValueError("The people an OIDC Console admits are email addresses.")


def _client_secret_file(path):
    """The OIDC client secret in a private file this user owns."""
    refused = "The OIDC client secret file must be a private ordinary file this user owns."
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        raise ValueError(refused) from None
    with os.fdopen(descriptor, "rb") as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or stat.S_IMODE(info.st_mode) & 0o077 or info.st_size > 4096):
            raise ValueError(refused)
        content = channel.read(4097)
    try:
        secret = content.decode("utf-8").removesuffix("\n").removesuffix("\r")
    except UnicodeError:
        raise ValueError("The OIDC client secret file must hold one secret as text.") from None
    if not secret or any(char in secret for char in "\r\n\x00"):
        raise ValueError("The OIDC client secret file must hold one secret as text.")
    return secret


def console_oidc_settings(root, fields, environ):
    """The OIDC sign-in a source declares, its client secret read by reference."""
    source = console_authentication_source(root, fields)
    if source is None or source["authentication"] != "oidc":
        raise ValueError("The application does not declare OIDC Console sign-in.")
    operator = environ.get("CONSOLE_USER_ID", "")
    if not isinstance(operator, str) or not CONSOLE_EMAIL.fullmatch(operator):
        raise ValueError("Set CONSOLE_USER_ID to the operator's email address.")
    try:
        allowed = console_allowed_emails(environ.get(CONSOLE_ALLOWED_EMAILS, ""))
    except ValueError as error:
        raise ValueError(f"{CONSOLE_ALLOWED_EMAILS}: {error}") from None
    reference = source["client_secret"]
    if "env" in reference:
        secret = environ.get(reference["env"], "")
        if not secret:
            raise ValueError(f"Set the OIDC client secret in private setting {reference['env']}.")
    else:
        secret = _client_secret_file(reference["file"])
    return ConsoleOidc(issuer=source["issuer"], client_id=source["client_id"], client_secret=secret,
                       allowed_emails=allowed)


def oidc_subject(issuer, subject):
    """The console's subject for one person at an identity provider.

    Their issuer and subject as a two-string JSON array: one encoding per
    pair and back, so no two people share a subject, two providers' equal
    subjects included.
    """
    return json.dumps([issuer, subject], separators=(",", ":"), ensure_ascii=True)


def _record_once(directory, name, make):
    """A private record made once, published only complete, then only read.

    `make()` gives its bytes the first time; a record that exists is read
    and never replaced.
    """
    path = directory / name
    if not path.exists() and not path.is_symlink():
        temporary = directory / ("." + name + "." + secrets.token_hex(8) + ".pending")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(descriptor, "wb") as output:
                output.write(make())
                output.flush()
                os.fsync(output.fileno())
            try:
                os.link(temporary, path, follow_symlinks=False)
            except FileExistsError:
                pass
        finally:
            temporary.unlink(missing_ok=True)
        handle = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(handle)
        finally:
            os.close(handle)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "rb") as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) & 0o077 or info.st_size > 64 * 1024):
            raise ValueError(f"The console front's {name} must be a private ordinary file this user owns.")
        return channel.read()


def _new_signing_key():
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    return ec.generate_private_key(ec.SECP256R1()).private_bytes(serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8, serialization.NoEncryption())


def _front_identity(root):
    """The front's own OIDC issuer, ES256 signing key and public JWK.

    Both are made once per installation in private state and never replaced.
    The issuer is a random name, never an address, so the console's people
    keep their keys when the front moves; a malformed issuer record is
    refused.
    """
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    directory = _private_state_directory(root, create=True)
    _clear_stale_pending(directory)
    recorded = _record_once(directory, OIDC_ISSUER_FILE,
        lambda: ("urn:forge-console:" + secrets.token_hex(16) + "\n").encode("ascii"))
    try:
        issuer = recorded.decode("ascii").removesuffix("\n")
    except UnicodeError:
        issuer = ""
    if not _FRONT_ISSUER.fullmatch(issuer) or recorded != (issuer + "\n").encode("ascii"):
        raise ValueError("The console front's recorded OIDC issuer is invalid. Restore it from a backup of "
                         "this installation; a new one would make every person new to the console.")
    try:
        key = serialization.load_pem_private_key(_record_once(directory, OIDC_KEY_FILE, _new_signing_key), None)
    except (ValueError, TypeError):
        raise ValueError("The console front's OIDC signing key is invalid.") from None
    if not isinstance(key, ec.EllipticCurvePrivateKey) or not isinstance(key.curve, ec.SECP256R1):
        raise ValueError("The console front's OIDC signing key must be a P-256 key.")
    numbers = key.public_key().public_numbers()
    public = {"crv": "P-256", "kty": "EC", "x": _base64(numbers.x.to_bytes(32, "big")),
              "y": _base64(numbers.y.to_bytes(32, "big"))}
    # The kid is the key's RFC 7638 thumbprint.
    kid = _base64(hashlib.sha256(json.dumps(public, separators=(",", ":"), sort_keys=True).encode()).digest())
    return issuer, key, {"kty": "EC", "crv": "P-256", "x": public["x"], "y": public["y"], "kid": kid,
                         "alg": "ES256", "use": "sig"}


class _OidcRefused(Exception):
    """A sign-in the front refuses, with the status and words its page shows."""

    def __init__(self, status, message):
        super().__init__(message)
        self.status, self.message = status, message


class _OidcUnavailable(Exception):
    """The identity provider could not be reached or used."""


class _OidcMalformed(Exception):
    """The identity provider answered with content the front cannot read."""


class _OidcTimeout(Exception):
    """The identity provider did not answer within the sign-in step's deadline."""


@dataclass(frozen=True)
class _OidcPending:
    """One started sign-in, kept until its callback or its expiry."""
    binding: bytes
    nonce: str
    verifier: str
    expires: float
    client: str
    earlier: str | None


def _oidc_discovery_checked(document, issuer):
    """The parts of a provider's discovery document this front uses, checked."""
    if not isinstance(document, dict) or document.get("issuer") != issuer:
        raise _OidcUnavailable("The discovery document names another issuer.")
    loopback = urlsplit(issuer).scheme == "http"
    endpoints = {}
    for name in ("authorization_endpoint", "token_endpoint", "jwks_uri"):
        value = document.get(name)
        if oidc_url_issue(value, loopback_http=loopback, query=True) is not None:
            raise _OidcUnavailable(f"The provider's {name} is not a usable https URL.")
        endpoints[name] = value
    responses = document.get("response_types_supported")
    challenges = document.get("code_challenge_methods_supported")
    methods = document.get("token_endpoint_auth_methods_supported", ["client_secret_basic"])
    if (responses is not None and (not isinstance(responses, list) or "code" not in responses)
            or challenges is not None and (not isinstance(challenges, list) or "S256" not in challenges)
            or not isinstance(methods, list)):
        raise _OidcUnavailable("The provider does not offer the code flow with S256 PKCE.")
    if "client_secret_basic" in methods:
        endpoints["client_auth"] = "client_secret_basic"
    elif "client_secret_post" in methods:
        endpoints["client_auth"] = "client_secret_post"
    else:
        raise _OidcUnavailable("The provider accepts no client secret this front can send.")
    return endpoints


def health_headers(root):
    """Host-owned read-only probe credentials. Never load application Python code."""
    try:
        from .application_configuration import application_fields
    except ImportError:
        from application_configuration import application_fields
    root = Path(root)
    if not console_configuration(root, application_fields(root)):
        return {}
    path = root / ".env"
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) & 0o077 or info.st_size > 2 * 1024 * 1024):
            raise ValueError("Console health needs an owned private configuration file.")
        rows = channel.read().splitlines()
    principal = None
    for row in rows:
        name, separator, value = row.partition("=")
        if separator and name.strip() == "CONSOLE_USER_ID":
            value = value.strip()
            principal = json.loads(value) if value.startswith('"') else value.strip("'")
    if not isinstance(principal, str) or not re.fullmatch(r"[A-Za-z0-9_.@+-]{1,200}", principal):
        raise ValueError("Console user identity is not configured.")
    return {"Authorization": "Bearer " + _token(_private_key(root), principal, scope="host-health")}


@dataclass(frozen=True)
class ConsoleInput:
    """Host-authenticated submission plus unmodified native admission evidence."""
    channel: str
    principal: str
    identity: str
    content: object
    interaction_id: str
    session_id: str
    input_frame_id: str
    cursor: str
    conversation_id: str | None
    handling_mode: str | None


@dataclass(eq=False)
class _ConsoleSubscription:
    task: asyncio.Task
    upstream: object = None
    original_close: object = None
    close_task: asyncio.Task | None = None


@dataclass(eq=False)
class _ConsoleLogin:
    session: str
    expires: float
    # The account this login signed in to, and whether that account is one
    # person's (a named account) rather than an account people share.
    principal: str = ""
    person: bool = False
    subscriptions: set = field(default_factory=set)
    expiry_task: asyncio.Task | None = None
    retirement_task: asyncio.Task | None = None
    retired: bool = False
    login_scope_id: str = field(default_factory=lambda: secrets.token_urlsafe(32))
    context: ConsoleConnectLogin | None = None
    connect_created: bool = False
    connect: object = None
    connect_start: asyncio.Task | None = None
    connect_close: asyncio.Task | None = None
    connect_failure: str | None = None
    controller_review_start: asyncio.Task | None = None
    console_login_required: bool = False
    console_login: object = None
    console_login_ready: bool = False
    console_login_failure: str | None = None
    console_login_close: asyncio.Task | None = None
    console_login_revoke_failed: bool = False
    mcp_attempts: dict = field(default_factory=dict)
    mcp_close: asyncio.Task | None = None
    # With OIDC sign-in, the provider's person this login's tokens name;
    # otherwise the account itself (None).
    token_subject: str | None = None

    @property
    def subject(self):
        """The subject of this login's console tokens."""
        return self.principal if self.token_subject is None else self.token_subject


class ConsoleFront:
    """Loopback Console front with an optional trusted public HTTPS origin.

    With public_origin, deployment-owned TLS ingress must enforce HTTPS and
    configure Strict-Transport-Security (HSTS) for that public host. This front
    supplies neither TLS termination nor HSTS policy. Route the selected public
    origin to this listener; forwarded headers never select its browser origin.

    The /host/availability header and 405 contract covers routed methods only.
    Unrouted methods (such as TRACE) receive Starlette's 405 without no-store.
    """

    def __init__(self, root, port, principal, password, on_input=None, *, transport=None, connect_factory=None,
                 console_assets=None, document_transform=None, controller_review_factory=None,
                 console_login_factory=None, runtime_namespace="mobkit", public_origin=None,
                 hosted_mcp_targets=(), accounts=None, oidc=None):
        if type(runtime_namespace) is not str or runtime_namespace not in ("mobkit", "meta"):
            raise ValueError("The application must select the mobkit or meta Console runtime namespace.")
        # The trusted application selects one SDK before serving. Browser RPC
        # methods never select or widen the admitted-input delivery hook.
        self._console_send_method = runtime_namespace + "/console/send"
        self._runtime_namespace = runtime_namespace
        if oidc is not None:
            # People sign in at the identity provider, each as themselves;
            # the installation's own account is the operator's email.
            if type(oidc) is not ConsoleOidc:
                raise ValueError("OIDC sign-in needs its checked settings.")
            if runtime_namespace != "meta":
                raise ValueError("OIDC sign-in needs the Meta Console runtime, whose tokens can name a person.")
            if accounts is not None:
                raise ValueError("OIDC sign-in has no named accounts: the identity provider signs people in.")
            if password is not None:
                raise ValueError("OIDC sign-in has no Console password.")
            if not isinstance(principal, str) or not CONSOLE_EMAIL.fullmatch(principal):
                raise ValueError("OIDC sign-in needs the operator's email address as the Console user.")
        else:
            validate_console_credentials(principal, password)
        self.oidc = oidc
        # None: the installation's own account is the only one, and everyone
        # with its password shares it. A mapping: further named accounts
        # (name to stored password hash), and every account is one person's.
        if accounts is not None and runtime_namespace != "meta":
            raise ValueError("Named Console accounts need the Meta Console runtime, whose tokens can name a person.")
        self.accounts = None if accounts is None else MappingProxyType(_console_accounts(accounts, principal))
        self.console_assets = _console_assets(console_assets)
        if document_transform is not None and not callable(document_transform):
            raise ValueError("The Console document transform must be application-owned and callable.")
        self.document_transform = document_transform
        self.root, self.port, self.principal = Path(root), port, principal
        self.local_origin = f"http://127.0.0.1:{port}"
        self.origin = _console_origin(public_origin, self.local_origin)
        self._browser_host = urlsplit(self.origin).netloc
        self._secure_cookie = public_origin is not None
        if type(hosted_mcp_targets) is not tuple or len(hosted_mcp_targets) > 32:
            raise ValueError("Hosted MCP needs a bounded tuple of trusted configured targets.")
        if hosted_mcp_targets:
            from meerkat_toolkit.mcp_auth import McpTarget
            if (not self._secure_cookie or any(type(target) is not McpTarget for target in hosted_mcp_targets)
                    or len(set(hosted_mcp_targets)) != len(hosted_mcp_targets)):
                raise ValueError("Hosted MCP needs distinct configured targets and the trusted HTTPS origin.")
        # Browser aliases name only these original targets. Request JSON never
        # supplies a target, account, native handle or redirect selection.
        self._mcp_targets = MappingProxyType({secrets.token_urlsafe(24): target for target in hosted_mcp_targets})
        self._mcp_original_targets = {key: copy.deepcopy(target) for key, target in self._mcp_targets.items()}
        self._mcp_routes = {}
        self.key = _private_key(root, create=True)
        # The console's tokens name this issuer: the front's own, recorded
        # once per installation, with OIDC sign-in.
        self._token_issuer = ISSUER
        self._oidc_key = self._oidc_jwk = None
        self._oidc_people = MappingProxyType({})
        if oidc is not None:
            self._token_issuer, self._oidc_key, self._oidc_jwk = _front_identity(self.root)
            people = {}
            # The operator first, then the listed people; an address differing
            # only in case is the same person.
            for email in (principal, *oidc.allowed_emails):
                people.setdefault(email.lower(), email)
            self._oidc_people = MappingProxyType(people)
        self._oidc_pending = {}
        self._oidc_discovery = self._oidc_jwks = self._oidc_jwks_lock = self._oidc_client = None
        self._oidc_exchanges = 0
        # Every account, the installation's own included, costs one scrypt per
        # check, and an unknown name costs one against a decoy, so the answer
        # time names no account. Both hashes are made at the first check.
        self._installation_secret = password
        self._installation_password = self._decoy_password = None
        self._password_checks = 0
        # Console sign-outs of logins a new sign-in replaced, still running.
        self._replaced_sign_outs = set()
        self.cookie = "mobkit_console_" + hashlib.sha256((str(self.root.resolve()) + str(port)).encode()).hexdigest()[:16]
        if self._secure_cookie:
            self.cookie = "__Host-" + self.cookie
        self.sessions, self.delivered = {}, set()
        self._logins, self._closing, self._close_task = {}, False, None
        self.login_tokens, self.failures = {}, {}
        # The client address each live sign-in form was issued to.
        self._login_token_clients = {}
        self.delivery_lock = asyncio.Lock()
        self.on_input, self.transport = on_input, transport
        if connect_factory is not None and not callable(connect_factory):
            raise ValueError("The Console Connect factory must be application-owned and callable.")
        self.connect_factory = connect_factory
        if controller_review_factory is not None and not callable(controller_review_factory):
            raise ValueError("The native review factory must be application-owned and callable.")
        self.controller_review_factory = controller_review_factory
        if console_login_factory is not None and not callable(console_login_factory):
            raise ValueError("The Console login factory must be application-owned and callable.")
        self.console_login_factory = console_login_factory
        # One application-owned review in this host lifetime, started at the
        # first ready login of a person and kept by the front, not by that
        # login: any administrator's login answers it, also after that login
        # ends (with a console that predates whoami, only that login does).
        # A new login never retries an unknown or retired invocation.
        self._controller_review_created = False
        self._controller_review_owner = None
        self._controller_review_failure = None
        self._controller_review_pending = None
        self._controller_review_close = None
        self.frontend_management = self.frontend_host = None
        self.frontend_private_fields = ()
        self.client = self.server = self.task = self.listener = None

    @property
    def native_auth(self):
        if self.oidc is not None:
            # The console trusts this front alone: its recorded issuer and one
            # public key. The provider's person is in each token's `sub`.
            return {"provider": "oidc", "issuer": self._token_issuer, "audience": AUDIENCE,
                    "jwks_json": {"keys": [dict(self._oidc_jwk)]},
                    "email_allowlist": list(self._oidc_people.values()), "accounts": "personal"}
        auth = {"provider": "jwt", "shared_secret": self.key, "issuer": ISSUER,
                "audience": AUDIENCE, "email_allowlist": [self.principal]}
        if self.accounts is not None:
            # Each named account is one person's, so the console names a
            # person per token. Without it the one account is shared and the
            # console treats its tokens as a shared account's.
            auth["email_allowlist"] += list(self.accounts)
            auth["accounts"] = "personal"
        return auth

    def _native_token(self, login=None):
        """The console's bearer for one request: a login's, or the host's own probe.

        With OIDC sign-in the front signs it with its ES256 key, as its own
        issuer: a login's token names the provider's person as `sub`; the
        host's probe names the operator's email and no person.
        """
        if self.oidc is None:
            return _token(self.key, self.principal if login is None else login.principal,
                          login_scope_id=None if login is None else login.login_scope_id)
        import jwt
        now = int(time.time())
        claims = {"iss": self._token_issuer, "aud": AUDIENCE,
                  "email": self.principal if login is None else login.principal, "email_verified": True,
                  "iat": now, "exp": now + TOKEN_SECONDS, "host_scope": "native-console"}
        if login is not None:
            claims.update(sub=login.subject, sid=login.login_scope_id)
        return jwt.encode(claims, self._oidc_key, algorithm="ES256", headers={"kid": self._oidc_jwk["kid"]})

    def application(self, native_url):
        import httpx
        from starlette.applications import Starlette
        from starlette.routing import Route
        parsed = urlsplit(native_url)
        if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port
                or parsed.username or parsed.password or parsed.path not in ("", "/") or parsed.query or parsed.fragment):
            raise ValueError("Console upstream must be this runtime's loopback HTTP endpoint.")
        self.native_url = native_url.rstrip("/")
        self.client = httpx.AsyncClient(transport=self.transport, trust_env=False,
                                        timeout=httpx.Timeout(60, connect=5), follow_redirects=False)
        return Starlette(routes=[Route("/{path:path}", self.handle, methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"])])

    async def start(self, native_url):
        import uvicorn
        app = self.application(native_url)
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            # A stopped front can leave accepted connections in TIME_WAIT.
            # Reuse its loopback address without allowing a second live listener.
            self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self.listener.bind(("127.0.0.1", self.port))
            self.listener.listen(128)
            self.listener.setblocking(False)
            # The listener is loopback only, so a forwarded-for header is
            # whatever the client chose: the client is the connection's peer.
            self.server = uvicorn.Server(uvicorn.Config(app, access_log=False, log_level="error", lifespan="off",
                                                        proxy_headers=False))
            # The generated app owns signals and shutdown, including its native process.
            self.server.capture_signals = _no_signals
            self.task = asyncio.create_task(self.server.serve(sockets=[self.listener]))
            deadline = asyncio.get_running_loop().time() + 10
            while not self.server.started:
                if self.task.done():
                    await self.task
                    raise RuntimeError("The Console login front did not start.")
                if asyncio.get_running_loop().time() >= deadline:
                    raise TimeoutError("The Console login front did not become ready.")
                await asyncio.sleep(0.02)
        except BaseException:
            await self.close()
            raise

    def _retire_login(self, session, reason="login_expired"):
        # Revoke admission before scheduling any asynchronous stream cleanup.
        self.sessions.pop(session, None)
        login = self._logins.get(session)
        if login is None:
            return None
        login.retired = True
        if login.console_login is not None:
            try:
                login.console_login.revoke()
            except Exception:
                login.console_login_revoke_failed = True
        # The controller review is the host lifetime's, not this login's: the
        # administrators' other logins keep it after this one ends (with a
        # console that predates whoami, none does).
        if login.retirement_task is None:
            login.retirement_task = asyncio.create_task(self._finish_login(login, reason))
            # Keep cleanup failures available to logout/close, without logging
            # upstream exception details from an unattended expiry task.
            login.retirement_task.add_done_callback(
                lambda task: task.exception() if not task.cancelled() else None)
        return login.retirement_task

    def _replace_login(self, login):
        """End a login that a new sign-in on its browser replaces.

        The front's login ends at once; the console's sign-out of it runs on,
        and the front's close waits for it.
        """
        self._retire_login(login.session, "logout")
        task = asyncio.create_task(self._console_sign_out(login))
        self._replaced_sign_outs.add(task)
        task.add_done_callback(self._replaced_sign_outs.discard)

    async def _console_sign_out(self, login):
        """End one login in the Meta console (`meta/console/sign_out`).

        The request carries a fresh token of that login (its `sid`), so the
        console refuses every token of the login issued before it. True once
        the console confirms it, or when it predates sign-out (method not
        found: this front's own logout is all there is). A 401 confirms
        nothing: a console that did not verify the token fenced nothing.
        """
        if self._runtime_namespace != "meta" or self.client is None or self._closing:
            return True
        import httpx
        token = self._native_token(login)
        try:
            outgoing = self.client.build_request("POST", self.native_url + "/console/rpc",
                json={"jsonrpc": "2.0", "id": "forge-logout", "method": "meta/console/sign_out", "params": {}},
                headers={"Authorization": "Bearer " + token}, timeout=httpx.Timeout(15, connect=5))
            outgoing.headers.pop("cookie", None)
            response = await self.client.send(outgoing)
            value = response.json() if response.status_code == 200 else None
        except Exception:
            return False
        if not isinstance(value, dict):
            return False
        if "error" in value:
            return _method_not_found(value)
        result = value.get("result")
        return isinstance(result, dict) and result.get("login") == "session"

    async def _finish_login(self, login, reason):
        fences = [asyncio.create_task(login.connect.retire(reason))] if login.connect is not None else []
        if login.console_login is not None:
            fences.append(asyncio.create_task(self._retire_native_login(login, "retire", reason)))
        if login.expiry_task and not login.expiry_task.done():
            login.expiry_task.cancel()
            await asyncio.gather(login.expiry_task, return_exceptions=True)
        subscriptions = list(login.subscriptions)
        for subscription in subscriptions:
            if not subscription.task.done():
                subscription.task.cancel()
        # The child fence and revoked-view cleanup run concurrently. A held
        # retirement ACK must not keep disclosing private prompt/event bytes.
        stopped = await asyncio.gather(*(item.task for item in subscriptions), return_exceptions=True)
        # The response generator also closes its response on normal disconnect.
        # Retained responses cover cancellation before that generator starts.
        closed = await asyncio.gather(*(self._close_subscription(item) for item in subscriptions),
                                      return_exceptions=True)
        fenced = await asyncio.gather(*fences, return_exceptions=True)
        if login.controller_review_start is not None:
            await asyncio.gather(login.controller_review_start, return_exceptions=True)
        if (any(isinstance(result, BaseException) and not isinstance(result, asyncio.CancelledError)
                for result in stopped) or any(isinstance(result, BaseException) for result in closed)):
            raise RuntimeError("Console subscription cleanup is unconfirmed.")
        if login.console_login_revoke_failed or any(isinstance(result, BaseException) for result in fenced):
            raise RuntimeError("Console access retirement is unconfirmed.")
        login.subscriptions.clear()
        if login.connect is None and login.console_login is None:
            self._logins.pop(login.session, None)
        # A retirement ACK fences new work, but does not settle entered work or
        # join the child. Keep this retired login's private custody until close.

    async def _close_subscription(self, subscription):
        if subscription.upstream is None:
            return
        if subscription.close_task is None:
            subscription.close_task = asyncio.create_task(self._close_upstream(subscription.original_close))
            subscription.close_task.add_done_callback(
                lambda task: task.exception() if not task.cancelled() else None)
        # Response.is_closed is set before its underlying stream closes. Join
        # the first close result, including its failure, instead of retrying it.
        await asyncio.shield(subscription.close_task)

    def _own_upstream(self, subscription, upstream):
        subscription.upstream = upstream
        subscription.original_close = upstream.aclose
        # HTTPX aiter_raw also invokes aclose on iterator exhaustion. Bind this
        # response instance to the same first close owner before consuming it.
        upstream.aclose = lambda: self._close_subscription(subscription)

    async def _close_upstream(self, close):
        try:
            await close()
        except BaseException:
            raise RuntimeError("Console upstream cleanup is unconfirmed.") from None

    async def _expire_login(self, login):
        while not login.retired:
            remaining = login.expires - time.time()
            if remaining <= 0:
                self._retire_login(login.session)
                return
            await asyncio.sleep(remaining)

    def _live_login(self, request):
        session = request.cookies.get(self.cookie)
        login = self._logins.get(session)
        if (self._closing or login is None or login.retired
                or self.sessions.get(session) != login.expires):
            return None
        if login.expires <= time.time():
            self._retire_login(session)
            return None
        return login

    def _login_current(self, login):
        if (self._closing or login.retired or self._logins.get(login.session) is not login
                or self.sessions.get(login.session) != login.expires):
            return False
        if login.expires <= time.time():
            self._retire_login(login.session, "login_expired")
            return False
        return True

    def _console_login_binding_current(self, login):
        owner = login.console_login
        try:
            # The enrollment names this login's tokens: their issuer and subject.
            return (owner is not None and owner.issuer == self._token_issuer and owner.subject == login.subject
                    and owner.login_scope_id == login.login_scope_id
                    and type(owner.expires_at_ms) is int and owner.expires_at_ms == int(login.expires * 1000)
                    and owner.is_current() is True)
        except Exception:
            return False

    def _login_context(self, login):
        """Retain one object for private adapters that admit by login identity."""
        if login.context is None:
            principal, origin = login.principal, self.origin
            installation, scope, expiry = self.principal, login.login_scope_id, login.expires
            issuer, subject = self._token_issuer, login.subject
            login.context = ConsoleConnectLogin(principal, scope, int(expiry * 1000), origin,
                lambda: (self._login_current(login) and login.principal == principal
                         and self.principal == installation and self.origin == origin
                         and login.login_scope_id == scope and login.expires == expiry
                         and self._token_issuer == issuer and login.subject == subject),
                issuer=issuer, subject=subject)
        return login.context

    def _reserve_console_login(self, login):
        """Retain the selected dormant owner before validation or eager entry."""
        login.console_login_required = self.console_login_factory is not None
        if not login.console_login_required:
            return
        context = self._login_context(login)
        try:
            owner = self.console_login_factory(context)
            if inspect.isawaitable(owner):
                if inspect.iscoroutine(owner):
                    owner.close()
                raise ValueError("The Console login factory must select synchronously.")
            login.console_login = owner
            for name in ("start", "revoke", "is_current"):
                method = getattr(owner, name, None)
                if not callable(method) or inspect.iscoroutinefunction(method):
                    raise ValueError("Console login admission must be synchronous.")
            for name in ("ready", "retire", "close"):
                if not inspect.iscoroutinefunction(getattr(owner, name, None)):
                    raise ValueError("Console login observation and cleanup must be asynchronous.")
            if not self._console_login_binding_current(login):
                raise ValueError("Invalid Console login binding.")
            owner.start()
        except Exception:
            # Keep original custody even if start entered before it failed.
            # Neither a later route nor a new factory selection retries it.
            login.console_login_failure = "unavailable"

    async def _ready_console_login(self, login):
        if not login.console_login_required:
            return True
        if login.console_login_failure is not None or login.console_login is None:
            return False
        try:
            await login.console_login.ready()
        except Exception:
            login.console_login_failure = "unavailable"
            return False
        # The original owner's retained observation outlives this caller.
        # Its ACK alone does not authenticate a native human or grant access.
        if not self._login_current(login) or not self._console_login_binding_current(login):
            return False
        login.console_login_ready = True
        return True

    def _disclosure_current(self, request, login):
        return (self._live_login(request) is login and login is not None
                and (not login.console_login_required or
                     (login.console_login_ready and login.console_login_failure is None
                      and self._console_login_binding_current(login))))

    async def _console_login_cleanup(self, owner, method, *args):
        # This is the retained owner's cleanup, never the shared client's close.
        result = getattr(owner, method)(*args)
        if not inspect.isawaitable(result):
            raise RuntimeError("Console login cleanup is unconfirmed.")
        return await result

    async def _close_mcp(self, login):
        if login.mcp_close is None:
            async def close():
                results = await asyncio.gather(*(attempt.close() for attempt in login.mcp_attempts.values()),
                                               return_exceptions=True)
                if any(isinstance(result, BaseException) for result in results):
                    raise RuntimeError("MCP sign-in cleanup is unconfirmed.")
            login.mcp_close = asyncio.create_task(close())
        return await asyncio.shield(login.mcp_close)

    async def _retire_native_login(self, login, method, *args):
        # Local admission is already revoked. Join ceremonies while the same
        # enrollment still permits their native cancel/release, then retire it.
        failure = None
        try:
            await self._close_mcp(login)
        except Exception:
            failure = True
        await self._console_login_cleanup(login.console_login, method, *args)
        if failure:
            raise RuntimeError("MCP sign-in cleanup is unconfirmed.")

    def _queue_controller_review(self, login):
        """Start the host lifetime's one review at an administrator's first login.

        An account people share decides nothing, and only an administrator
        answers the review, so the review starts at the first ready login of a
        person the console names an administrator. While one login is checked,
        no other starts one; if it is not an administrator's, the next may.
        A console that predates whoami names no administrator: the review then
        starts at the first ready login of a person, and is that login's.
        """
        if (self.controller_review_factory is None or not login.person
                or self._controller_review_created or self._controller_review_pending is not None):
            return
        self._controller_review_pending = login
        async def start():
            try:
                if login.console_login_required and not await self._ready_console_login(login):
                    return
                held = await self._console_administrator(login)
                if (held is True or held is _CONSOLE_PREDATES_WHOAMI) and self._login_current(login):
                    self._reserve_controller_review(login)
            finally:
                if self._controller_review_pending is login:
                    self._controller_review_pending = None
        login.controller_review_start = asyncio.create_task(start())
        login.controller_review_start.add_done_callback(lambda task: task.exception() if not task.cancelled() else None)

    async def _ensure_controller_review(self, login):
        """Start the review for this login if no start created it yet.

        A check already running for another login is waited for first; when
        it finds no administrator, this login's own check runs.
        """
        for _ in range(2):
            if self._controller_review_created or self._closing:
                return
            pending = self._controller_review_pending
            if pending is None:
                self._queue_controller_review(login)
                task = login.controller_review_start
                if task is not None:
                    await asyncio.gather(asyncio.shield(task), return_exceptions=True)
                return
            task = pending.controller_review_start
            if task is None:
                return
            await asyncio.gather(asyncio.shield(task), return_exceptions=True)

    async def close(self):
        if self._close_task is None:
            self._closing = True
            self.login_tokens.clear()
            self._login_token_clients.clear()
            self._oidc_pending.clear()
            for session in list(self.sessions):
                self._retire_login(session, "application_stopping")
            self._close_task = asyncio.create_task(self._close_resources())
        try:
            await asyncio.shield(self._close_task)
        except asyncio.CancelledError:
            # Caller cancellation does not abandon owned subscription cleanup.
            await asyncio.shield(self._close_task)
            raise

    async def _close_resources(self):
        if self.server:
            self.server.should_exit = True
        try:
            logins = list(self._logins.values())
            for login in logins:
                if login.connect is not None and login.connect_close is None:
                    login.connect_close = asyncio.create_task(login.connect.close())
                if login.console_login is not None and login.console_login_close is None:
                    login.console_login_close = asyncio.create_task(self._retire_native_login(login, "close"))
            review = self._controller_review_owner
            if review is not None and self._controller_review_close is None:
                self._controller_review_close = asyncio.create_task(review.close())
            # Joined close can release an entered startup/presenter while its
            # retirement is pending. Neither is abandoned on a view loss.
            joined = await asyncio.gather(*(task for login in logins for task in
                (login.retirement_task, login.connect_close,
                 login.console_login_close, login.controller_review_start) if task is not None),
                *((self._controller_review_close,) if self._controller_review_close is not None else ()),
                return_exceptions=True)
            # Initialization refusal is not cleanup failure. Its entered work
            # is joined by the child close owner, then this host task is joined.
            await asyncio.gather(*(login.connect_start for login in logins if login.connect_start is not None),
                                 return_exceptions=True)
            try:
                if self.task:
                    await asyncio.wait_for(self.task, 5)
            except asyncio.TimeoutError:
                self.task.cancel()
                await asyncio.gather(self.task, return_exceptions=True)
            except asyncio.CancelledError:
                if self.task:
                    self.task.cancel()
                    await asyncio.gather(self.task, return_exceptions=True)
                raise
            except Exception:
                # A failed server must not prevent application cleanup.
                pass
            if any(isinstance(result, BaseException) for result in joined):
                raise RuntimeError("Console access cleanup is unconfirmed." if review is not None or any(login.connect is not None or login.console_login is not None for login in logins)
                                   else "Console subscription cleanup is unconfirmed.")
            self._logins.clear()
            self._mcp_routes.clear()
        finally:
            if self.listener:
                self.listener.close()
            # A replaced login's console sign-out still uses the client.
            await asyncio.gather(*list(self._replaced_sign_outs), return_exceptions=True)
            if self.client:
                await self.client.aclose()
            if self._oidc_client is not None:
                await self._oidc_client.aclose()
            self.sessions.clear()

    async def _body(self, request, limit):
        body = bytearray()
        async for chunk in request.stream():
            body.extend(chunk)
            if len(body) > limit:
                raise ValueError("Request too large.")
        return bytes(body)

    def _login_response(self, now, client, *, error=None, status_code=200):
        from starlette.responses import HTMLResponse
        # A full table refuses no one: the address's oldest form goes first,
        # then the oldest of all, so one caller behind a shared address
        # cannot hold everyone else's sign-in off.
        issued = [nonce for nonce, address in self._login_token_clients.items()
                  if address == client and nonce in self.login_tokens]
        if len(issued) >= LOGIN_FORMS_PER_CLIENT:
            self._drop_login_form(issued[0])
        if len(self.login_tokens) >= LOGIN_FORMS:
            self._drop_login_form(next(iter(self.login_tokens)))
        nonce = secrets.token_urlsafe(32)
        self.login_tokens[nonce] = now + 300
        self._login_token_clients[nonce] = client
        provider = None if self.oidc is None else urlsplit(self.oidc.issuer).hostname
        response = HTMLResponse(_login_form(nonce, error, named=self.accounts is not None, provider=provider),
                                status_code=status_code)
        response.set_cookie(self.cookie + "_login", nonce, httponly=True, samesite="strict", max_age=300,
                            secure=self._secure_cookie, path="/")
        # The form posts only to the front. An OIDC start moves on to the
        # provider from a page of its own (_oidc_start), never by a redirect
        # of this form's submission, which form-action would also govern.
        response.headers.update({"Cache-Control": "no-store", "Content-Security-Policy":
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'"})
        return response

    def _drop_login_form(self, nonce):
        self.login_tokens.pop(nonce, None)
        self._login_token_clients.pop(nonce, None)

    def _sign_in_held(self, account, client, now):
        """Whether recent failures hold this account or this client address."""
        self.failures = {key: kept for key, times in self.failures.items()
                         if (kept := [at for at in times if at > now - FAILURE_WINDOW_SECONDS])}
        return (len(self.failures.get(("account", account), ())) >= FAILURES_PER_ACCOUNT
                or len(self.failures.get(("client", client), ())) >= FAILURES_PER_CLIENT)

    def _record_failure(self, account, client, now):
        # An OIDC sign-in names no account before it opens: its address alone counts.
        keys = (("client", client),) if account is None else (("account", account), ("client", client))
        for key in keys:
            self.failures.setdefault(key, []).append(now)
        # A name nobody has is counted like an account, and every client
        # address too, in bounded tables: the oldest key is forgotten first.
        unknown = [key for key in self.failures if key[0] == "account" and key[1] != self.principal
                   and (self.accounts is None or key[1] not in self.accounts)]
        if len(unknown) > FAILURE_NAMES:
            del self.failures[min(unknown, key=lambda key: self.failures[key][-1])]
        clients = [key for key in self.failures if key[0] == "client"]
        if len(clients) > FAILURE_CLIENTS:
            del self.failures[min(clients, key=lambda key: self.failures[key][-1])]

    def _forget_failure(self, account, client, now):
        """A sign-in that opened takes back the attempt counted for it."""
        keys = (("client", client),) if account is None else (("account", account), ("client", client))
        for key in keys:
            times = self.failures.get(key)
            if times and now in times:
                times.remove(now)
                if not times:
                    del self.failures[key]

    def _sessions_full(self, account):
        live = [login for login in self._logins.values()
                if not login.retired and self.sessions.get(login.session) == login.expires]
        return (len(self.sessions) >= SESSIONS
                or sum(1 for login in live if login.principal == account) >= SESSIONS_PER_ACCOUNT)

    def _login_account(self, form):
        """The account a sign-in opens. With one shared account, that account."""
        if self.accounts is None:
            return self.principal
        # Names hold no whitespace; an empty name is the installation's own.
        return form.get("account", "").strip() or self.principal

    def _login_refusal(self):
        if self.accounts is None:
            return "That password didn't open this Console. Try the Console password you chose during installation."
        return ("That account and password didn't open this Console. Leave the account empty for this "
                "installation's own account.")

    async def _password_opens(self, account, password):
        """One scrypt check, whichever account the name selects."""
        if account == self.principal:
            if self._installation_password is None:
                stored = await asyncio.to_thread(hash_console_password, self._installation_secret)
                self._installation_password = self._installation_password or stored
            return await asyncio.to_thread(verify_console_password, self._installation_password, password)
        stored = None if self.accounts is None else self.accounts.get(account)
        if stored is None and self._decoy_password is None:
            decoy = await asyncio.to_thread(hash_console_password, secrets.token_urlsafe(24))
            self._decoy_password = self._decoy_password or decoy
        opened = await asyncio.to_thread(verify_console_password, stored or self._decoy_password, password)
        return opened and stored is not None

    def _open_login(self, account, *, person, subject=None):
        """A new login of this account; its session's time starts now."""
        session = secrets.token_urlsafe(32)
        self.sessions[session] = time.time() + SESSION_SECONDS
        login = _ConsoleLogin(session, self.sessions[session], principal=account, person=person,
                              token_subject=subject)
        self._logins[session] = login
        self._reserve_console_login(login)
        self._queue_controller_review(login)
        login.expiry_task = asyncio.create_task(self._expire_login(login))
        return login

    def _oidc_http(self):
        import httpx
        if self._oidc_client is None:
            # The provider is reached directly: no proxy, netrc or redirect.
            self._oidc_client = httpx.AsyncClient(transport=self.oidc.transport, trust_env=False,
                timeout=httpx.Timeout(10, connect=5), follow_redirects=False)
        return self._oidc_client

    async def _oidc_document(self, method, url, **options):
        """One bounded JSON answer from the identity provider: (status, value)."""
        import httpx
        body = bytearray()
        try:
            async with self._oidc_http().stream(method, url, **options) as response:
                async for chunk in response.aiter_bytes():
                    body.extend(chunk)
                    if len(body) > OIDC_MAX_DOCUMENT:
                        raise _OidcUnavailable("The identity provider's answer is too large.")
                status = response.status_code
        except (httpx.HTTPError, httpx.InvalidURL):
            raise _OidcUnavailable("The identity provider could not be reached.") from None
        try:
            value = json.loads(bytes(body).decode("utf-8"), object_pairs_hook=_frontend_object)
        except (ValueError, UnicodeError, RecursionError):
            value = None
        return status, value

    async def _oidc_bounded(self, step):
        """A sign-in step's provider calls, awaited under their one overall deadline."""
        try:
            async with asyncio.timeout(OIDC_SIGN_IN_SECONDS):
                return await step
        except TimeoutError:
            raise _OidcTimeout() from None

    def _oidc_trouble(self, trouble, unavailable):
        """The page for a provider that could not be used: late, unreadable or unavailable."""
        if isinstance(trouble, _OidcTimeout):
            return self._oidc_refusal(504, "The identity provider did not answer in time. Try again shortly; "
                                      "if it keeps failing, tell the Console's operator.")
        if isinstance(trouble, _OidcMalformed):
            return self._oidc_refusal(502, "The identity provider sent an answer this Console could not read. "
                                      "If it keeps happening, tell the Console's operator.")
        return self._oidc_refusal(503, unavailable)

    async def _oidc_discovery_document(self):
        cached = self._oidc_discovery
        if cached is not None and time.time() - cached[1] < OIDC_DISCOVERY_SECONDS:
            return cached[0]
        status, document = await self._oidc_document("GET",
            self.oidc.issuer.rstrip("/") + "/.well-known/openid-configuration", headers={"Accept": "application/json"})
        if status != 200:
            raise _OidcUnavailable("The identity provider's discovery document is unavailable.")
        checked = _oidc_discovery_checked(document, self.oidc.issuer)
        self._oidc_discovery = (checked, time.time())
        return checked

    async def _oidc_fetch_jwks(self):
        discovery = await self._oidc_discovery_document()
        previous = () if self._oidc_jwks is None else self._oidc_jwks[0]
        # The attempt is recorded first, so the refetch bound holds after a failure too.
        self._oidc_jwks = (previous, time.time())
        status, document = await self._oidc_document("GET", discovery["jwks_uri"],
            headers={"Accept": "application/json"})
        keys = document.get("keys") if isinstance(document, dict) else None
        if status != 200 or not isinstance(keys, list) or len(keys) > OIDC_MAX_KEYS:
            raise _OidcUnavailable("The identity provider's keys are unavailable.")
        import jwt
        parsed = []
        for item in keys:
            if (not isinstance(item, dict) or item.get("use", "sig") != "sig"
                    or item.get("kty") not in ("RSA", "EC")):
                continue
            try:
                parsed.append((item.get("kid"), item.get("alg"), item["kty"], item.get("crv"), jwt.PyJWK(item).key))
            except (jwt.PyJWTError, KeyError, TypeError, ValueError):
                continue
        self._oidc_jwks = (tuple(parsed), self._oidc_jwks[1])

    def _oidc_find_key(self, alg, kid):
        """The one published key that fits this token's algorithm and kid, or None."""
        if self._oidc_jwks is None:
            return None
        family = "EC" if alg == "ES256" else "RSA"
        found = [key for key_id, key_alg, kty, curve, key in self._oidc_jwks[0]
                 if kty == family and key_alg in (None, alg) and (family == "RSA" or curve == "P-256")
                 and (kid is None or key_id == kid)]
        return found[0] if len(found) == 1 else None

    async def _oidc_signing_key(self, alg, kid):
        """The provider's key for this token, refetching the keys once for an unknown one.

        A refetch happens at most once per OIDC_JWKS_REFRESH_SECONDS, so
        tokens naming keys nobody published cannot drive the provider.
        """
        key = self._oidc_find_key(alg, kid)
        if key is not None:
            return key
        if self._oidc_jwks_lock is None:
            self._oidc_jwks_lock = asyncio.Lock()
        async with self._oidc_jwks_lock:
            key = self._oidc_find_key(alg, kid)
            if key is not None:
                return key
            if self._oidc_jwks is not None and time.time() - self._oidc_jwks[1] < OIDC_JWKS_REFRESH_SECONDS:
                return None
            await self._oidc_fetch_jwks()
            return self._oidc_find_key(alg, kid)

    async def _oidc_start(self, request, now, client):
        """Send the browser to the identity provider: the code flow, with PKCE, state and nonce."""
        from starlette.responses import HTMLResponse, Response
        if self._sign_in_held(None, client, now):
            return Response("Too many sign-in attempts from this address. Try again in one minute.",
                            status_code=429)
        try:
            discovery = await self._oidc_bounded(self._oidc_discovery_document())
        except (_OidcUnavailable, _OidcMalformed, _OidcTimeout) as trouble:
            return self._oidc_trouble(trouble, "This Console could not use its identity provider just now. Try "
                                      "again shortly; if it keeps failing, tell the Console's operator.")
        if self._closing:
            return Response("The Console is stopping.", status_code=503)
        # A full table refuses no new sign-in: the address's oldest pending
        # sign-in goes first, then the oldest of all, so one caller behind a
        # shared address cannot hold everyone else's off. A flood can still
        # make pending sign-ins expire early; a rate-limiting proxy in front
        # of the Console bounds it.
        self._oidc_pending = {state: pending for state, pending in self._oidc_pending.items()
                              if pending.expires > now}
        started = [state for state, pending in self._oidc_pending.items() if pending.client == client]
        if len(started) >= LOGIN_FORMS_PER_CLIENT:
            del self._oidc_pending[started[0]]
        if len(self._oidc_pending) >= LOGIN_FORMS:
            del self._oidc_pending[next(iter(self._oidc_pending))]
        state, nonce, binding = (secrets.token_urlsafe(32) for _ in range(3))
        verifier = secrets.token_urlsafe(64)
        # A sign-in over this browser's live login ends that login when it opens.
        earlier = self._live_login(request)
        self._oidc_pending[state] = _OidcPending(hashlib.sha256(binding.encode()).digest(), nonce, verifier,
            time.time() + OIDC_PENDING_SECONDS, client, None if earlier is None else earlier.session)
        endpoint = discovery["authorization_endpoint"]
        query = urlencode({"response_type": "code", "client_id": self.oidc.client_id,
            "redirect_uri": self.origin + OIDC_CALLBACK_PATH, "scope": OIDC_SCOPES, "state": state,
            "nonce": nonce, "code_challenge": _base64(hashlib.sha256(verifier.encode("ascii")).digest()),
            "code_challenge_method": "S256"})
        # The form's answer is a same-origin page that moves on to the
        # provider, not a redirect: Chromium checks form-action on every
        # redirect of a form submission, the provider's own included, and no
        # CSP source can name an IPv6 address. A meta refresh is no form
        # submission, so the sign-in page keeps form-action 'self' alone.
        response = HTMLResponse(_oidc_onward_page(endpoint + ("&" if urlsplit(endpoint).query else "?") + query))
        # The provider sends the browser back on a top-level navigation from
        # its own site, which carries Lax cookies only: this one binds the
        # callback to the browser that started the sign-in.
        response.set_cookie(self.cookie + "_oidc", binding, httponly=True, samesite="lax",
                            max_age=OIDC_PENDING_SECONDS, secure=self._secure_cookie, path="/")
        response.delete_cookie(self.cookie + "_login", secure=self._secure_cookie, httponly=True,
                               samesite="strict", path="/")
        response.headers.update(_oidc_page_headers())
        return response

    async def _oidc_callback(self, request, now):
        """The provider's answer: spend its state once, verify its ID token, open a login."""
        from starlette.responses import Response
        if request.method != "GET":
            return Response("Sign-in answers use GET.", status_code=405,
                            headers={**_console_public_headers(), "Allow": "GET"})
        raw = request.scope["query_string"]
        if len(raw) > OIDC_MAX_CALLBACK_QUERY:
            return self._oidc_refusal(413, "This sign-in answer is too large.")
        try:
            pairs = parse_qsl(raw.decode("ascii"), keep_blank_values=True, strict_parsing=True, max_num_fields=8)
        except (ValueError, UnicodeError):
            return self._oidc_refusal(400, "This sign-in answer is not one this Console can read.")
        params = dict(pairs)
        if len(params) != len(pairs):
            return self._oidc_refusal(400, "This sign-in answer is not one this Console can read.")
        if self._oidc_exchanges >= OIDC_EXCHANGES:
            return Response("Try again shortly.", status_code=429, headers=_console_public_headers())
        state = params.get("state", "")
        pending = self._oidc_pending.pop(state, None) if state else None
        if pending is None or pending.expires <= now:
            return self._oidc_refusal(400, "This sign-in expired or was already used.")
        binding = request.cookies.get(self.cookie + "_oidc", "")
        if not hmac.compare_digest(hashlib.sha256(binding.encode()).digest(), pending.binding):
            return self._oidc_refusal(400, "This sign-in was started in another browser or window.")
        if "error" in params:
            code = params["error"]
            named = f" ({code})" if re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", code) else ""
            return self._oidc_refusal(401, f"The identity provider did not sign you in{named}.")
        code = params.get("code", "")
        if not code or len(code) > 4096:
            return self._oidc_refusal(400, "This sign-in answer carries no code.")
        client = request.client.host if request.client is not None else ""
        # The attempt counts before the exchange; a sign-in that opens takes it back.
        self._record_failure(None, client, now)
        self._oidc_exchanges += 1
        try:
            principal, subject = await self._oidc_bounded(self._oidc_person(code, pending))
        except _OidcRefused as refused:
            return self._oidc_refusal(refused.status, refused.message)
        except (_OidcUnavailable, _OidcMalformed, _OidcTimeout) as trouble:
            return self._oidc_trouble(trouble, "The identity provider could not confirm this sign-in just now.")
        finally:
            self._oidc_exchanges -= 1
        self._forget_failure(None, client, now)
        if self._closing:
            return Response("The Console is stopping.", status_code=503)
        if self._sessions_full(principal):
            return self._oidc_refusal(429, "Too many active sessions. Sign out of another one, or restart the "
                                      "application to clear them.")
        earlier = None if pending.earlier is None else self._logins.get(pending.earlier)
        if earlier is not None and self._login_current(earlier):
            self._replace_login(earlier)
        return self._oidc_landing(self._open_login(principal, person=True, subject=subject))

    async def _oidc_person(self, code, pending):
        """Exchange the code with its PKCE verifier; the admitted person and their console subject."""
        discovery = await self._oidc_discovery_document()
        form = {"grant_type": "authorization_code", "code": code,
                "redirect_uri": self.origin + OIDC_CALLBACK_PATH, "code_verifier": pending.verifier}
        headers = {"Accept": "application/json"}
        if discovery["client_auth"] == "client_secret_basic":
            pair = quote_plus(self.oidc.client_id, safe="") + ":" + quote_plus(self.oidc.client_secret, safe="")
            headers["Authorization"] = "Basic " + base64.b64encode(pair.encode("utf-8")).decode("ascii")
        else:
            form.update(client_id=self.oidc.client_id, client_secret=self.oidc.client_secret)
        status, answer = await self._oidc_document("POST", discovery["token_endpoint"], data=form, headers=headers)
        id_token = answer.get("id_token") if isinstance(answer, dict) else None
        if status != 200 or not isinstance(id_token, str) or not 1 <= len(id_token) <= OIDC_MAX_ID_TOKEN:
            raise _OidcRefused(401, "The identity provider did not confirm this sign-in.")
        # Only the ID token is read; the provider's other tokens are dropped here.
        try:
            claims = await self._oidc_verified_claims(id_token, pending.nonce)
            return self._oidc_admitted(claims)
        except (TypeError, ValueError, UnicodeError, OverflowError, RecursionError):
            # Signed content none of the checks can read, such as a claim
            # that is not text: the provider's answer is malformed.
            raise _OidcMalformed() from None

    async def _oidc_verified_claims(self, id_token, nonce):
        import jwt
        refused = _OidcRefused(401, "The identity provider's answer did not verify.")
        try:
            header = jwt.get_unverified_header(id_token)
        except jwt.PyJWTError:
            raise refused from None
        alg, kid = header.get("alg"), header.get("kid")
        if not isinstance(alg, str):
            raise _OidcMalformed()
        if alg not in OIDC_ALGORITHMS or kid is not None and (not isinstance(kid, str) or len(kid) > 256):
            raise refused
        key = await self._oidc_signing_key(alg, kid)
        if key is None:
            raise refused
        try:
            claims = jwt.decode(id_token, key, algorithms=[alg], audience=self.oidc.client_id,
                issuer=self.oidc.issuer, options={"verify_exp": False, "verify_iat": False, "verify_nbf": False,
                "require": ["iss", "aud", "sub", "exp", "iat", "nonce"]})
        except jwt.PyJWTError:
            raise refused from None
        now = time.time()
        def moment(value):
            # A time is a finite number; Python's json reads Infinity and NaN too.
            return type(value) is int or type(value) is float and math.isfinite(value)
        if (not moment(claims["exp"]) or not moment(claims["iat"]) or claims["exp"] + OIDC_SKEW_SECONDS <= now
                or claims["iat"] > now + OIDC_SKEW_SECONDS
                or "nbf" in claims and (not moment(claims["nbf"]) or claims["nbf"] > now + OIDC_SKEW_SECONDS)):
            raise refused
        # Several audiences, or a named authorized party, must name this client.
        if (isinstance(claims["aud"], list) and len(claims["aud"]) > 1 or "azp" in claims) \
                and claims.get("azp") != self.oidc.client_id:
            raise refused
        if not isinstance(claims["nonce"], str) or not hmac.compare_digest(claims["nonce"].encode(), nonce.encode()):
            raise refused
        subject = claims["sub"]
        if not isinstance(subject, str) or not 1 <= len(subject) <= 255 or not subject.isascii() or not subject.isprintable():
            raise refused
        return claims

    def _oidc_admitted(self, claims):
        """The admitted person: a verified email on the list. Their name and console subject."""
        email, verified = claims.get("email"), claims.get("email_verified")
        # Without a usable email, the front's own page names the person by
        # the provider's subject; the console never sees them.
        named = email if isinstance(email, str) and email and len(email) <= 320 and email.isprintable() else claims["sub"]
        if not isinstance(email, str) or not email or verified is not True:
            raise _OidcRefused(403, f"Your identity provider did not confirm a verified email address for {named}. "
                               "This Console signs people in by their verified email address.")
        principal = self._oidc_people.get(email.lower()) if email.isascii() else None
        if principal is None:
            raise _OidcRefused(403, f"{named} may not sign in to this Console. Ask its operator to add you.")
        return principal, oidc_subject(self.oidc.issuer, claims["sub"])

    def _oidc_landing(self, login):
        """The signed-in page: a same-site step on to the console, which carries the Strict cookie."""
        from starlette.responses import HTMLResponse
        response = HTMLResponse(_oidc_landing_page())
        response.set_cookie(self.cookie, login.session, httponly=True, samesite="strict", max_age=SESSION_SECONDS,
                            secure=self._secure_cookie, path="/")
        response.delete_cookie(self.cookie + "_oidc", secure=self._secure_cookie, httponly=True, samesite="lax",
                               path="/")
        response.headers.update(_oidc_page_headers())
        return response

    def _oidc_refusal(self, status, message):
        from starlette.responses import HTMLResponse
        response = HTMLResponse(_oidc_refusal_page(message), status_code=status)
        response.delete_cookie(self.cookie + "_oidc", secure=self._secure_cookie, httponly=True, samesite="lax",
                               path="/")
        response.headers.update(_oidc_page_headers())
        return response

    async def handle(self, request):
        from starlette.responses import JSONResponse, RedirectResponse, Response, StreamingResponse
        path = request.url.path
        mcp_callback = self._mcp_routes.get(path)
        mcp_landing = mcp_callback is not None and request.method in ("GET", "HEAD")
        if mcp_callback is not None and request.scope.get("raw_path") != path.encode("ascii"):
            return Response(status_code=400, headers=_console_public_headers())
        availability = path == AVAILABILITY_PATH
        availability_headers = (_console_public_headers() if availability or mcp_callback is not None
                                or path == HOSTED_MCP_PATH or path.startswith(HOSTED_MCP_PATH + "/") else None)
        toolkit_callback_landing = path == "/host/toolkit/setup/callback" and request.method == "GET"
        # Internal health probes keep their existing loopback URL and separate
        # read-only bearer credential. A browser cookie never admits this alias.
        local_health = (self._secure_cookie and request.method == "GET" and path in HEALTH_PATHS
                        and request.headers.get("host") == urlsplit(self.local_origin).netloc)
        if (request.headers.get("host") != self._browser_host and not local_health
                or not (toolkit_callback_landing or mcp_landing) and any(key == "auth_token" for key, _ in parse_qsl(request.url.query))):
            return Response("Invalid Console address.", status_code=400, headers=availability_headers)
        # Routing reads the path as sent and the upstream client normalizes it,
        # so a path with dot or empty segments is refused before either.
        segments = path.split("/")[1:]
        if any(part in (".", "..") for part in segments) or "" in segments[:-1]:
            return Response("Invalid Console address.", status_code=400, headers=availability_headers)
        if request.headers.get("origin") not in (None, self.origin) and not (toolkit_callback_landing or mcp_landing):
            return Response("Cross-origin Console requests are refused.", status_code=403, headers=availability_headers)
        if mcp_landing:
            # The SDK fixed landing is independent of browser/native login.
            # Never parse, log or forward the provider query on this GET.
            if len(request.scope["query_string"]) > MAX_HOSTED_MCP_BODY:
                return Response(status_code=413, headers=_console_public_headers())
            redirect, binding = mcp_callback
            response = await binding.handle_callback(method="GET", redirect_uri=redirect)
            return Response(b"" if request.method == "HEAD" else response.body,
                            status_code=response.status, headers=dict(response.headers))
        if availability:
            # Deployment relays may probe this front without allocating a login
            # nonce, inspecting a session, driving retirement or calling native.
            # Empty 204 means only that this listener accepts requests; native
            # application readiness still requires authenticated HEALTH_PATHS.
            if request.method not in ("GET", "HEAD"):
                return Response(status_code=405, headers={**availability_headers, "Allow": "GET, HEAD"})
            if request.scope["query_string"]:
                return Response(status_code=400, headers=availability_headers)
            return Response(status_code=503 if self._closing else 204, headers=availability_headers)
        if self._closing:
            if path == FRONTEND_DESCRIPTOR_PATH:
                return _frontend_descriptor_error(503)
            return Response("The Console is stopping.", status_code=503)
        if toolkit_callback_landing and len(request.scope["query_string"]) > 16 * 1024:
            return Response("Invalid Toolkit callback request.", status_code=413,
                            headers=_console_public_headers())
        now = time.time()
        if not toolkit_callback_landing:
            for session, expiry in list(self.sessions.items()):
                if expiry <= now:
                    self._retire_login(session)
        if path == "/login":
            self.login_tokens = {key: expiry for key, expiry in self.login_tokens.items() if expiry > now}
            self._login_token_clients = {key: address for key, address in self._login_token_clients.items()
                                         if key in self.login_tokens}
            client = request.client.host if request.client is not None else ""
            if request.method == "GET":
                return self._login_response(now, client)
            if request.method != "POST" or request.headers.get("origin") != self.origin:
                return Response("Login requires a same-origin form.", status_code=403)
            try:
                body = await self._body(request, MAX_LOGIN_BODY)
                form = dict(parse_qsl(body.decode(), max_num_fields=4))
            except (ValueError, UnicodeError):
                return Response("Invalid login form.", status_code=400)
            if self._closing:
                return Response("The Console is stopping.", status_code=503)
            nonce = form.get("nonce", "")
            if (not nonce or nonce != request.cookies.get(self.cookie + "_login")
                    or not self.login_tokens.pop(nonce, None)):
                return self._login_response(now, client, error=("This sign-in form expired. Sign in again."
                    if self.oidc is not None else "This sign-in form expired. Enter your Console password again."),
                    status_code=403)
            self._login_token_clients.pop(nonce, None)
            if self.oidc is not None:
                return await self._oidc_start(request, now, client)
            account = self._login_account(form)
            # A name that cannot be an account costs no password hash and
            # shares one limit with every other such name.
            possible = account == self.principal or console_account_issue(account, None) is None
            key = account if possible else _INVALID_ACCOUNT
            if self._sign_in_held(key, client, now):
                return Response("Too many login attempts. Try again in one minute.", status_code=429)
            if self._password_checks >= PASSWORD_CHECKS:
                return Response("Try again shortly.", status_code=429)
            # The attempt counts before the password work, so attempts made at
            # once cannot pass the hold together; a password that opens takes
            # it back.
            self._record_failure(key, client, now)
            self._password_checks += 1
            try:
                opened = possible and await self._password_opens(account, form.get("password", ""))
            finally:
                self._password_checks -= 1
            if not opened:
                return self._login_response(now, client, error=self._login_refusal(), status_code=401)
            self._forget_failure(key, client, now)
            if self._closing:
                return Response("The Console is stopping.", status_code=503)
            if self._sessions_full(account):
                return Response("Too many active sessions. Sign out of another one, or restart the application "
                                "to clear them.", status_code=429)
            # A sign-in over this browser's live login ends that login, here
            # and in the console, rather than leaving it running unseen.
            earlier = self._live_login(request)
            if earlier is not None:
                self._replace_login(earlier)
            # The session's time starts now, after the password work.
            login = self._open_login(account, person=self.accounts is not None)
            session = login.session
            response = RedirectResponse("/console", status_code=303)
            response.set_cookie(self.cookie, session, httponly=True, samesite="strict", max_age=SESSION_SECONDS,
                                secure=self._secure_cookie, path="/")
            response.delete_cookie(self.cookie + "_login", secure=self._secure_cookie, httponly=True,
                                   samesite="strict", path="/")
            response.headers["Cache-Control"] = "no-store"
            return response
        if self.oidc is not None and path == OIDC_CALLBACK_PATH:
            return await self._oidc_callback(request, now)
        # Provider navigation can omit the Strict cookie. This fixed landing
        # GET never borrows a viewer; only its later same-origin POST may do so.
        login = None if toolkit_callback_landing or local_health else self._live_login(request)
        is_human = login is not None
        health = False
        if not is_human and not toolkit_callback_landing:
            import jwt
            authorization = request.headers.get("authorization", "")
            if authorization.startswith("Bearer ") and request.method == "GET" and path in HEALTH_PATHS:
                try:
                    claims = jwt.decode(authorization[7:], self.key, algorithms=["HS256"], issuer=ISSUER, audience=HEALTH_AUDIENCE,
                                        options={"require": ["exp", "sub", "host_scope"]})
                    health = claims.get("sub") == self.principal and claims.get("host_scope") == "host-health"
                except jwt.PyJWTError:
                    pass
            if not health:
                if request.method == "GET" and path in ("/", "/console", "/console/"):
                    return RedirectResponse("/login", status_code=303)
                return JSONResponse({"error": "Console login required."}, status_code=401,
                                    headers=availability_headers)
        if request.method not in ("GET", "HEAD") and request.headers.get("origin") != self.origin:
            return Response("Console changes require a same-origin authenticated browser.", status_code=403,
                            headers=availability_headers)
        if path == "/logout" and request.method == "POST":
            # The front's own login ends first and in every case: its session
            # is revoked here and its cookie cleared in every answer below.
            retirement = self._retire_login(login.session, "logout")
            # The console then ends it too, so a token this front already
            # issued for the login stops working there.
            signed_out = await self._console_sign_out(login)
            problems = []
            if retirement is not None:
                try:
                    await asyncio.shield(retirement)
                except Exception:
                    problems.append("Console access was revoked but cleanup is unconfirmed.")
            if not signed_out:
                # A token lives two minutes and the console allows it half a
                # minute more, so say three.
                problems.append("You are signed out here, but the Console did not confirm ending this login. "
                                "Tokens it already holds for this login expire within three minutes.")
            response = (RedirectResponse("/login", status_code=303) if not problems
                        else _signed_out_page(" ".join(problems)))
            response.delete_cookie(self.cookie, secure=self._secure_cookie, httponly=True,
                                   samesite="strict", path="/")
            return response
        if path == "/":
            return RedirectResponse("/console", status_code=303)
        if is_human:
            ready = await self._ready_console_login(login)
            if path == FRONTEND_DESCRIPTOR_PATH and self._closing:
                return _frontend_descriptor_error(503)
            if self._live_login(request) is not login:
                return JSONResponse({"error": "Console login required."}, status_code=401,
                                    headers=_console_public_headers())
            if not ready or not self._disclosure_current(request, login):
                if path == FRONTEND_DESCRIPTOR_PATH:
                    if callable(self.frontend_management) and callable(self.frontend_host):
                        try:
                            self.frontend_host()
                        except Exception:
                            return _frontend_descriptor_error(409)
                    return _frontend_descriptor_error(503)
                return Response("Console login enrollment is unavailable.", status_code=503,
                                headers=_console_public_headers())
        if path == FRONTEND_DESCRIPTOR_PATH:
            return await self._frontend_descriptor(request, login)
        if mcp_callback is not None:
            return await self._mcp_callback(request, login, mcp_callback[0])
        if path == HOSTED_MCP_PATH or path.startswith(HOSTED_MCP_PATH + "/"):
            return await self._mcp_request(request, login)
        if path == FRONTEND_PATH or path.startswith(FRONTEND_PATH + "/"):
            return await self._frontend_request(request, login)
        if path == CONTROLLER_REVIEW_PATH or path.startswith(CONTROLLER_REVIEW_PATH + "/"):
            return await self._controller_review_request(request, login)
        if path == CONNECT_PATH or path.startswith(CONNECT_PATH + "/"):
            return await self._connect_request(request, login)
        if path == CONSOLE_ASSET_PATH or path.startswith(CONSOLE_ASSET_PATH + "/"):
            if request.url.query:
                return Response("Invalid Console asset request.", status_code=400,
                                headers=_console_public_headers())
            asset = self.console_assets.get(path)
            if asset is None:
                return Response("Unknown Console asset.", status_code=404,
                                headers=_console_public_headers())
            if request.method != "GET":
                return Response("Console assets require GET.", status_code=405,
                                headers={**_console_public_headers(), "Allow": "GET"})
            if not self._disclosure_current(request, login):
                return JSONResponse({"error": "Console login required."}, status_code=401)
            return Response(asset[0], media_type=asset[1], headers=_console_public_headers())
        toolkit_setup = path in TOOLKIT_SETUP_PATHS
        toolkit_data_get = path == "/host/toolkit/setup/data" and request.method == "GET"
        toolkit_post_limit = None if toolkit_callback_landing or toolkit_data_get else TOOLKIT_SETUP_POST_LIMITS.get(path)
        if toolkit_post_limit is not None:
            if request.method != "POST":
                return Response("Toolkit setup changes require POST.", status_code=405,
                                headers={**_console_public_headers(), "Allow": "GET, POST" if path in ("/host/toolkit/setup/callback", "/host/toolkit/setup/data") else "POST"})
            if (request.url.query or request.headers.get("content-type", "").split(";", 1)[0].strip() != "application/json"):
                return Response("Invalid Toolkit setup request.", status_code=400,
                                headers=_console_public_headers())
        elif toolkit_setup:
            if request.method != "GET":
                return Response("Toolkit setup requires GET.", status_code=405,
                                headers={**_console_public_headers(), "Allow": "GET"})
            if request.url.query and not toolkit_callback_landing:
                return Response("Invalid Toolkit setup request.", status_code=400,
                                headers=_console_public_headers())
        elif path != "/" and not path.startswith(("/console", "/blobs/")) and path != "/favicon.ico":
            return Response("Unknown Console route.", status_code=404)
        try:
            body = b"" if toolkit_callback_landing else await self._body(request, toolkit_post_limit if toolkit_post_limit is not None else MAX_BODY)
            if toolkit_post_limit is not None:
                try:
                    payload = json.loads(body.decode("utf-8"), object_pairs_hook=_frontend_object)
                except RecursionError:
                    raise ValueError("Invalid Toolkit setup request.") from None
                if path == "/host/toolkit/setup/callback":
                    if (not isinstance(payload, dict) or set(payload) != {"query"}
                            or not isinstance(payload["query"], str) or len(payload["query"].encode("utf-8")) > 16 * 1024):
                        raise ValueError("Invalid Toolkit callback request.")
                elif path == "/host/toolkit/setup/start":
                    if (not isinstance(payload, dict) or set(payload) != {"review_id"}
                            or type(payload["review_id"]) is not int or abs(payload["review_id"]) > 2**53 - 1):
                        raise ValueError("Invalid Toolkit start request.")
                elif path == "/host/toolkit/setup/data":
                    if (not isinstance(payload, dict) or set(payload) != {"page_id", "direction"}
                            or type(payload["page_id"]) is not int or not 1 <= payload["page_id"] <= 2**53 - 1
                            or payload["direction"] not in ("refresh", "next")):
                        raise ValueError("Invalid Toolkit status request.")
                elif path == "/host/toolkit/setup/cancel-sign-in":
                    if (not isinstance(payload, dict) or set(payload) != {"review_id"}
                            or type(payload["review_id"]) is not int or not 1 <= payload["review_id"] <= 2**53 - 1):
                        raise ValueError("Invalid Toolkit cancellation request.")
                elif path == "/host/toolkit/setup/access/revoke":
                    if (not isinstance(payload, dict) or set(payload) != {"offer_id", "row_id"}
                            or type(payload["offer_id"]) is not int or not 1 <= payload["offer_id"] <= 2**53 - 1
                            or not isinstance(payload["row_id"], str) or not re.fullmatch(r"[a-f0-9]{64}", payload["row_id"])):
                        raise ValueError("Invalid Toolkit access request.")
                elif path == "/host/toolkit/setup/credentials":
                    if (not isinstance(payload, dict)
                            or set(payload) != {"review_id", "approval_id", "decision", "credentials"}
                            or type(payload["review_id"]) is not int or abs(payload["review_id"]) > 2**53 - 1
                            or not isinstance(payload["approval_id"], str) or len(payload["approval_id"].encode("utf-8")) > 512
                            or payload["decision"] not in ("approve", "deny")
                            or not isinstance(payload["credentials"], list)):
                        raise ValueError("Invalid Toolkit credentials request.")
                    # This is only the private transport envelope. The native
                    # review owner validates declared slots and the decision.
                    if any(not isinstance(item, dict) or set(item) != {"slot", "value"}
                           or not isinstance(item["slot"], str) or not isinstance(item["value"], str)
                           for item in payload["credentials"]):
                        raise ValueError("Invalid Toolkit credentials request.")
                elif path == "/host/toolkit/consent/read":
                    if (not isinstance(payload, dict) or set(payload) != {"approval_id"}
                            or not isinstance(payload["approval_id"], str)
                            or len(payload["approval_id"].encode("utf-8")) > 512):
                        raise ValueError("Invalid consent request.")
                elif path == "/host/toolkit/consent/decide":
                    if (not isinstance(payload, dict)
                            or set(payload) not in ({"approval_id", "decision"}, {"approval_id", "decision", "reason"})
                            or not isinstance(payload["approval_id"], str)
                            or len(payload["approval_id"].encode("utf-8")) > 512
                            or payload["decision"] not in ("approve", "deny")
                            or "reason" in payload and (not isinstance(payload["reason"], str)
                                                        or len(payload["reason"].encode("utf-8")) > 4096)):
                        raise ValueError("Invalid consent decision.")
                elif (not isinstance(payload, dict) or set(payload) != {"review_id", "approval_id", "decision"}
                        or type(payload["review_id"]) is not int or abs(payload["review_id"]) > 2**53 - 1
                        or not isinstance(payload["approval_id"], str) or len(payload["approval_id"].encode("utf-8")) > 512
                        or payload["decision"] not in ("approve", "deny")):
                    raise ValueError("Invalid Toolkit answer request.")
            submission = (await self._submission(request, body)
                          if is_human and request.method == "POST" and toolkit_post_limit is None else None)
        except (ValueError, UnicodeError, json.JSONDecodeError):
            return Response("Invalid Console request.", status_code=400)
        if self._closing:
            return Response("The Console is stopping.", status_code=503)
        if is_human and not self._disclosure_current(request, login):
            return JSONResponse({"error": "Console login required."}, status_code=401)
        # A browser login acts as its own account; a host health probe as the
        # installation's own account, with no login association.
        headers = {} if toolkit_callback_landing else {
            "Authorization": "Bearer " + self._native_token(login if is_human else None)}
        if toolkit_post_limit is not None:
            headers["Origin"] = self.origin
        for name in ("content-type", "accept", "last-event-id", "range"):
            if name in request.headers and not toolkit_callback_landing:
                headers[name] = request.headers[name]
        # The landing asset reads its browser location and submits the closed
        # callback envelope. Its GET needs only fixed bytes, never callback data.
        # httpx emits a query delimiter for b"". Native protected setup requires
        # no query at all, so absent and stripped queries must use None.
        url = self.client.base_url.copy_with(scheme="http", host="127.0.0.1", port=urlsplit(self.native_url).port,
                                              path=path, query=None if toolkit_callback_landing or not request.url.query
                                              else request.url.query.encode())
        subscription = None
        if is_human and request.method == "GET" and path.endswith("/stream"):
            subscription = _ConsoleSubscription(asyncio.current_task())
            login.subscriptions.add(subscription)
        try:
            import httpx
            timeout = httpx.Timeout(None, connect=5, write=60, pool=5) if path.endswith("/stream") else 60
            native_request = self.client.build_request(request.method, url, content=body, headers=headers, timeout=timeout)
            # The shared client's response-cookie jar must not supply authority
            # to another login or to the public fixed callback landing.
            native_request.headers.pop("cookie", None)
            upstream = await self.client.send(native_request, stream=True)
            if subscription is not None:
                self._own_upstream(subscription, upstream)
        except BaseException as error:
            if subscription is not None:
                login.subscriptions.discard(subscription)
            if not isinstance(error, Exception):
                raise
            return Response("The native Console is unavailable.", status_code=502)
        forwarded = {key: value for key, value in upstream.headers.items()
                     if key.lower() in {"content-type", "content-range", "accept-ranges", "etag"}}
        forwarded["Cache-Control"] = "no-store"
        forwarded["X-Content-Type-Options"] = "nosniff"
        panel_html = (path == "/host/toolkit/setup"
                      and upstream.headers.get("content-type", "").split(";", 1)[0].strip().lower() == "text/html")
        forwarded["Content-Security-Policy"] = "frame-ancestors 'self'" if panel_html else "frame-ancestors 'none'"
        forwarded["Referrer-Policy"] = "no-referrer"
        if toolkit_callback_landing:
            policy = upstream.headers.get("content-security-policy", "")
            if (upstream.status_code != 200
                    or upstream.headers.get("content-type", "").split(";", 1)[0].strip().lower() != "text/html"
                    or not TOOLKIT_LANDING_CSP.fullmatch(policy)):
                await upstream.aclose()
                return Response("The Toolkit sign-in landing is unavailable.", status_code=502,
                                headers=_console_public_headers())
            forwarded["Content-Security-Policy"] = policy
        if toolkit_post_limit is None and "text/event-stream" in upstream.headers.get("content-type", ""):
            if not is_human or not self._disclosure_current(request, login):
                if subscription is not None:
                    await self._close_subscription(subscription)
                else:
                    await upstream.aclose()
                if subscription is not None:
                    login.subscriptions.discard(subscription)
                return JSONResponse({"error": "Console login required."}, status_code=401)
            if subscription is None:
                subscription = _ConsoleSubscription(asyncio.current_task())
                self._own_upstream(subscription, upstream)
                login.subscriptions.add(subscription)
            async def stream():
                try:
                    async for chunk in upstream.aiter_raw():
                        if not self._disclosure_current(request, login):
                            return
                        yield chunk
                finally:
                    await self._close_subscription(subscription)
                    login.subscriptions.discard(subscription)
            return StreamingResponse(stream(), status_code=upstream.status_code, headers=forwarded)
        try:
            payload = await upstream.aread()
        finally:
            if subscription is not None:
                await self._close_subscription(subscription)
                login.subscriptions.discard(subscription)
            else:
                await upstream.aclose()
        if is_human and (toolkit_setup or toolkit_post_limit is not None) and not self._disclosure_current(request, login):
            return JSONResponse({"error": "Console login required."}, status_code=401,
                                headers=_console_public_headers())
        if (self.document_transform is not None and is_human and request.method == "GET"
                and path in ("/console", "/console/") and upstream.status_code == 200
                and upstream.headers.get("content-type", "").split(";", 1)[0].strip().lower() == "text/html"):
            if not self._disclosure_current(request, login):
                return JSONResponse({"error": "Console login required."}, status_code=401)
            try:
                if len(payload) > MAX_CONSOLE_ASSET:
                    raise ValueError("The Console document exceeds its limit.")
                transformed = self.document_transform(payload)
                if not isinstance(transformed, bytes) or len(transformed) > MAX_CONSOLE_ASSET:
                    raise ValueError("The transformed Console document is invalid.")
                payload = transformed
            except Exception:
                return Response("The native Console is unavailable.", status_code=502,
                                headers=_console_public_headers())
            forwarded.pop("etag", None)
            forwarded.pop("content-range", None)
            forwarded.pop("accept-ranges", None)
        if submission is not None and upstream.is_success and self.on_input:
            try:
                envelope = json.loads(payload)
                accepted = envelope.get("result") if path != "/console/send" else envelope
                if isinstance(accepted, dict) and not envelope.get("error"):
                    await self._deliver(submission, accepted, login.principal)
            except Exception:
                # Native may already have admitted the turn. Retrying its same
                # idempotency key lets the application retry its own delivery.
                return JSONResponse({"error": "Native input was admitted but application delivery failed. Retry the same submission."}, status_code=503)
        if is_human and not self._disclosure_current(request, login):
            return JSONResponse({"error": "Console login required."}, status_code=401,
                                headers=_console_public_headers())
        return Response(payload, status_code=upstream.status_code, headers=forwarded)

    async def _mcp_callback(self, request, login, redirect):
        from starlette.responses import Response
        headers = _console_public_headers()
        if (request.method != "POST" or request.scope["query_string"]
                or request.headers.get("content-type", "").split(";", 1)[0].strip() != "application/json"):
            return Response(status_code=400, headers=headers)
        if login is None or not self._disclosure_current(request, login):
            return Response(status_code=403, headers=headers)
        # Select only the attempt owned by this exact authenticated login.
        matches = [attempt for attempt in login.mcp_attempts.values()
                   if attempt.client is not None and attempt.client.redirect_uri == redirect]
        if len(matches) != 1:
            return Response(status_code=403, headers=headers)
        try:
            body = await self._body(request, MAX_HOSTED_MCP_BODY)
            if not self._disclosure_current(request, login):
                return Response(status_code=403, headers=headers)
            result = await matches[0].callback(method="POST", redirect_uri=redirect,
                origin=request.headers.get("origin"), body=body)
            if not self._disclosure_current(request, login):
                return Response(status_code=403, headers=headers)
            return Response(result.body, status_code=result.status, headers=dict(result.headers))
        except ValueError:
            return Response(status_code=400, headers=headers)
        except Exception:
            return Response(status_code=503, headers=headers)

    def _mount_mcp(self, login, attempt, redirect):
        parsed = urlsplit(redirect)
        path = parsed.path
        # The callback handler compares the raw ASGI path to ASCII bytes. Reject
        # paths that browsers must encode before starting a native ceremony.
        if (not path.isascii() or parsed.scheme + "://" + parsed.netloc != self.origin or not path.startswith("/")
                or parsed.query or parsed.fragment or "%" in path or "//" in path
                or any(part in (".", "..") for part in path.split("/"))
                or path in ("/", "/login", "/logout", AVAILABILITY_PATH)
                or path.startswith(("/console", "/blobs/", "/host/toolkit/setup", "/host/toolkit/consent", "/login/"))):
            raise ValueError("The native callback cannot be mounted on this Console.")
        if any(other is not attempt and other.client is not None and other.client.redirect_uri == redirect
               for other in login.mcp_attempts.values()):
            raise ValueError("The native callback has an ambiguous login target.")
        previous = self._mcp_routes.get(path)
        if previous is not None and previous[0] != redirect:
            raise ValueError("The native callback changed its selected route.")
        self._mcp_routes[path] = (redirect, attempt.binding)

    async def _mcp_request(self, request, login):
        from starlette.responses import HTMLResponse, JSONResponse, RedirectResponse, Response
        headers = _console_public_headers()
        if not self._mcp_targets:
            return Response(status_code=404, headers=headers)
        if login is None or not self._disclosure_current(request, login) or not login.console_login_required:
            return Response(status_code=403, headers=headers)
        if request.scope["query_string"]:
            return Response(status_code=400, headers=headers)
        path = request.url.path
        if path == HOSTED_MCP_PATH:
            if request.method != "GET":
                return Response(status_code=405, headers={**headers, "Allow": "GET"})
            rows = "".join("<li><p>" + html.escape(target.server_name) + "</p>"
                '<form method="post" action="' + HOSTED_MCP_PATH + "/" + key + '/start">'
                '<button type="submit">Sign in</button></form><a href="' + HOSTED_MCP_PATH
                + "/" + key + '">Check credential status</a></li>' for key, target in self._mcp_targets.items())
            return HTMLResponse('<!doctype html><html lang="en"><meta charset="utf-8">'
                '<meta name="viewport" content="width=device-width, initial-scale=1">'
                '<title>MCP sign-in</title><h1>MCP sign-in</h1><p>Sign in to a configured service. '
                'A saved credential does not confirm a connection or permission to use it.</p><ul>' + rows
                + '</ul><a href="/console">Return to Console</a></html>', headers={**headers,
                    "Content-Security-Policy": "default-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"})
        parts = path.removeprefix(HOSTED_MCP_PATH + "/").split("/")
        key = parts[0]
        target = self._mcp_targets.get(key)
        action = "status" if len(parts) == 1 else parts[1] if len(parts) == 2 else None
        if target is None or action not in ("status", "start", "close"):
            return Response(status_code=404, headers=headers)
        if target != self._mcp_original_targets[key]:
            return Response(status_code=409, headers=headers)
        method = "GET" if action == "status" else "POST"
        if request.method != method:
            return Response(status_code=405, headers={**headers, "Allow": method})
        try:
            if await self._body(request, 0):
                return Response(status_code=400, headers=headers)
            if not self._disclosure_current(request, login) or login.mcp_close is not None:
                return Response(status_code=403, headers=headers)
            attempt = login.mcp_attempts.get(key)
            if attempt is None:
                if action == "close":
                    return Response(status_code=204, headers=headers)
                # The factory is synchronous and dormant. Retain it before
                # describe, even if this HTTP waiter is subsequently cancelled.
                attempt = login.console_login.hosted_mcp(self._login_context(login), target)
                login.mcp_attempts[key] = attempt
            if action == "close":
                await attempt.close()
                return Response(status_code=204, headers=headers)
            redirect = await attempt.prepare()
            if not self._disclosure_current(request, login):
                return Response(status_code=403, headers=headers)
            self._mount_mcp(login, attempt, redirect)
            if action == "status":
                observed = await attempt.observe()
                if not self._disclosure_current(request, login):
                    return Response(status_code=403, headers=headers)
                return JSONResponse({"phase": observed.phase}, headers=headers)
            disposition = await attempt.begin()
            if disposition == "joined":
                return JSONResponse({"phase": "joined"}, status_code=202, headers=headers)
            response = None

            async def present(url):
                nonlocal response
                if not self._disclosure_current(request, login):
                    raise ValueError("The Console login changed during presentation.")
                response = RedirectResponse(url, status_code=303, headers=headers)

            presented = await attempt.present(present)
            if not presented or response is None or not self._disclosure_current(request, login):
                return Response(status_code=503, headers=headers)
            return response
        except ValueError:
            return Response(status_code=400, headers=headers)
        except Exception as error:
            # Preserve native diagnostic tokens, never provider text or frames.
            from meerkat_toolkit.mcp_auth import McpAuthError, NativeAuthFailure
            payload = {"error": "mcp_sign_in_unavailable"}
            failure = error.native_failure if isinstance(error, McpAuthError) else None
            if not self._disclosure_current(request, login):
                return Response(status_code=403, headers=headers)
            if isinstance(failure, NativeAuthFailure):
                codes = {"reason": failure.reason}
                if failure.detail is not None:
                    codes["detail"] = failure.detail
                if all(type(code) is str and re.fullmatch(r"[a-z][a-z0-9_]{0,127}", code)
                       for code in codes.values()):
                    payload["native_failure"] = codes
            return JSONResponse(payload, status_code=503, headers=headers)

    def _reserve_controller_review(self, login):
        """Trusted login lifecycle hook; no browser route starts native review.

        The application factory supplies a pending exact retained original and
        source on its existing client. A successful login only binds disclosure
        and invocation lifetime; it does not enroll a root or grant permission.
        The review keeps this login's correlation (scope, deadline, origin) for
        its native invocation, but it is the front's: its currentness is the
        front's and that deadline, so it outlives this login.
        """
        if (self.controller_review_factory is None or self._controller_review_created
                or not self._login_current(login)):
            return
        self._controller_review_created = True
        try:
            if __package__:
                from .native_controller_review import NativeControllerReview
            else:
                from native_controller_review import NativeControllerReview
            context = self._review_context(login)
            owner = self.controller_review_factory(context)
            if owner is None:
                return
            if (not isinstance(owner, NativeControllerReview)
                    or owner.login_scope_id != context.login_scope_id
                    or owner.expires_at_ms != context.expires_at_ms or owner.origin != context.origin):
                raise ValueError("Invalid protected native review binding.")
            application_current = owner.is_current
            owner.is_current = lambda: context.is_current() and application_current() is True
            self._controller_review_owner = owner
            owner.start()
        except Exception:
            self._controller_review_failure = "invalid_binding"

    def _review_context(self, login):
        principal, scope, origin = login.principal, login.login_scope_id, self.origin
        deadline = int(login.expires * 1000)
        return ConsoleConnectLogin(principal, scope, deadline, origin,
            lambda: not self._closing and self.origin == origin and time.time() * 1000 < deadline,
            issuer=self._token_issuer, subject=login.subject)

    async def _console_administrator(self, login):
        """Whether the console names this login's person an administrator.

        Read from the console's `meta/console/whoami` with a token of the
        login, so an access edit applies at once. None when it cannot tell.
        `_CONSOLE_PREDATES_WHOAMI` when the console answers "method not
        found", as for its sign-out: such a console names no administrator.
        """
        import httpx
        token = self._native_token(login)
        try:
            outgoing = self.client.build_request("POST", self.native_url + "/console/rpc",
                json={"jsonrpc": "2.0", "id": "forge-whoami", "method": "meta/console/whoami", "params": {}},
                headers={"Authorization": "Bearer " + token}, timeout=httpx.Timeout(15, connect=5))
            outgoing.headers.pop("cookie", None)
            response = await self.client.send(outgoing)
            value = response.json() if response.status_code == 200 else None
        except Exception:
            return None
        if _method_not_found(value):
            return _CONSOLE_PREDATES_WHOAMI
        result = value.get("result") if isinstance(value, dict) else None
        if not isinstance(result, dict) or not isinstance(result.get("actor"), dict):
            return None
        return result["actor"].get("kind") == "person" and result.get("can_administer") is True

    async def historical_controller_outcome(self):
        """Trusted application/compiler input, never a browser endpoint.

        Retained across logout and expiry. The complete native lineage is only
        historical data and must enter normal native compilation/publication.
        """
        if self._controller_review_owner is None:
            raise RuntimeError("The application has no retained native review.")
        return await self._controller_review_owner.historical_outcome()

    async def _controller_review_request(self, request, login):
        from starlette.responses import JSONResponse
        headers = _console_public_headers()
        def refusal(code, status):
            return JSONResponse({"error": {"code": code}}, status_code=status, headers=headers)
        suffix = request.url.path.removeprefix(CONTROLLER_REVIEW_PATH)
        if suffix not in {"/state", "/answer"}:
            return refusal("invalid_message", 404)
        if request.method != ("POST" if suffix == "/answer" else "GET"):
            return refusal("invalid_message", 405)
        if request.url.query:
            return refusal("invalid_message", 400)
        if login is None or not self._login_current(login):
            return refusal("invalid_binding", 401)
        if self.controller_review_factory is None:
            return refusal("unsupported_presenter", 501)
        if not login.person:
            # The review asks for a decision, and an account people share
            # decides nothing.
            return refusal("approver_unauthenticated", 403)
        if self._controller_review_failure:
            return refusal(self._controller_review_failure, 409)
        if suffix == "/state" and not self._controller_review_created:
            # The start at sign-in may not have run: the console could not
            # tell then, or another person's check held it. Opening the
            # review tries again for this login.
            await self._ensure_controller_review(login)
            if not self._login_current(login):
                return refusal("invalid_binding", 401)
            if self._controller_review_failure:
                return refusal(self._controller_review_failure, 409)
        owner = self._controller_review_owner
        if owner is None or not owner.is_current():
            return refusal("unavailable", 409)
        if __package__:
            from .native_controller_review import ControllerReviewError
        else:
            from native_controller_review import ControllerReviewError

        async def administrator():
            # Any administrator's login sees and answers the review. Read the
            # console's answer for this login each time: an access edit
            # applies at once. None: the console could not tell.
            held = await self._console_administrator(login)
            if held is None:
                return refusal("unavailable", 503)
            if not self._disclosure_current(request, login):
                return refusal("invalid_binding", 401)
            if held is _CONSOLE_PREDATES_WHOAMI:
                # Such a console names no administrator: only the login the
                # review started at sees and answers it.
                return None if owner.login_scope_id == login.login_scope_id else refusal("unavailable", 409)
            return None if held else refusal("not_administrator", 403)
        try:
            if suffix == "/answer":
                if request.headers.get("content-type", "").split(";", 1)[0].strip() != "application/json":
                    return refusal("invalid_message", 400)
                def invalid_constant(_):
                    raise ValueError("Invalid JSON number.")
                payload = json.loads((await self._body(request, MAX_CONTROLLER_REVIEW_BODY)).decode("utf-8"),
                    object_pairs_hook=_frontend_object, parse_constant=invalid_constant)
                if not isinstance(payload, dict) or set(payload) != {"approval_id", "decision"}:
                    return refusal("invalid_message", 400)
                refused = await administrator()
                if refused is not None:
                    return refused
                # Exact login and application guard, then claim, with no await.
                if not self._disclosure_current(request, login):
                    return refusal("invalid_binding", 401)
                owner.claim_answer(payload["approval_id"], payload["decision"])
                value, status = {"status": "answer_submitted"}, 202
            else:
                refused = await administrator()
                if refused is not None:
                    return refused
                value = await owner.observe()
                status = 202 if value["status"] == "starting" else 200
                # The read awaited: show it only to a person who still administers.
                refused = await administrator()
                if refused is not None:
                    return refused
            if not self._disclosure_current(request, login):
                return refusal("invalid_binding", 401)
            return JSONResponse(value, status_code=status, headers=headers)
        except ControllerReviewError as error:
            return refusal(error.code, 401 if error.code == "invalid_binding" else 409)
        except (ValueError, UnicodeError):
            return refusal("invalid_message", 400)
        except Exception:
            return refusal("unavailable", 503)

    def _reserve_connect(self, login):
        """Create/start once for this exact login, never for a browser selector."""
        try:
            from .console_connect import ConsoleConnect, ConnectError
        except ImportError:
            from console_connect import ConsoleConnect, ConnectError
        if not self._login_current(login):
            raise ConnectError("invalid_binding")
        if self.connect_factory is None:
            raise ConnectError("unsupported_presenter")
        if not login.connect_created:
            login.connect_created = True
            context = self._login_context(login)
            try:
                bridge = self.connect_factory(context)
                if not isinstance(bridge, ConsoleConnect):
                    raise ValueError("Invalid Connect factory result.")
                if (bridge.bootstrap["viewer"] != {"principal_id": context.principal,
                        "login_scope_id": context.login_scope_id, "expires_at_ms": context.expires_at_ms}
                        or bridge.bootstrap["application"]["origin"] != context.origin):
                    raise ValueError("Invalid Connect login binding.")
                application_current = bridge.is_current
                bridge.is_current = lambda: self._login_current(login) and application_current() is True
                login.connect = bridge
                login.connect_start = asyncio.create_task(bridge.start())
                login.connect_start.add_done_callback(lambda task: task.exception() if not task.cancelled() else None)
            except Exception:
                login.connect_failure = "invalid_binding"
        if login.connect_failure is not None:
            raise ConnectError(login.connect_failure)
        return login.connect

    async def _connect_request(self, request, login):
        """Closed authenticated setup transport, separate from native Console."""
        from starlette.responses import JSONResponse, StreamingResponse
        try:
            from .console_connect import ConnectError, REFUSALS, MAX_UINT
        except ImportError:
            from console_connect import ConnectError, REFUSALS, MAX_UINT
        headers = {"Cache-Control": "no-store", "X-Content-Type-Options": "nosniff",
                   "Referrer-Policy": "no-referrer", "Content-Security-Policy": "frame-ancestors 'none'"}
        def refusal(code, status=None):
            if code not in REFUSALS:
                code = "unavailable"
            if status is None:
                status = (400 if code in {"invalid_message", "invalid_answer"} else
                          403 if code == "approver_unauthenticated" else
                          409 if code in {"busy", "stale_prompt", "invalid_binding", "binding_retired", "stop_unavailable"} else
                          501 if code == "unsupported_presenter" else 503)
            return JSONResponse({"error": {"code": code}}, status_code=status, headers=headers)
        suffix = request.url.path.removeprefix(CONNECT_PATH)
        result_match = re.fullmatch(r"/result/(h:[A-Za-z0-9_-]{1,64})", suffix, re.ASCII)
        association_match = re.fullmatch(r"/send/(b:[0-9a-f]{32})", suffix, re.ASCII)
        method = "POST" if suffix in {"/initialize", "/send", "/cancel", "/answer"} else "GET"
        if suffix not in {"/initialize", "/state", "/history", "/send", "/cancel", "/answer",
                          "/events/public", "/events/private"} and not result_match and not association_match:
            return refusal("invalid_message", 404)
        if request.method != method:
            return refusal("invalid_message", 405)
        if login is None or not self._login_current(login):
            return refusal("invalid_binding", 401)
        try:
            query = parse_qsl(request.url.query, keep_blank_values=True, strict_parsing=True)
            offset = None
            if query:
                if suffix != "/history" or len(query) != 1 or query[0][0] != "offset" or not re.fullmatch(r"[0-9]+", query[0][1]):
                    raise ValueError("Invalid query.")
                offset = int(query[0][1])
                if not 0 <= offset <= MAX_UINT:
                    raise ValueError("Invalid offset.")
            payload = None
            if method == "POST":
                if request.headers.get("content-type", "").split(";", 1)[0].strip() != "application/json":
                    raise ValueError("JSON is required.")
                def invalid_constant(_):
                    raise ValueError("Invalid number.")
                body = await self._body(request, MAX_CONNECT_BODY)
                payload = json.loads(body.decode("utf-8"), object_pairs_hook=_frontend_object,
                    parse_constant=invalid_constant)
                _frontend_json(payload) if suffix != "/send" else json.dumps(payload, allow_nan=False)
                required = {"/initialize": set(), "/send": {"text"},
                            "/cancel": {"send_request_id"}, "/answer": {"prompt_id", "answer"}}[suffix]
                if not isinstance(payload, dict) or (set(payload) != required and
                        not (suffix == "/send" and set(payload) == {"text", "association"})):
                    raise ValueError("Invalid request fields.")
                if suffix == "/send" and "association" in payload and (
                        not isinstance(payload["association"], str) or
                        not re.fullmatch(r"b:[0-9a-f]{32}", payload["association"], re.ASCII)):
                    raise ValueError("Invalid browser association.")
            # No await between this actual-login check and bridge reservation.
            if not self._disclosure_current(request, login):
                return refusal("invalid_binding", 401)
            if suffix == "/answer" and not login.person:
                # Every setup prompt asks for a decision or a credential, and
                # an account people share decides nothing.
                return refusal("approver_unauthenticated")
            if self.connect_factory is None:
                return refusal("unsupported_presenter")
            if suffix == "/initialize":
                bridge = self._reserve_connect(login)
            else:
                if not login.connect_created or login.connect is None:
                    return refusal("unavailable")
                if login.connect_failure:
                    return refusal(login.connect_failure)
                bridge = login.connect
            if suffix in {"/initialize", "/state"}:
                if not login.connect_start.done():
                    value, status = {"status": "starting"}, 202
                else:
                    # Retrieving this original start never repeats it or reads
                    # history to invent send/prompt controls.
                    login.connect_start.result()
                    value, status = {"status": "ready", "state": bridge.snapshot(),
                                     "capabilities": copy.deepcopy(bridge.capabilities)}, 200
                # Protocol 3 can request private provider interaction before
                # initialize completes. This is the host's original retained
                # request target, not a fabricated native session or readiness.
                # Legacy projections retain their exact existing shape.
                binding = bridge.presentation_binding()
                if binding is not None:
                    value["binding"] = binding
            elif suffix == "/history":
                value, status = await bridge.history(offset), 200
            elif suffix == "/send":
                identity = (await bridge.begin_send(payload["text"], browser_association=payload["association"])
                            if "association" in payload else await bridge.begin_send(payload["text"]))
                value, status = bridge.retained_result(identity), 202
                if "association" in payload:
                    value = {"association": payload["association"], **value}
            elif association_match:
                value, status = bridge.retained_send(association_match[1]), 200
            elif result_match:
                value, status = bridge.retained_result(result_match[1]), 200
            elif suffix == "/cancel":
                value, status = await bridge.cancel(payload["send_request_id"]), 200
            elif suffix == "/answer":
                await bridge.answer(payload["prompt_id"], payload["answer"])
                value, status = {"status": "forwarded"}, 202
            else:
                subscription = _ConsoleSubscription(asyncio.current_task())
                login.subscriptions.add(subscription)
                try:
                    observer = await bridge.attach(private=suffix == "/events/private")
                    subscription.upstream, subscription.original_close = observer, observer.close
                    if not self._disclosure_current(request, login):
                        await self._close_subscription(subscription)
                        login.subscriptions.discard(subscription)
                        return refusal("invalid_binding", 401)
                except BaseException:
                    login.subscriptions.discard(subscription)
                    raise
                async def stream():
                    try:
                        async for item in observer:
                            if not self._disclosure_current(request, login):
                                return
                            # The actual child reader owns snapshot/event order.
                            # Private prompts never enter the native proxy stream.
                            yield b"data: " + json.dumps(item, ensure_ascii=False, allow_nan=False,
                                separators=(",", ":")).encode("utf-8") + b"\n\n"
                    finally:
                        await self._close_subscription(subscription)
                        login.subscriptions.discard(subscription)
                return StreamingResponse(stream(), media_type="text/event-stream", headers=headers)
            if not self._disclosure_current(request, login):
                return refusal("invalid_binding", 401)
            return JSONResponse(value, status_code=status, headers=headers)
        except ConnectError as error:
            return refusal(error.code)
        except (ValueError, TypeError, UnicodeError, RecursionError):
            return refusal("invalid_message")
        except Exception:
            return refusal("unavailable")

    async def _frontend_descriptor(self, request, login):
        """Read retained loaded-source declarations, without a native command."""
        from starlette.responses import JSONResponse, Response
        if request.method != "GET" or request.url.query:
            return JSONResponse({"detail": "The frontend descriptor requires GET without query parameters."},
                status_code=405 if request.method != "GET" else 400, headers=_console_public_headers())
        if not callable(self.frontend_management) or not callable(self.frontend_host):
            return _frontend_descriptor_error(501)
        try:
            host = self.frontend_host()
        except Exception:
            return _frontend_descriptor_error(409)
        try:
            if (not isinstance(host, dict) or set(host) != {"protocol", "host_id", "application_id", "source_digest"}
                    or type(host["protocol"]) is not int or host["protocol"] != 1
                    or not isinstance(host["host_id"], str) or not re.fullmatch(r"forge:[a-f0-9]{64}", host["host_id"])
                    or not isinstance(host["application_id"], str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", host["application_id"])
                    or not isinstance(host["source_digest"], str) or not re.fullmatch(r"[a-f0-9]{64}", host["source_digest"])):
                raise ValueError("The retained frontend host is invalid.")
            host = copy.deepcopy(host)
            fields = self.frontend_private_fields
            if (not isinstance(fields, tuple) or any(not isinstance(name, str) for name in fields)
                    or len(set(fields)) != len(fields)):
                raise ValueError("The retained private declarations are invalid.")
            envelope = _frontend_json({"protocol": 1, "host": host, "private_fields": list(fields)})
        except (ValueError, TypeError, UnicodeError, RecursionError):
            return _frontend_descriptor_error(503)
        try:
            if await self._body(request, MAX_FRONTEND_BODY):
                raise ValueError("The descriptor has no request body.")
        except ValueError:
            return JSONResponse({"detail": "The frontend descriptor has no request body."},
                                status_code=400, headers=_console_public_headers())
        if self._closing:
            return _frontend_descriptor_error(503)
        if self._live_login(request) is not login:
            return _frontend_descriptor_error(401)
        try:
            if self.frontend_host() != host or self.frontend_private_fields != fields:
                return _frontend_descriptor_error(409)
        except Exception:
            return _frontend_descriptor_error(409)
        if self._closing:
            return _frontend_descriptor_error(503)
        if self._live_login(request) is not login:
            return _frontend_descriptor_error(401)
        if not self._disclosure_current(request, login):
            return _frontend_descriptor_error(503)
        return Response(envelope, media_type="application/json", headers=_console_public_headers())

    async def _frontend_request(self, request, login):
        """Delegate component truth to one explicit application owner, once."""
        from starlette.responses import JSONResponse
        def error(code, detail, *, change_id=None):
            result = {"detail": detail}
            if change_id is not None:
                result.update(status="unknown", change_id=change_id)
            return JSONResponse(result, status_code=code, headers={"Cache-Control": "no-store"})

        # Every command names the login's own account; a proof binds it and
        # the login itself.
        principal = self.principal if login is None else login.principal
        scope = None if login is None else login.login_scope_id
        path = request.url.path
        if path == FRONTEND_PATH:
            operation, method = "inventory", "GET"
        elif path in (FRONTEND_PATH + "/prepare", FRONTEND_PATH + "/submit"):
            operation, method = path.rsplit("/", 1)[1], "POST"
        elif path.startswith(FRONTEND_PATH + "/changes/"):
            operation, method = "status", "GET"
            change_id = path.removeprefix(FRONTEND_PATH + "/changes/")
            if not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", change_id):
                return error(400, "Use the exact frontend change identifier.")
        else:
            return error(404, "Unknown frontend management route.")
        if request.method != method:
            return error(405, "This frontend management method is not supported.")
        if not callable(self.frontend_management) or not callable(self.frontend_host):
            return error(501, "This application has not declared frontend management support.")
        if request.url.query:
            return error(400, "Frontend management does not accept query parameters.")
        if operation in ("prepare", "submit") and (login is None or not login.person):
            # A proposal exists to be approved, and only a person approves:
            # an account people share decides nothing. Reads stay open.
            return JSONResponse({"detail": "Approving frontend changes needs your own named account. "
                                 "A shared Console account approves nothing.", "code": "approver_unauthenticated"},
                                status_code=403, headers={"Cache-Control": "no-store"})
        try:
            host = self.frontend_host()
            _frontend_json(host)
        except Exception:
            return error(409, "The loaded application source changed. No frontend command was admitted.")

        payload, approved = {}, False
        try:
            if method == "GET":
                if await self._body(request, MAX_FRONTEND_BODY):
                    raise ValueError("A read command has no body.")
                if operation == "status":
                    payload = {"change_id": change_id}
            else:
                if request.headers.get("content-type", "").split(";", 1)[0].strip() != "application/json":
                    raise ValueError("JSON is required.")
                payload = json.loads(await self._body(request, MAX_FRONTEND_BODY),
                                     object_pairs_hook=_frontend_object)
                _frontend_json(payload)
                self._frontend_no_private_literals(payload)
                if not isinstance(payload, dict):
                    raise ValueError("A command is an object.")
                if operation == "prepare":
                    if set(payload) != {"configuration", "expected_revision"} or not isinstance(payload["configuration"], dict):
                        raise ValueError("Prepare needs exact configuration and revision.")
                else:
                    if set(payload) != {"proposal", "approval"}:
                        raise ValueError("Submit needs the exact proposal and protected approval.")
                    self._frontend_proposal(payload["proposal"])
                    approval = payload["approval"]
                    if (not isinstance(approval, dict) or set(approval) != {"approved", "expires_at", "proof"}
                            or approval["approved"] is not True or type(approval["expires_at"]) is not int
                            or not time.time() < approval["expires_at"] <= time.time() + FRONTEND_APPROVAL_SECONDS
                            or not isinstance(approval["proof"], str)
                            or not hmac.compare_digest(approval["proof"], self._frontend_proof(host, payload["proposal"], approval["expires_at"], principal, scope))):
                        return error(403, "Explicit approval of this exact current proposal is required.")
                    approved = True
                    payload = {"proposal": payload["proposal"],
                               "approval": {"principal": principal, "expires_at": approval["expires_at"]}}
            command = FrontendManagementRequest(operation, principal, copy.deepcopy(host), copy.deepcopy(payload), approved)
        except (ValueError, TypeError, UnicodeError, RecursionError, KeyError):
            return error(400, "Invalid frontend command. Use nonsecret configuration and the exact proposal.")

        if self._closing:
            return error(503, "The Console is stopping. No frontend command was admitted.")
        if not self._disclosure_current(request, login):
            return error(401, "Console login required. No frontend command was admitted.")
        try:
            result = self.frontend_management(command)
            if inspect.isawaitable(result):
                result = await result
            if not self._disclosure_current(request, login):
                return error(401, "Console login required.")
            submission_outcome = None
            if isinstance(result, FrontendSubmissionResult):
                if (type(result) is not FrontendSubmissionResult or operation != "submit"
                        or type(result.disposition) is not FrontendSubmissionDisposition
                        or result.disposition not in (FrontendSubmissionDisposition.KNOWN,
                            FrontendSubmissionDisposition.UNKNOWN)
                        or type(result.observation) is not dict or type(result.host) is not dict
                        or set(result.host) != {"protocol", "host_id", "application_id", "source_digest"}
                        or type(result.host["protocol"]) is not int or result.host["protocol"] != 1
                        or not isinstance(result.host["host_id"], str)
                        or not re.fullmatch(r"forge:[a-f0-9]{64}", result.host["host_id"])
                        or not isinstance(result.host["application_id"], str)
                        or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", result.host["application_id"])
                        or not isinstance(result.host["source_digest"], str)
                        or not re.fullmatch(r"[a-f0-9]{64}", result.host["source_digest"])
                        or _frontend_json(result.host) != _frontend_json(host)
                        or type(result.change_id) is not str
                        or result.change_id != payload["proposal"]["change_id"]):
                    raise ValueError("The explicit frontend submission result lost its original binding.")
                submission_outcome = {"change_id": result.change_id,
                    "disposition": result.disposition.value}
                result = result.observation
            if not isinstance(result, dict):
                raise ValueError("The application observation must be an object.")
            _frontend_json(result)
            self._frontend_no_private_literals(result)
            if self.frontend_host() != host:
                raise ValueError("The application source changed during the command.")
            if (submission_outcome is not None
                    and _frontend_json(self.frontend_host()) != _frontend_json(host)):
                raise ValueError("The explicit frontend submission source binding changed.")
            envelope = {"protocol": 1, "host": host}
            if operation == "prepare":
                proposal = result.get("proposal")
                self._frontend_proposal(proposal)
                expires_at = int(time.time()) + FRONTEND_APPROVAL_SECONDS
                envelope.update(proposal=proposal, approval={"expires_at": expires_at,
                    "proof": self._frontend_proof(host, proposal, expires_at, principal, scope)})
            else:
                envelope["observation"] = result
                if (operation == "submit" and submission_outcome is not None
                        and request.headers.getlist("X-Forge-Frontend-Submission-Outcome") == ["1"]):
                    envelope["submission_outcome"] = submission_outcome
            _frontend_json(envelope)
            if not self._disclosure_current(request, login):
                return error(401, "Console login required.")
            return JSONResponse(envelope, headers={"Cache-Control": "no-store"})
        except Exception:
            if operation == "submit":
                return error(503, "Frontend submission outcome is unknown. Read this exact change status before any further action.",
                             change_id=payload["proposal"]["change_id"])
            return error(503, "The application frontend owner is unavailable. No activation is claimed.")

    def _frontend_proposal(self, proposal):
        if (not isinstance(proposal, dict) or not {"change_id", "configuration", "expected_revision"} <= set(proposal)
                or not isinstance(proposal["configuration"], dict)
                or not isinstance(proposal["change_id"], str)
                or not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", proposal["change_id"])):
            raise ValueError("An exact application proposal is required.")
        _frontend_json(proposal)
        self._frontend_no_private_literals(proposal)

    def _frontend_proof(self, host, proposal, expires_at, principal=None, login_scope_id=None):
        # The proof is the login's that prepared the proposal: another login,
        # the same account's included, cannot submit it, nor can anyone once
        # that login has ended.
        binding = {"scope": "forge-frontend-proposal-v2", "host": host,
                   "principal": self.principal if principal is None else principal,
                   "login": login_scope_id, "proposal": proposal, "expires_at": expires_at}
        return hmac.new(self.key.encode(), _frontend_json(binding), hashlib.sha256).hexdigest()

    def _frontend_no_private_literals(self, value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key in self.frontend_private_fields and child != {"private_field": key}:
                    raise ValueError("Private values stay with the existing private setup owner.")
                self._frontend_no_private_literals(child)
        elif isinstance(value, list):
            for child in value:
                self._frontend_no_private_literals(child)

    async def _submission(self, request, body):
        path = request.url.path
        if path == "/console/send":
            return json.loads(body)
        if path == "/console/rpc":
            value = json.loads(body)
        elif path == "/console/rpc/multipart":
            # Inspect the same bounded bytes forwarded verbatim to native.
            # Upload substitutions stay native-owned, content is submitted data.
            from starlette.requests import Request
            sent = False
            async def receive():
                nonlocal sent
                chunk = body if not sent else b""
                sent = True
                return {"type": "http.request", "body": chunk, "more_body": False}
            copy = Request(request.scope, receive)
            async with copy.form(max_files=4, max_fields=8, max_part_size=2 * 1024 * 1024) as form:
                value = json.loads(form.get("payload", "{}"))
        else:
            return None
        if not isinstance(value, dict):
            raise ValueError("Console RPC must be an object.")
        return value.get("params") if value.get("method") == self._console_send_method else None

    async def _deliver(self, submission, accepted, principal):
        fields = ("identity", "interaction_id", "session_id", "input_frame_id", "cursor")
        if (not isinstance(submission, dict) or "content" not in submission
                or any(not isinstance(accepted.get(key), str) or not accepted[key] for key in fields)
                or accepted["identity"] != submission.get("identity")):
            raise ValueError("Native admission evidence is incomplete.")
        event = ConsoleInput(channel="host-authenticated-console", principal=principal,
            identity=accepted["identity"], content=submission["content"], interaction_id=accepted["interaction_id"],
            session_id=accepted["session_id"], input_frame_id=accepted["input_frame_id"], cursor=accepted["cursor"],
            conversation_id=accepted.get("conversation_id"), handling_mode=submission.get("handling_mode"))
        async with self.delivery_lock:
            if event.interaction_id in self.delivered:
                return
            if len(self.delivered) >= 10000:
                raise RuntimeError("Application delivery deduplication is full; restart after persisting application evidence.")
            result = self.on_input(event)
            if inspect.isawaitable(result):
                await asyncio.wait_for(result, 10)
            self.delivered.add(event.interaction_id)


def _signed_out_page(problem):
    """A logout whose cleanup is not confirmed: signed out here, said plainly."""
    from starlette.responses import HTMLResponse
    return HTMLResponse(f'''<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Signed out</title><style>body{{font:16px system-ui;background:#faf9f6;color:#242522;margin:0;padding:6vh 20px}}main{{max-width:400px;margin:auto}}p{{line-height:1.6}}</style></head><body><main><h1>Signed out</h1><p role="alert">{html.escape(problem)}</p><p><a href="/login">Sign in again</a></p></main></body></html>''',
        status_code=503, headers={"Cache-Control": "no-store", "X-Content-Type-Options": "nosniff",
            "Referrer-Policy": "no-referrer",
            "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"})


def _login_form(nonce, error=None, *, named=False, provider=None):
    notice = f'<p id="login-error" role="alert">{html.escape(error)}</p>' if error else ""
    description = ' aria-describedby="login-error"' if error else ""
    button = "Open Console"
    if provider is not None:
        guidance = (f"Sign in with your account at this Console's identity provider ({html.escape(provider)}). "
                    "Only the people its operator lists can open it.")
        account = password = ""
        button = "Continue to sign in"
    elif named:
        guidance = ("Sign in with your account name and password. Leave the account empty to use this "
                    "installation's own account and the Console password chosen during installation. "
                    "Your provider API key is separate.")
        account = (f'<label for="account">Account</label><input id="account" name="account" '
                   f'autocomplete="username" autocapitalize="none" spellcheck="false" maxlength="200" '
                   f'autofocus{description}>')
        password = f'<label for="password">Password</label><input id="password" type="password" name="password" autocomplete="current-password" required{description}>'
    else:
        guidance = ("Enter the private Console password you chose during installation. Your configured user "
                    "identity is applied automatically. Your provider API key is separate.")
        account = ""
        password = f'<label for="password">Console password</label><input id="password" type="password" name="password" autocomplete="current-password" required autofocus{description}>'
    return f'''<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>MobKit Console login</title><style>body{{font:16px system-ui;background:#faf9f6;color:#242522;margin:0;padding:6vh 20px}}main{{max-width:400px;margin:auto}}p{{line-height:1.6}}input,button{{font:inherit;padding:12px;box-sizing:border-box;width:100%;margin:12px 0}}button{{background:#242522;color:white;border:0;border-radius:6px}}#login-error{{padding:12px;background:#fbece5;color:#783d2e;border:1px solid #dab19d;border-radius:6px}}</style></head><body><main><h1>Open your Console</h1><p>{guidance}</p>{notice}<form method="post" action="/login"><input type="hidden" name="nonce" value="{html.escape(nonce)}">{account}{password}<button>{button}</button></form></main></body></html>'''


def _oidc_page_headers():
    # The callback address holds the provider's code: no page here sends it on.
    return {"Cache-Control": "no-store", "Referrer-Policy": "no-referrer", "X-Content-Type-Options": "nosniff",
            "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"}


def _oidc_page(title, body):
    return f'''<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">{body[0]}<title>{title}</title><style>body{{font:16px system-ui;background:#faf9f6;color:#242522;margin:0;padding:6vh 20px}}main{{max-width:400px;margin:auto}}p{{line-height:1.6}}</style></head><body><main><h1>{title}</h1>{body[1]}</main></body></html>'''


def _oidc_landing_page():
    """Signed in: a same-site step on to the Console, which sends the Strict session cookie."""
    return _oidc_page("Signed in", ('<meta http-equiv="refresh" content="0;url=/console">',
                                    '<p><a href="/console">Open the Console</a></p>'))


def _oidc_onward_page(url):
    """The start's answer: a same-origin step on to the provider's authorization URL.

    The URL is one escaped attribute value, in the meta refresh and the link.
    """
    target = html.escape(url, quote=True)
    return _oidc_page("Continue to sign in", (f'<meta http-equiv="refresh" content="0;url={target}">',
                                              f'<p><a href="{target}">Continue to the identity provider</a></p>'))


def _oidc_refusal_page(message):
    return _oidc_page("Not signed in", ("", f'<p role="alert">{html.escape(message)}</p>'
                                            '<p><a href="/login">Sign in again</a></p>'))


from contextlib import contextmanager


@contextmanager
def _no_signals():
    yield
