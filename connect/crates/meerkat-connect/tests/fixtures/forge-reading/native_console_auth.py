"""Local login front for the unchanged native Console and admitted human input.

The host authenticates this channel; native admission remains native authority.
Native timeline frames do not attest the authenticated human principal.
"""
from __future__ import annotations

import asyncio
import copy
from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import Enum
import hashlib
import hmac
import html
import inspect
import json
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import time
from types import MappingProxyType
from urllib.parse import parse_qsl, urlsplit

try:
    from .console_configuration import console_configuration, validate_console_credentials
except ImportError:
    from console_configuration import console_configuration, validate_console_credentials

ISSUER = "http://127.0.0.1/mobkit-gateway"
AUDIENCE = "persistent-gateway"
HEALTH_AUDIENCE = "mobkit-launch-host-health"
SESSION_SECONDS = 8 * 60 * 60
HEALTH_PATHS = {"/console/experience", "/console/identities"}
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
    "/host/toolkit/setup/start": 1024,
    "/host/toolkit/setup/callback": 64 * 1024,
    "/host/toolkit/setup/answer": 4 * 1024,
})
TOOLKIT_LANDING_CSP = re.compile(
    r"default-src 'none'; script-src 'sha256-[A-Za-z0-9+/]{43}='; connect-src 'self'; "
    r"frame-ancestors 'none'; base-uri 'none'; form-action 'none'")


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


def _private_key(root, *, create=False):
    directory = Path(root) / ".mobkit"
    if create:
        directory.mkdir(mode=0o700, exist_ok=True)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("The Console private state directory is invalid.")
    info = directory.stat()
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise ValueError("The Console state directory must be owned privately by this user.")
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


def _token(key, principal, *, scope="native-console", login_scope_id=None):
    import jwt
    now = int(time.time())
    claims = {"iss": ISSUER, "aud": HEALTH_AUDIENCE if scope == "host-health" else AUDIENCE,
              "sub": principal, "email": principal,
              "email_verified": True, "iat": now, "exp": now + 120,
              "host_scope": scope}
    if login_scope_id is not None:
        claims["sid"] = login_scope_id
    return jwt.encode(claims, key, algorithm="HS256")


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
    subscriptions: set = field(default_factory=set)
    expiry_task: asyncio.Task | None = None
    retirement_task: asyncio.Task | None = None
    retired: bool = False
    login_scope_id: str = field(default_factory=lambda: secrets.token_urlsafe(32))
    connect_created: bool = False
    connect: object = None
    connect_start: asyncio.Task | None = None
    connect_close: asyncio.Task | None = None
    connect_failure: str | None = None
    controller_review: object = None
    controller_review_close: asyncio.Task | None = None
    controller_review_failure: str | None = None
    controller_review_start: asyncio.Task | None = None
    console_login_required: bool = False
    console_login: object = None
    console_login_ready: bool = False
    console_login_failure: str | None = None
    console_login_close: asyncio.Task | None = None
    console_login_revoke_failed: bool = False


