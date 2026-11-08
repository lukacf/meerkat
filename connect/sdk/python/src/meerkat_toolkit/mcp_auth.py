"""Host-only callback transport for the public Meerkat MCP login methods.

This module does not authorize callers, implement OAuth, launch a browser or own
credentials. Its caller must be an authenticated host with a separately protected
presenter. Never expose this API, its client or its private objects as agent tools.
"""
from __future__ import annotations

import asyncio
import hmac
import math
import re
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Protocol
from urllib.parse import parse_qsl, urlsplit


class McpAuthClient(Protocol):
    """Meerkat client methods or the same-runtime private hosted adapter."""

    async def auth_mcp_login_start(self, server_name: str, server_url: str,
                                   redirect_uri: str, *, oauth_account: str | None = None,
                                   oauth_account_selection: str | None = None) -> Any: ...
    async def auth_mcp_login_complete(self, server_name: str, server_url: str, *,
                                      code: str, state: str, redirect_uri: str,
                                      oauth_account: str | None = None,
                                      oauth_account_selection: str | None = None) -> Any: ...
    async def auth_mcp_login_cancel(self, server_name: str, server_url: str,
                                    state: str, *, oauth_account: str | None = None,
                                    oauth_account_selection: str | None = None) -> Any: ...
    async def auth_mcp_status(self, server_name: str, server_url: str, *,
                              oauth_account: str | None = None,
                              oauth_account_selection: str | None = None) -> Any: ...


@dataclass(frozen=True)
class McpTarget:
    server_name: str
    server_url: str = field(repr=False)
    oauth_account: str | None = None
    oauth_account_selection: str | None = None

    def _wire(self) -> dict[str, str]:
        return {"server_name": self.server_name, "server_url": self.server_url, **self._auth_kwargs()}

    def _auth_kwargs(self):
        if self.oauth_account is not None:
            return {"oauth_account": self.oauth_account}
        return {"oauth_account_selection": self.oauth_account_selection}


@dataclass(frozen=True)
class AuthPresentation:
    """Allowlisted labels only. Neither field certifies issuer or account identity."""
    authorization_origin: str
    requested_account_label: str


@dataclass(frozen=True)
class CredentialObservation:
    """Native credential observation, never a connection, grant or task receipt."""
    phase: str
    account_id: str | None
    expires_at: str | None
    scopes: tuple[str, ...] | None = None
    has_refresh_token: bool | None = None
    account_verification: str | None = None


@dataclass(frozen=True)
class CleanupObservation:
    """Local callback retirement and native cancellation are separate facts.

    For a registered HTTPS callback, callback_closed describes this inbox only,
    not the host's shared HTTPS listener, route registration or active requests.
    """
    callback_closed: bool
    native_cancel: str  # acknowledged, unconfirmed, not_requested, not_needed


@dataclass(frozen=True)
class NativeAuthFailure:
    """Core-owned diagnostic codes, not settlement or retry authority.

    Unknown codes remain observable for a matching newer native runtime. The
    transport validates their token shape, never provider error text.
    """
    reason: str
    detail: str | None = None


class McpAuthError(RuntimeError):
    """Fixed, value-free failure. Unconfirmed effects must not be retried automatically."""
    def __init__(self, code: str, cleanup: CleanupObservation | None = None, *,
                 native_failure: NativeAuthFailure | None = None):
        self.code, self.cleanup = code, cleanup
        self.native_failure = native_failure
        super().__init__(code)


class McpAuthCancelled(asyncio.CancelledError):
    def __init__(self, cleanup: CleanupObservation):
        self.cleanup = cleanup
        super().__init__("mcp_auth_cancelled")


def _text(value: Any, limit: int = 8192) -> bool:
    return (isinstance(value, str) and 0 < len(value) <= limit
            and not any(ord(c) < 32 or ord(c) == 127 for c in value))


def _url(value: Any):
    if not _text(value) or "\\" in value or any(c.isspace() for c in value):
        raise ValueError
    parsed = urlsplit(value)
    if (parsed.scheme not in ("http", "https") or not parsed.hostname
            or parsed.username is not None or parsed.password is not None
            or parsed.fragment):
        raise ValueError
    _ = parsed.port
    return parsed


def _origin(value: str) -> str:
    parsed = _url(value)
    host = parsed.hostname.encode("idna").decode("ascii").lower()
    if ":" in host:
        host = f"[{host}]"
    port = parsed.port
    suffix = f":{port}" if port and port != (443 if parsed.scheme == "https" else 80) else ""
    return f"{parsed.scheme}://{host}{suffix}"


def _target(target: McpTarget) -> None:
    if (type(target) is not McpTarget or not _text(target.server_name, 512)
            or not target.server_name.strip()
            or (target.oauth_account is not None) == (target.oauth_account_selection is not None)
            or target.oauth_account is not None and (not _text(target.oauth_account, 4096) or not target.oauth_account.strip())
            or target.oauth_account_selection is not None and target.oauth_account_selection not in ("discover", "unverified")):
        raise ValueError
    _url(target.server_url)


def _matches(value: Any, target: McpTarget) -> bool:
    return isinstance(value, dict) and value == target._wire()


def _observation(value: Any, target: McpTarget, *, completed: bool) -> CredentialObservation:
    if not isinstance(value, dict) or not _matches(value.get("mcp"), target):
        raise ValueError
    account, expires, verification = value.get("account_id"), value.get("expires_at"), value.get("account_verification")
    if (account is not None and not _text(account, 4096)) or (expires is not None and not _text(expires, 128)):
        raise ValueError
    authorized = completed or value.get("phase") == "authorized"
    if target.oauth_account is not None:
        if (account is not None and account != target.oauth_account) or (authorized and account != target.oauth_account):
            raise ValueError
        if verification not in (None, "verified"):
            raise ValueError
    elif target.oauth_account_selection == "discover":
        if verification != "verified" or authorized and account is None:
            raise ValueError
    elif target.oauth_account_selection == "unverified":
        if verification != "unverified" or account is not None:
            raise ValueError
    else:
        raise ValueError
    if completed:
        scopes, refresh = value.get("scopes"), value.get("has_refresh_token")
        if (type(refresh) is not bool
                or not isinstance(scopes, list) or len(scopes) > 256
                or any(not _text(s, 2048) for s in scopes)):
            raise ValueError
        return CredentialObservation("authorized", account, expires, tuple(scopes), refresh, verification)
    phase = value.get("phase")
    if phase not in ("authorized", "reauth_required", "authorization_required"):
        raise ValueError
    return CredentialObservation(phase, account, expires, account_verification=verification)


async def observe_mcp_auth(client: McpAuthClient, target: McpTarget,
                           *, timeout: float = 10) -> CredentialObservation:
    """One explicit native status read. No refresh, polling loop or new attempt."""
    _bounds(timeout)
    failure = "invalid_target"
    cancelled = False
    native_failure = None
    try:
        _target(target)
        failure = "status_unconfirmed"
        value = await asyncio.wait_for(client.auth_mcp_status(
            target.server_name, target.server_url, **target._auth_kwargs()), timeout)
        failure = "invalid_status_response"
        return _observation(value, target, completed=False)
    except asyncio.CancelledError:
        cancelled = True
    except McpAuthError as error:
        if error.native_failure is not None:
            failure, native_failure = "native_auth_failure", error.native_failure
    except Exception:
        pass
    if cancelled:
        raise asyncio.CancelledError("mcp_auth_status_wait_cancelled")
    raise McpAuthError(failure, native_failure=native_failure)


def _bounds(*values: float) -> None:
    if any(type(v) not in (int, float) or not math.isfinite(v) or not 0 < v <= 300
           for v in values):
        raise McpAuthError("invalid_wait_bound")


async def _join(task: asyncio.Task):
    """Join cleanup despite repeated caller cancellation; report cancellation after joining."""
    interrupted = False
    while True:
        try:
            return await asyncio.shield(task), interrupted
        except asyncio.CancelledError:
            if task.cancelled():
                raise McpAuthError("local_cleanup_unconfirmed") from None
            interrupted = True