class ConsoleFront:
    def __init__(self, root, port, principal, password, on_input=None, *, transport=None, connect_factory=None,
                 console_assets=None, document_transform=None, controller_review_factory=None,
                 console_login_factory=None, runtime_namespace="mobkit"):
        if type(runtime_namespace) is not str or runtime_namespace not in ("mobkit", "meta"):
            raise ValueError("The application must select the mobkit or meta Console runtime namespace.")
        # The trusted application selects one SDK before serving. Browser RPC
        # methods never select or widen the admitted-input delivery hook.
        self._console_send_method = runtime_namespace + "/console/send"
        validate_console_credentials(principal, password)
        self.console_assets = _console_assets(console_assets)
        if document_transform is not None and not callable(document_transform):
            raise ValueError("The Console document transform must be application-owned and callable.")
        self.document_transform = document_transform
        self.root, self.port, self.principal = Path(root), port, principal
        self.origin = f"http://127.0.0.1:{port}"
        self.key = _private_key(root, create=True)
        self.password_digest = hashlib.sha256(password.encode()).digest()
        self.cookie = "mobkit_console_" + hashlib.sha256((str(self.root.resolve()) + str(port)).encode()).hexdigest()[:16]
        self.sessions, self.delivered = {}, set()
        self._logins, self._closing, self._close_task = {}, False, None
        self.login_tokens, self.failures = {}, []
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
        # One pending application-owned review in this host lifetime. A new
        # browser login must not retry an unknown or retired native invocation.
        self._controller_review_created = False
        self._controller_review_owner = None
        self.frontend_management = self.frontend_host = None
        self.frontend_private_fields = ()
        self.client = self.server = self.task = self.listener = None

    @property
    def native_auth(self):
        return {"provider": "jwt", "shared_secret": self.key, "issuer": ISSUER,
                "audience": AUDIENCE, "email_allowlist": [self.principal]}

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
            self.server = uvicorn.Server(uvicorn.Config(app, access_log=False, log_level="error", lifespan="off"))
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
        if login.controller_review is not None:
            login.controller_review.revoke()
        if login.retirement_task is None:
            login.retirement_task = asyncio.create_task(self._finish_login(login, reason))
            # Keep cleanup failures available to logout/close, without logging
            # upstream exception details from an unattended expiry task.
            login.retirement_task.add_done_callback(
                lambda task: task.exception() if not task.cancelled() else None)
        return login.retirement_task

    async def _finish_login(self, login, reason):
        fences = [asyncio.create_task(owner.retire(reason))
                  for owner in (login.connect, login.controller_review) if owner is not None]
        if login.console_login is not None:
            fences.append(asyncio.create_task(self._console_login_cleanup(login.console_login, "retire", reason)))
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
        if login.connect is None and login.controller_review is None and login.console_login is None:
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
            return (owner is not None and owner.issuer == ISSUER and owner.subject == self.principal
                    and owner.login_scope_id == login.login_scope_id
                    and type(owner.expires_at_ms) is int and owner.expires_at_ms == int(login.expires * 1000)
                    and owner.is_current() is True)
        except Exception:
            return False

    def _reserve_console_login(self, login):
        """Retain the selected dormant owner before validation or eager entry."""
        login.console_login_required = self.console_login_factory is not None
        if not login.console_login_required:
            return
        context = ConsoleConnectLogin(self.principal, login.login_scope_id,
            int(login.expires * 1000), self.origin, lambda: self._login_current(login))
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

    def _queue_controller_review(self, login):
        if not login.console_login_required:
            self._reserve_controller_review(login)
            return
        if self.controller_review_factory is None:
            return
        if self._controller_review_created:
            login.controller_review_failure = "unavailable"
            return
        self._controller_review_created = True
        async def start():
            if await self._ready_console_login(login) and self._login_current(login):
                self._reserve_controller_review(login, reserved=True)
            else:
                login.controller_review_failure = "unavailable"
        login.controller_review_start = asyncio.create_task(start())
        login.controller_review_start.add_done_callback(lambda task: task.exception() if not task.cancelled() else None)

    async def close(self):
        if self._close_task is None:
            self._closing = True
            self.login_tokens.clear()
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
                if login.controller_review is not None and login.controller_review_close is None:
                    login.controller_review_close = asyncio.create_task(login.controller_review.close())
                if login.console_login is not None and login.console_login_close is None:
                    login.console_login_close = asyncio.create_task(self._console_login_cleanup(login.console_login, "close"))
            # Joined close can release an entered startup/presenter while its
            # retirement is pending. Neither is abandoned on a view loss.
            joined = await asyncio.gather(*(task for login in logins for task in
                (login.retirement_task, login.connect_close, login.controller_review_close,
                 login.console_login_close, login.controller_review_start) if task is not None),
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
                raise RuntimeError("Console access cleanup is unconfirmed." if any(login.connect is not None or login.controller_review is not None or login.console_login is not None for login in logins)
                                   else "Console subscription cleanup is unconfirmed.")
            self._logins.clear()
        finally:
            if self.listener:
                self.listener.close()
            if self.client:
                await self.client.aclose()
            self.sessions.clear()

    async def _body(self, request, limit):
        body = bytearray()
        async for chunk in request.stream():
            body.extend(chunk)
            if len(body) > limit:
                raise ValueError("Request too large.")
        return bytes(body)

    def _login_response(self, now, *, error=None, status_code=200):
        from starlette.responses import HTMLResponse, Response
        if len(self.login_tokens) >= 100:
            return Response("Try again shortly.", status_code=429)
        nonce = secrets.token_urlsafe(32)
        self.login_tokens[nonce] = now + 300
        response = HTMLResponse(_login_form(nonce, error), status_code=status_code)
        response.set_cookie(self.cookie + "_login", nonce, httponly=True, samesite="strict", max_age=300)
        response.headers.update({"Cache-Control": "no-store", "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'"})
        return response

    async def handle(self, request):
        from starlette.responses import JSONResponse, RedirectResponse, Response, StreamingResponse
        path = request.url.path
        toolkit_callback_landing = path == "/host/toolkit/setup/callback" and request.method == "GET"
        if (request.headers.get("host") != self.origin.removeprefix("http://")
                or not toolkit_callback_landing and any(key == "auth_token" for key, _ in parse_qsl(request.url.query))):
            return Response("Invalid Console address.", status_code=400)
        if request.headers.get("origin") not in (None, self.origin) and not toolkit_callback_landing:
            return Response("Cross-origin Console requests are refused.", status_code=403)
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
            if request.method == "GET":
                return self._login_response(now)
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
                return self._login_response(now, error="This sign-in form expired. Enter your Console password again.", status_code=403)
            self.failures = [at for at in self.failures if at > now - 60]
            if len(self.failures) >= 10:
                return Response("Too many login attempts. Try again in one minute.", status_code=429)
            if not hmac.compare_digest(hashlib.sha256(form.get("password", "").encode()).digest(), self.password_digest):
                self.failures.append(now)
                return self._login_response(now, error="That password didn't open this Console. Try the Console password you chose during installation.", status_code=401)
            if len(self.sessions) >= 100:
                return Response("Too many active sessions. Restart the application to clear them.", status_code=429)
            session = secrets.token_urlsafe(32)
            self.sessions[session] = now + SESSION_SECONDS
            login = _ConsoleLogin(session, self.sessions[session])
            self._logins[session] = login
            self._reserve_console_login(login)
            self._queue_controller_review(login)
            login.expiry_task = asyncio.create_task(self._expire_login(login))
            response = RedirectResponse("/console", status_code=303)
            response.set_cookie(self.cookie, session, httponly=True, samesite="strict", max_age=SESSION_SECONDS)
            response.delete_cookie(self.cookie + "_login")
            response.headers["Cache-Control"] = "no-store"
            return response
        # Provider navigation can omit the Strict cookie. This fixed landing
        # GET never borrows a viewer; only its later same-origin POST may do so.
        login = None if toolkit_callback_landing else self._live_login(request)
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
                return JSONResponse({"error": "Console login required."}, status_code=401)
        if request.method not in ("GET", "HEAD") and request.headers.get("origin") != self.origin:
            return Response("Console changes require a same-origin authenticated browser.", status_code=403)
        if path == "/logout" and request.method == "POST":
            retirement = self._retire_login(login.session, "logout")
            if retirement is not None:
                try:
                    await asyncio.shield(retirement)
                except Exception:
                    return Response("Console access was revoked but cleanup is unconfirmed.", status_code=503)
            response = RedirectResponse("/login", status_code=303)
            response.delete_cookie(self.cookie)
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
        toolkit_post_limit = None if toolkit_callback_landing else TOOLKIT_SETUP_POST_LIMITS.get(path)
        if toolkit_post_limit is not None:
            if request.method != "POST":
                return Response("Toolkit setup changes require POST.", status_code=405,
                                headers={**_console_public_headers(), "Allow": "GET, POST" if path == "/host/toolkit/setup/callback" else "POST"})
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
        headers = {} if toolkit_callback_landing else {"Authorization": "Bearer " + _token(self.key, self.principal,
                   login_scope_id=login.login_scope_id if is_human else None)}
        if toolkit_post_limit is not None:
            headers["Origin"] = self.origin
        for name in ("content-type", "accept", "last-event-id", "range"):
            if name in request.headers and not toolkit_callback_landing:
                headers[name] = request.headers[name]
        # The landing asset reads its browser location and submits the closed
        # callback envelope. Its GET needs only fixed bytes, never callback data.
        url = self.client.base_url.copy_with(scheme="http", host="127.0.0.1", port=urlsplit(self.native_url).port,
                                              path=path, query=b"" if toolkit_callback_landing else request.url.query.encode())
        subscription = None
        if is_human and request.method == "GET" and path.endswith("/stream"):
            subscription = _ConsoleSubscription(asyncio.current_task())
            login.subscriptions.add(subscription)
        try:
            import httpx
            timeout = httpx.Timeout(None, connect=5, write=60, pool=5) if path.endswith("/stream") else 60
            upstream = await self.client.send(self.client.build_request(request.method, url, content=body, headers=headers,
                                                                         timeout=timeout), stream=True)
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
                    await self._deliver(submission, accepted)
            except Exception:
                # Native may already have admitted the turn. Retrying its same
                # idempotency key lets the application retry its own delivery.
                return JSONResponse({"error": "Native input was admitted but application delivery failed. Retry the same submission."}, status_code=503)
        if is_human and not self._disclosure_current(request, login):
            return JSONResponse({"error": "Console login required."}, status_code=401,
                                headers=_console_public_headers())
        return Response(payload, status_code=upstream.status_code, headers=forwarded)

    def _reserve_controller_review(self, login, *, reserved=False):
        """Trusted login lifecycle hook; no browser route starts native review.

        The application factory supplies a pending exact retained original and
        source on its existing client. A successful login only binds disclosure
        and invocation lifetime; it does not enroll a root or grant permission.
        """
        if self.controller_review_factory is None:
            return
        if (self._controller_review_created and not reserved) or not self._login_current(login):
            login.controller_review_failure = "unavailable"
            return
        self._controller_review_created = True
        try:
            if __package__:
                from .native_controller_review import NativeControllerReview
            else:
                from native_controller_review import NativeControllerReview
            context = ConsoleConnectLogin(self.principal, login.login_scope_id,
                int(login.expires * 1000), self.origin, lambda: self._login_current(login))
            owner = self.controller_review_factory(context)
            if owner is None:
                return
            if (not isinstance(owner, NativeControllerReview)
                    or owner.login_scope_id != context.login_scope_id
                    or owner.expires_at_ms != context.expires_at_ms or owner.origin != context.origin):
                raise ValueError("Invalid protected native review binding.")
            application_current = owner.is_current
            owner.is_current = lambda: self._login_current(login) and application_current() is True
            login.controller_review = self._controller_review_owner = owner
            owner.start()
        except Exception:
            login.controller_review_failure = "invalid_binding"

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
        if login.controller_review_failure:
            return refusal(login.controller_review_failure, 409)
        owner = login.controller_review
        if owner is None:
            return refusal("unavailable", 409)
        if __package__:
            from .native_controller_review import ControllerReviewError
        else:
            from native_controller_review import ControllerReviewError
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
                # Exact login and application guard, then claim, with no await.
                if not self._disclosure_current(request, login):
                    return refusal("invalid_binding", 401)
                owner.claim_answer(payload["approval_id"], payload["decision"])
                value, status = {"status": "answer_submitted"}, 202
            else:
                value = await owner.observe()
                status = 202 if value["status"] == "starting" else 200
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
            context = ConsoleConnectLogin(self.principal, login.login_scope_id,
                int(login.expires * 1000), self.origin, lambda: self._login_current(login))
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
                            or not hmac.compare_digest(approval["proof"], self._frontend_proof(host, payload["proposal"], approval["expires_at"]))):
                        return error(403, "Explicit approval of this exact current proposal is required.")
                    approved = True
                    payload = {"proposal": payload["proposal"],
                               "approval": {"principal": self.principal, "expires_at": approval["expires_at"]}}
            command = FrontendManagementRequest(operation, self.principal, copy.deepcopy(host), copy.deepcopy(payload), approved)
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
                    "proof": self._frontend_proof(host, proposal, expires_at)})
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

    def _frontend_proof(self, host, proposal, expires_at):
        binding = {"scope": "forge-frontend-proposal-v1", "host": host, "principal": self.principal,
                   "proposal": proposal, "expires_at": expires_at}
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

    async def _deliver(self, submission, accepted):
        fields = ("identity", "interaction_id", "session_id", "input_frame_id", "cursor")
        if (not isinstance(submission, dict) or "content" not in submission
                or any(not isinstance(accepted.get(key), str) or not accepted[key] for key in fields)
                or accepted["identity"] != submission.get("identity")):
            raise ValueError("Native admission evidence is incomplete.")
        event = ConsoleInput(channel="host-authenticated-console", principal=self.principal,
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


def _login_form(nonce, error=None):
    notice = f'<p id="login-error" role="alert">{html.escape(error)}</p>' if error else ""
    description = ' aria-describedby="login-error"' if error else ""
    return f'''<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>MobKit Console login</title><style>body{{font:16px system-ui;background:#faf9f6;color:#242522;margin:0;padding:6vh 20px}}main{{max-width:400px;margin:auto}}p{{line-height:1.6}}input,button{{font:inherit;padding:12px;box-sizing:border-box;width:100%;margin:12px 0}}button{{background:#242522;color:white;border:0;border-radius:6px}}#login-error{{padding:12px;background:#fbece5;color:#783d2e;border:1px solid #dab19d;border-radius:6px}}</style></head><body><main><h1>Open your Console</h1><p>Enter the private Console password you chose during installation. Your configured user identity is applied automatically. Your provider API key is separate.</p>{notice}<form method="post" action="/login"><input type="hidden" name="nonce" value="{html.escape(nonce)}"><label for="password">Console password</label><input id="password" type="password" name="password" autocomplete="current-password" required autofocus{description}><button>Open Console</button></form></main></body></html>'''


from contextlib import contextmanager


@contextmanager
def _no_signals():
    yield