@dataclass(frozen=True, repr=False)
class _HostedCallbackResult:
    """Private callback fields, retained without inventing a native wire shape."""
    code: str | None
    issuer: str | None


def _receive_query(query, state, callback, *, hosted=False) -> bool:
    """Accept one bounded callback; provider extras never become authority."""
    try:
        if (not isinstance(query, str) or not 0 < len(query) <= 8192
                or not query.isascii() or any(ord(c) < 32 or ord(c) == 127 for c in query)
                or "#" in query or re.search(r"%(?![0-9A-Fa-f]{2})", query)
                or not state or callback is None or callback.done()):
            return False
        pairs = parse_qsl(query, keep_blank_values=True, strict_parsing=True,
                          encoding="utf-8", errors="strict", max_num_fields=32)
        params = dict(pairs)
        if len(params) != len(pairs):
            return False
        received_state = params.get("state", "")
        if (not _text(received_state)
                or not hmac.compare_digest(received_state.encode(), state.encode())):
            return False
        code, error = params.get("code"), params.get("error")
        if (code is None) == (error is None) or not _text(code if code is not None else error):
            return False
        issuer = params.get("iss")
        if hosted and issuer is not None and not _text(issuer):
            return False
        callback.set_result(_HostedCallbackResult(code, issuer) if hosted else code)
        return True
    except (ValueError, UnicodeError):
        return False


class RegisteredHttpsCallback:
    """One private callback inbox for an already admitted host HTTPS route.

    This creates no server, registration, native configuration or authority.
    The protected host owns TLS, exact route/person/offer admission and disposal
    of its route. receive() must run on the custodian's event loop. A true result
    means received locally, not authenticated. No query value is echoed.

    The exact HTTPS URI must have no static query or fragment. Native must
    independently admit it; released loopback-only native clients still refuse.
    One instance can be claimed once. Only the successful claimant retires it.
    """

    __slots__ = ("_target", "_redirect", "_state", "_callback", "_claimed", "_closed")

    def __init__(self, target: McpTarget, redirect_uri: str):
        failure = "invalid_callback_target"
        valid = False
        try:
            _target(target)
            failure = "invalid_callback_uri"
            parsed = _url(redirect_uri)
            if parsed.scheme != "https" or "?" in redirect_uri or "#" in redirect_uri:
                raise ValueError
            valid = True
        except Exception:
            pass
        if not valid:
            raise McpAuthError(failure)
        self._target, self._redirect = target, redirect_uri
        self._state = self._callback = None
        self._claimed = self._closed = False

    @property
    def redirect(self):
        return self._redirect

    @property
    def state(self):
        return self._state

    @property
    def callback(self):
        return self._callback

    def _claim(self, target) -> bool:
        if self._claimed or self._closed or target != self._target:
            return False
        self._callback = asyncio.get_running_loop().create_future()
        self._claimed = True
        return True

    async def bind(self):
        # The host already owns its route; no shared listener is created here.
        pass

    def _arm(self, state):
        self._state = state

    def receive(self, *, redirect_uri: str, query: str) -> bool:
        """Supply an exact-route raw query under captured host admission.

        The URI argument is a consistency check, not evidence of TLS, a trusted
        request path or human identity. Do not derive it from query parameters.
        Early, mismatched, repeated and retired callbacks leave no completion.
        """
        if self._closed or redirect_uri != self._redirect:
            return False
        return _receive_query(query, self._state, self._callback, hosted=True)

    async def close(self):
        """Retire only this inbox. The custodian owns this lifecycle call."""
        self._closed = True
        if self._callback is not None and not self._callback.done():
            self._callback.cancel()
        self._callback = self._state = None


class _Listener:
    def __init__(self):
        self.server: asyncio.Server | None = None
        self.redirect = ""
        self.state: str | None = None
        self.callback = asyncio.get_running_loop().create_future()
        self.tasks: set[asyncio.Task] = set()
        self.writers: set[asyncio.StreamWriter] = set()
        self.closed = False

    async def bind(self):
        self.server = await asyncio.start_server(self._accepted, "127.0.0.1", 0, limit=16384)
        port = self.server.sockets[0].getsockname()[1]
        self.redirect = f"http://127.0.0.1:{port}/callback"

    def _arm(self, state):
        self.state = state

    def _accepted(self, reader, writer):
        task = asyncio.create_task(self._handle(reader, writer))
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)

    async def _handle(self, reader, writer):
        self.writers.add(writer)
        status = "400 Bad Request"
        try:
            if self.closed or len(self.writers) > 4:
                return
            raw = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 3)
            if len(raw) > 16384:
                return
            lines = raw.decode("ascii").split("\r\n")
            method, path, version = lines[0].split(" ")
            headers = []
            for line in lines[1:-2]:
                name, value = line.split(":", 1)
                headers.append((name.lower(), value.strip()))
            if (method != "GET" or version != "HTTP/1.1" or len(path) > 8192
                    or not path.startswith("/callback?") or "#" in path
                    or re.search(r"%(?![0-9A-Fa-f]{2})", path)
                    or [v for k, v in headers if k == "host"] != [urlsplit(self.redirect).netloc]
                    or any(k in ("content-length", "transfer-encoding") for k, _ in headers)):
                raise ValueError
            if not _receive_query(path.partition("?")[2], self.state, self.callback):
                raise ValueError
            # Auxiliary provider query fields are bounded above, never trusted,
            # reflected, logged or forwarded as account/scope/authority facts.
            status = "202 Accepted"
        except (Exception, asyncio.CancelledError):
            pass
        finally:
            try:
                body = b"Sign-in response received. Return to the application." if status.startswith("202") else b"Sign-in response not accepted."
                writer.write((f"HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n"
                              "Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n"
                              "Content-Security-Policy: default-src 'none'\r\nConnection: close\r\n"
                              f"Content-Length: {len(body)}\r\n\r\n").encode() + body)
                await asyncio.wait_for(writer.drain(), 1)
            except (Exception, asyncio.CancelledError):
                pass
            writer.close()
            try:
                await asyncio.wait_for(writer.wait_closed(), 1)
            except (Exception, asyncio.CancelledError):
                pass
            self.writers.discard(writer)

    async def close(self):
        self.closed = True
        if self.server:
            self.server.close()
            await self.server.wait_closed()
        for writer in tuple(self.writers):
            writer.close()
        tasks = tuple(self.tasks)
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        if self.callback is not None and not self.callback.done():
            self.callback.cancel()
        self.callback = None
        self.state = None


@dataclass(frozen=True)
class JoinedMcpLogin:
    """Another custodian owns the attempt. This object has no cancel or presenter."""
    presentation: AuthPresentation


@dataclass(frozen=True)
class _Outcome:
    observation: CredentialObservation | None
    error: str | None
    cleanup: CleanupObservation
    native_failure: NativeAuthFailure | None = None


class StartedMcpLogin:
    """A host-owned callback transport, independent of UI panels and waiters.

    Keep this handle until wait/close settles. Cancelling wait() leaves the
    ceremony running; only its host custodian may explicitly call close().
    """
    def __init__(self, client, target, listener, authorize_url, presentation,
                 callback_timeout, rpc_timeout):
        self.presentation = presentation
        self._client, self._target, self._listener = client, target, listener
        self._authorize_url = authorize_url
        self._rpc_timeout = rpc_timeout
        self._close_task = None
        self._running = asyncio.Event()
        self._run_task = asyncio.create_task(self._run(callback_timeout))

    async def present(self, presenter: Callable[[str], Awaitable[None]]) -> bool:
        """Invoke the host's protected presenter. True proves only callback return.

        No browser implementation or isolation guarantee is supplied here. A
        failed/cancelled presentation does not cancel or restart native auth.
        """
        if self._run_task.done() or self._close_task is not None or not self._authorize_url:
            raise McpAuthError("presentation_no_longer_available")
        try:
            await presenter(self._authorize_url)
            return True
        except asyncio.CancelledError:
            pass
        except Exception:
            return False
        raise asyncio.CancelledError("mcp_auth_presentation_wait_cancelled")

    async def wait(self) -> CredentialObservation:
        outcome = await asyncio.shield(self._run_task)
        if outcome.error:
            raise McpAuthError(outcome.error, outcome.cleanup, native_failure=outcome.native_failure)
        return outcome.observation

    async def close(self) -> CleanupObservation:
        if self._close_task is None:
            self._close_task = asyncio.create_task(self._close())
        outcome, interrupted = await _join(self._close_task)
        if interrupted:
            raise McpAuthCancelled(outcome.cleanup)
        return outcome.cleanup

    async def _close(self):
        await self._running.wait()
        if not self._run_task.done():
            self._run_task.cancel()
        outcome, _ = await _join(self._run_task)
        return outcome

    async def _run(self, callback_timeout):
        self._running.set()
        observation, error = None, None
        native_failure = None
        state = self._listener.state
        try:
            callback = await asyncio.wait_for(asyncio.shield(self._listener.callback), callback_timeout)
            issuer = callback.issuer if isinstance(callback, _HostedCallbackResult) else None
            code = callback.code if isinstance(callback, _HostedCallbackResult) else callback
            registered = getattr(self._client, "auth_mcp_login_complete_registered", None)
            if code is not None and issuer is not None and not callable(registered):
                # The owner's hosted completion contract is not bound yet. Keep
                # issuer bytes intact and refuse instead of silently dropping
                # RFC 9207 evidence into the old loopback-only client call.
                error = "hosted_issuer_binding_unavailable"
            elif code is None:
                error = "authorization_denied"
            else:
                error = "completion_uncertain"
                args = dict(code=code, state=state, redirect_uri=self._listener.redirect,
                    **self._target._auth_kwargs())
                complete = self._client.auth_mcp_login_complete
                if isinstance(callback, _HostedCallbackResult) and callable(registered):
                    complete = registered
                    args["iss"] = issuer
                value = await asyncio.wait_for(complete(
                    self._target.server_name, self._target.server_url, **args), self._rpc_timeout)
                error = "invalid_completion_response"
                observation = _observation(value, self._target, completed=True)
                error = None
        except asyncio.TimeoutError:
            error = error or "callback_wait_abandoned"
        except asyncio.CancelledError:
            error = error or "custodian_closed_uncertain"
        except McpAuthError as failure:
            if failure.native_failure is not None:
                error, native_failure = "native_auth_failure", failure.native_failure
            else:
                error = error or "callback_unconfirmed"
        except Exception:
            error = error or "callback_unconfirmed"
        cleanup, _ = await _join(asyncio.create_task(self._cleanup(state, observation is None)))
        self._authorize_url = None
        return _Outcome(observation, error, cleanup, native_failure)

    async def _cleanup(self, state, cancel):
        await self._listener.close()
        disposition = "not_needed"
        if cancel:
            disposition = "unconfirmed"
            try:
                value = await asyncio.wait_for(self._client.auth_mcp_login_cancel(
                    self._target.server_name, self._target.server_url, state,
                    **self._target._auth_kwargs()), self._rpc_timeout)
                if (isinstance(value, dict) and _matches(value.get("mcp"), self._target)
                        and value.get("cancelled") is True):
                    disposition = "acknowledged"
            except (Exception, asyncio.CancelledError):
                pass
        return CleanupObservation(True, disposition)


async def begin_mcp_login(client: McpAuthClient, target: McpTarget, *,
                          callback_timeout: float = 240,
                          rpc_timeout: float = 10,
                          callback: RegisteredHttpsCallback | None = None,
                          ) -> StartedMcpLogin | JoinedMcpLogin:
    """Bind before native start. This is privileged host code, never a tool call.

    Bounds are host I/O budgets, not native expiry or permission to retry. An
    ambiguous start cannot be cancelled without a verified native response.
    """
    _bounds(callback_timeout, rpc_timeout)
    listener, failure, cancelled = None, "invalid_target", False
    native_failure = None
    try:
        _target(target)
        if callback is None:
            listener = _Listener()
        else:
            failure = "callback_unavailable"
            # Do not assign listener until this call owns it. A losing caller
            # must never close the winning custodian's inbox during cleanup.
            if type(callback) is not RegisteredHttpsCallback or not callback._claim(target):
                raise ValueError
            listener = callback
        failure = "unsupported_client"
        if any(not callable(getattr(client, name, None)) for name in (
                "auth_mcp_login_start", "auth_mcp_login_complete",
                "auth_mcp_login_cancel", "auth_mcp_status")):
            raise ValueError
        failure = "callback_bind_failed"
        await listener.bind()
        failure = "start_uncertain"
        value = await asyncio.wait_for(client.auth_mcp_login_start(
            target.server_name, target.server_url, listener.redirect,
            **target._auth_kwargs()), rpc_timeout)
        failure = "invalid_start_response_uncertain"
        if not isinstance(value, dict) or not _matches(value.get("mcp"), target):
            raise ValueError
        state, url, redirect = value.get("state"), value.get("authorize_url"), value.get("redirect_uri")
        if not _text(state) or not _text(url):
            raise ValueError
        presentation = AuthPresentation(_origin(url), target.oauth_account or
            ("Discover account" if target.oauth_account_selection == "discover" else "Unverified resource access"))
        if isinstance(listener, RegisteredHttpsCallback):
            if redirect != listener.redirect:
                raise ValueError
        else:
            parsed = _url(redirect)
            if (parsed.scheme != "http" or parsed.hostname not in ("127.0.0.1", "::1")
                    or not parsed.port):
                raise ValueError
        if value.get("disposition") == "joined":
            _, interrupted = await _join(asyncio.create_task(listener.close()))
            if interrupted:
                raise McpAuthCancelled(CleanupObservation(True, "not_requested"))
            return JoinedMcpLogin(presentation)
        if value.get("disposition") != "started" or redirect != listener.redirect:
            raise ValueError
        listener._arm(state)
        return StartedMcpLogin(client, target, listener, url, presentation,
                               callback_timeout, rpc_timeout)
    except asyncio.CancelledError:
        cancelled = True
    except McpAuthError as error:
        if error.native_failure is not None:
            failure, native_failure = "native_auth_failure", error.native_failure
    except Exception:
        pass
    interrupted = False
    if listener is not None:
        _, interrupted = await _join(asyncio.create_task(listener.close()))
    cleanup = CleanupObservation(listener is not None or callback is None, "not_requested")
    if cancelled or interrupted:
        raise McpAuthCancelled(cleanup)
    raise McpAuthError(failure, cleanup, native_failure=native_failure)


@dataclass(frozen=True, repr=False)
class _CustodianResult:
    handle: StartedMcpLogin | JoinedMcpLogin | None = None
    cleanup: CleanupObservation | None = None
    error: str | None = None
    cancelled: bool = False
    native_failure: NativeAuthFailure | None = None

    def raise_error(self) -> None:
        # Cache values, not exception objects with retained or growing tracebacks.
        if self.cancelled:
            raise McpAuthCancelled(self.cleanup) from None
        if self.error is not None:
            raise McpAuthError(self.error, self.cleanup, native_failure=self.native_failure) from None


class McpLoginCustodian:
    """Retain one host step's login transport across request cancellation.

    Construct and retain this object under an existing authenticated host step
    before awaiting begin(). The host admits every invocation, including replay,
    and owns the protected presenter. This object supplies no step identity,
    permissions, browser, credential storage, retry policy or native lifecycle.

    The client, exact target and I/O bounds are fixed for this custodian. begin()
    retains one start result, including uncertainty; it never retries. Waiter
    cancellation does not abandon that result. Presentation and credential
    observation remain on the existing StartedMcpLogin handle.

    Explicit close() prevents further begin() results, joins an admitted pending
    start, and closes only a StartedMcpLogin owned here. A begin() already waiting
    when close wins raises custodian_closed instead of handing out its handle.
    The host must retain this object until close settles, including on shutdown.
    This is in-memory transport custody, not recovery across process loss.
    """

    __slots__ = ("_client", "_target", "_callback_timeout", "_rpc_timeout", "_callback",
                 "_begin_task", "_close_task", "_closed")

    def __init__(self, client: McpAuthClient, target: McpTarget, *,
                 callback_timeout: float = 240, rpc_timeout: float = 10,
                 callback: RegisteredHttpsCallback | None = None):
        _bounds(callback_timeout, rpc_timeout)
        invalid_target = False
        try:
            _target(target)
        except Exception:
            invalid_target = True
        if invalid_target:
            raise McpAuthError("invalid_target")
        self._client, self._target = client, target
        self._callback = callback
        self._callback_timeout, self._rpc_timeout = callback_timeout, rpc_timeout
        self._begin_task: asyncio.Task[_CustodianResult] | None = None
        self._close_task: asyncio.Task[_CustodianResult] | None = None
        self._closed = False

    async def begin(self) -> StartedMcpLogin | JoinedMcpLogin:
        """Retrieve the single start result without cancelling it with a waiter."""
        if self._closed:
            raise McpAuthError("custodian_closed")
        if self._begin_task is None:
            self._begin_task = asyncio.create_task(self._begin())
        cancelled = False
        try:
            result = await asyncio.shield(self._begin_task)
        except asyncio.CancelledError:
            cancelled = True
        if cancelled:
            raise asyncio.CancelledError("mcp_auth_start_wait_cancelled")
        if self._closed:
            raise McpAuthError("custodian_closed")
        result.raise_error()
        return result.handle

    async def _begin(self) -> _CustodianResult:
        try:
            handle = await begin_mcp_login(
                self._client, self._target, callback_timeout=self._callback_timeout,
                rpc_timeout=self._rpc_timeout, callback=self._callback)
            return _CustodianResult(handle=handle)
        except McpAuthCancelled as error:
            return _CustodianResult(cleanup=error.cleanup, cancelled=True)
        except McpAuthError as error:
            return _CustodianResult(cleanup=error.cleanup, error=error.code,
                                    native_failure=error.native_failure)
        except asyncio.CancelledError:
            return _CustodianResult(cleanup=CleanupObservation(False, "not_requested"),
                                    cancelled=True)
        except Exception:
            return _CustodianResult(cleanup=CleanupObservation(False, "not_requested"),
                                    error="start_uncertain")

    async def close(self) -> CleanupObservation:
        """Join explicit host shutdown; request cancellation is reported afterward.

        A failed start retains its original error and cleanup observation. Closing
        it does not guess native state or turn uncertainty into successful cleanup.
        """
        self._closed = True
        if self._close_task is None:
            self._close_task = asyncio.create_task(self._close())
        result, interrupted = await _join(self._close_task)
        if interrupted:
            raise McpAuthCancelled(result.cleanup) from None
        result.raise_error()
        return result.cleanup

    async def _close(self) -> _CustodianResult:
        try:
            if self._begin_task is None:
                return _CustodianResult(cleanup=CleanupObservation(
                    self._callback is None, "not_requested"))
            result = await asyncio.shield(self._begin_task)
            if result.error is not None or result.cancelled:
                return result
            if isinstance(result.handle, JoinedMcpLogin):
                return _CustodianResult(cleanup=CleanupObservation(True, "not_requested"))
            return _CustodianResult(cleanup=await result.handle.close())
        except McpAuthCancelled as error:
            return _CustodianResult(cleanup=error.cleanup, cancelled=True)
        except McpAuthError as error:
            return _CustodianResult(cleanup=error.cleanup, error=error.code,
                                    native_failure=error.native_failure)
        except asyncio.CancelledError:
            return _CustodianResult(cleanup=CleanupObservation(False, "unconfirmed"),
                                    cancelled=True)
        except Exception:
            return _CustodianResult(cleanup=CleanupObservation(False, "unconfirmed"),
                                    error="custodian_cleanup_unconfirmed")
