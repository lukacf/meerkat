"""Trusted Forge composition of original native login and OAuth custody.

The installation owner supplies a real PrivateHostClient and its authenticated
ConsoleConnectLogin, and registers close() with that login's retirement. No
HTTP field constructs these objects, a native caller, or an account target.
"""
from __future__ import annotations

import asyncio
import inspect
import json
import time
from urllib.parse import parse_qsl, urlsplit

from .service_callback import MCP_CALLBACK_PATH


class ServiceLoginUnavailable(RuntimeError):
    def __init__(self):
        super().__init__("The protected service login is unavailable.")


def _retain(task):
    task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
    return task


def _unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError()
        value[key] = item
    return value


class ForgeServiceLoginHost:
    """One original authenticated login, with one retained attempt per target.

    All public calls belong to the owning event loop. Register close() with the
    actual host's login lifecycle before calling start() or offering actions.
    Caller cancellation only stops waiting; close() fences first, then joins
    original ceremony cleanup and native enrollment retirement.
    """
    def __init__(self, client, login, *, issuer, redirect_uri, targets):
        from .templates_v2.native_console_login import NativeConsoleLogin
        from meerkat_toolkit.mcp_auth import McpTarget, RegisteredHttpsCallback

        parsed = urlsplit(redirect_uri)
        if (parsed.scheme != "https" or parsed.path != MCP_CALLBACK_PATH
                or parsed.query or parsed.fragment or parsed.username or parsed.password
                or login.origin != f"https://{parsed.netloc}"
                or type(targets) is not dict or not targets):
            raise ServiceLoginUnavailable()
        for key, target in targets.items():
            if not isinstance(key, str) or not key or not isinstance(target, McpTarget):
                raise ServiceLoginUnavailable()
            RegisteredHttpsCallback(target, redirect_uri)  # Existing SDK validation, no I/O.
        self._native_login = NativeConsoleLogin(client, login, issuer=issuer)
        self._client, self._context = client, login
        self._origin, self._redirect = login.origin, redirect_uri
        self._targets = dict(targets)
        self._attempts, self._states = {}, {}
        self._loop = None
        self._closed = False
        self._close_task = None
        self._enrollment_ready = False
        self._retired = False

    def __repr__(self):
        return "ForgeServiceLoginHost(<host-private>)"

    @property
    def retired(self):
        return self._retired

    def start(self):
        if self._loop is not None or self._closed:
            raise ServiceLoginUnavailable()
        self._loop = asyncio.get_running_loop()
        self._native_login.start()

    def _admit(self, context):
        if (self._closed or self._loop is not asyncio.get_running_loop()
                or context is not self._context or not self._native_login.is_current()
                or context.origin != self._origin
                or time.time() * 1000 >= context.expires_at_ms):
            raise ServiceLoginUnavailable()

    def revoke(self):
        """The actual host invokes this synchronously at logout/expiry."""
        self._closed = True
        self._states.clear()
        self._native_login.revoke()

    async def begin(self, context, service_id):
        """Host-selected target ID only; retain before entering native work."""
        from meerkat_toolkit.mcp_auth import StartedMcpLogin

        self._admit(context)
        if type(service_id) is not str or service_id not in self._targets:
            raise ServiceLoginUnavailable()
        attempt = self._attempts.get(service_id)
        if attempt is None:
            attempt = {"client": None, "custodian": None, "callback": None, "handle": None,
                       "target": self._targets[service_id]}
            self._attempts[service_id] = attempt
            attempt["task"] = _retain(asyncio.create_task(self._begin(attempt, self._targets[service_id])))
        try:
            await asyncio.shield(attempt["task"])
        except asyncio.CancelledError:
            raise
        except Exception:
            raise ServiceLoginUnavailable() from None
        self._admit(context)
        return "started" if isinstance(attempt["handle"], StartedMcpLogin) else "joined"

    async def _begin(self, attempt, target):
        from meerkat_toolkit.mcp_auth import McpLoginCustodian, RegisteredHttpsCallback, StartedMcpLogin
        from meerkat_toolkit.mcp_auth_native import NativeHostedMcpClient

        enrollment = await self._native_login.ready()
        self._enrollment_ready = True
        self._admit(self._context)
        native = await NativeHostedMcpClient.for_target(self._client, enrollment, target)
        attempt["client"] = native  # Retain late native results even after logout.
        self._admit(self._context)
        if native.redirect_uri != self._redirect:
            raise ServiceLoginUnavailable()
        callback = RegisteredHttpsCallback(target, self._redirect)
        attempt["callback"] = callback
        custodian = McpLoginCustodian(native, target, callback=callback)
        attempt["custodian"] = custodian
        handle = await custodian.begin()
        attempt["handle"] = handle
        self._admit(self._context)
        if isinstance(handle, StartedMcpLogin):
            state = callback.state
            if not isinstance(state, str) or not state or state in self._states:
                # Ambiguous state cannot choose either original attempt.
                self.revoke()
                self._start_close()
                raise ServiceLoginUnavailable()
            self._states[state] = attempt

    async def present(self, context, service_id, protected_presenter):
        from meerkat_toolkit.mcp_auth import StartedMcpLogin

        self._admit(context)
        attempt = self._attempts.get(service_id)
        if (attempt is None or not isinstance(attempt["handle"], StartedMcpLogin)
                or not callable(protected_presenter)):
            raise ServiceLoginUnavailable()
        result = await attempt["handle"].present(protected_presenter)
        self._admit(context)
        return result

    async def observe(self, context, service_id):
        from meerkat_toolkit.mcp_auth import observe_mcp_auth

        self._admit(context)
        attempt = self._attempts.get(service_id)
        if attempt is None or attempt["client"] is None:
            raise ServiceLoginUnavailable()
        result = await observe_mcp_auth(attempt["client"], self._targets[service_id])
        self._admit(context)
        return result  # Native credential observation, never required-operation success.

    async def wait(self, context, service_id):
        from meerkat_toolkit.mcp_auth import StartedMcpLogin

        self._admit(context)
        attempt = self._attempts.get(service_id)
        if attempt is None or not isinstance(attempt["handle"], StartedMcpLogin):
            raise ServiceLoginUnavailable()
        result = await attempt["handle"].wait()
        self._admit(context)
        return result

    def receive(self, context, *, origin, body):
        """Route raw query only within this login; SDK validates the whole query."""
        try:
            self._admit(context)
        except ServiceLoginUnavailable:
            return 403
        if origin != self._origin:
            return 403
        try:
            if type(body) is not bytes or not 0 < len(body) <= 65536:
                return 400
            value = json.loads(body.decode("utf-8"), object_pairs_hook=_unique)
            if type(value) is not dict or set(value) != {"query"}:
                return 400
            query = value["query"]
            if type(query) is not str or not 0 < len(query) <= 8192 or not query.isascii():
                return 400
            pairs = parse_qsl(query, keep_blank_values=True, max_num_fields=32,
                              encoding="utf-8", errors="strict")
            states = [v for k, v in pairs if k == "state"]
            if len(states) != 1:
                return 400
            attempt = self._states.get(states[0])
            if attempt is None:
                return 400
            accepted = attempt["callback"].receive(redirect_uri=self._redirect, query=query)
            return 202 if accepted else 400
        except (ValueError, UnicodeError, RecursionError):
            return 400

    def _start_close(self):
        if self._close_task is None:
            self._close_task = _retain(asyncio.create_task(self._close()))

    async def close(self):
        self.revoke()
        self._start_close()
        interrupted = False
        while not self._close_task.done():
            try:
                await asyncio.shield(self._close_task)
            except asyncio.CancelledError:
                interrupted = True
            except Exception:
                break
        self._close_task.result()
        if interrupted:
            raise asyncio.CancelledError("Service login cleanup joined.")

    async def _close_attempt(self, attempt):
        # Entered describe/start must settle before its original owners retire.
        await asyncio.gather(attempt["task"], return_exceptions=True)
        confirmed = True
        if attempt["custodian"] is not None:
            try:
                cleanup = await attempt["custodian"].close()
                confirmed = cleanup.callback_closed and cleanup.native_cancel != "unconfirmed"
            except BaseException:
                confirmed = False
        if attempt["client"] is not None:
            try:
                result = await attempt["client"].close_owned_hosted_login()
                target = attempt["target"]
                expected = {"server_name": target.server_name, "server_url": target.server_url}
                if target.oauth_account is not None:
                    expected["oauth_account"] = target.oauth_account
                else:
                    expected["oauth_account_selection"] = target.oauth_account_selection
                if (type(result) is not dict or result.get("mcp") != expected
                        or not (result.get("cancelled") is True
                                or result.get("cancelled") is False and (
                                    result.get("completed") is True
                                    or set(result) == {"mcp", "cancelled"}))):
                    confirmed = False
            except BaseException:
                confirmed = False
        return confirmed

    async def _close(self):
        # A waiting enrollment result cannot delay its own retirement. No
        # ceremony has entered before ready() succeeds, and revoke() prevents
        # a late result from entering one. Otherwise, retire owned ceremonies
        # before retiring their acknowledged enrollment.
        retired = False
        if not self._enrollment_ready:
            try:
                await self._native_login.close()
                retired = True
            except BaseException:
                pass
        results = await asyncio.gather(*(self._close_attempt(a) for a in self._attempts.values()),
                                       return_exceptions=True)
        if self._enrollment_ready:
            try:
                await self._native_login.close()
                retired = True
            except BaseException:
                pass
        if not retired or any(value is not True for value in results):
            raise ServiceLoginUnavailable()
        self._retired = True


class ServiceLoginRoute:
    """Route original sessions through one trusted request-to-context join.

    The installation owner supplies every host and the same synchronous
    resolver. Session labels and callback state never select a login owner.
    Retired owners remain retained for this route's lifetime, preventing a
    second enrollment of the same original native login scope.
    """
    def __init__(self, host, authenticated_context):
        if (not callable(authenticated_context)
                or inspect.iscoroutinefunction(authenticated_context)):
            raise ServiceLoginUnavailable()
        self.authenticated_context = authenticated_context
        self._loop = asyncio.get_running_loop()
        self._hosts = {}
        self._closed = False
        self._shutdown_task = None
        self.add(host)

    @property
    def retired(self):
        return (self._closed and self._shutdown_task is not None
                and self._shutdown_task.done() and not self._shutdown_task.cancelled()
                and self._shutdown_task.exception() is None
                and all(host.retired for host in self._hosts.values()))

    def add(self, host):
        """Retain before start; only the trusted launcher can register owners."""
        if (self._closed or self._loop is not asyncio.get_running_loop()
                or type(host) is not ForgeServiceLoginHost
                or host._loop is not None or host._closed):
            raise ServiceLoginUnavailable()
        binding = host._native_login
        for existing in self._hosts.values():
            other = existing._native_login
            if (host._context is existing._context or
                    (binding.issuer, binding.subject, binding.login_scope_id) ==
                    (other.issuer, other.subject, other.login_scope_id)):
                raise ServiceLoginUnavailable()
        self._hosts[id(host._context)] = host

    def _host(self, context):
        if self._loop is not asyncio.get_running_loop():
            raise ServiceLoginUnavailable()
        host = self._hosts.get(id(context))
        if host is None or context is not host._context:
            raise ServiceLoginUnavailable()
        return host

    def revoke(self, context):
        """Fence only this original session at actual logout or expiry."""
        self._host(context).revoke()

    async def retire(self, context):
        """Join the original session's cleanup without replacing its owner."""
        await self._host(context).close()

    async def post(self, request):
        from starlette.responses import Response
        from .service_callback import callback_unavailable

        response = callback_unavailable()
        try:
            if self._closed:
                raise ServiceLoginUnavailable()
            context = self.authenticated_context(request)
            if inspect.isawaitable(context):
                if inspect.iscoroutine(context):
                    context.close()
                raise ServiceLoginUnavailable()
            host = self._host(context)
            host._admit(context)
            origins = request.headers.getlist("origin")
            if origins != [host._origin]:
                raise ServiceLoginUnavailable()
        except Exception:
            response.status_code = 403
            return response
        if (request.scope.get("query_string") or request.headers.getlist("content-type") != ["application/json"]):
            response.status_code = 400
            return response
        body = bytearray()
        try:
            async for part in request.stream():
                if len(body) + len(part) > 65536:
                    response.status_code = 413
                    return response
                body.extend(part)
        except Exception:
            response.status_code = 400
            return response
        status = host.receive(context, origin=origins[0], body=bytes(body))
        return Response("Callback received." if status == 202 else "The callback could not be confirmed.",
                        status_code=status, media_type="text/plain",
                        headers={k: v for k, v in response.headers.items()
                                 if k not in ("content-length", "content-type")})

    async def shutdown(self):
        if self._loop is not asyncio.get_running_loop():
            raise ServiceLoginUnavailable()
        self._closed = True
        # Fence every session before waiting on any one native retirement.
        for host in self._hosts.values():
            host.revoke()
        if self._shutdown_task is None:
            self._shutdown_task = _retain(asyncio.create_task(self._shutdown()))
        interrupted = False
        while not self._shutdown_task.done():
            try:
                await asyncio.shield(self._shutdown_task)
            except asyncio.CancelledError:
                interrupted = True
            except Exception:
                break
        self._shutdown_task.result()
        if interrupted:
            raise asyncio.CancelledError("Service login cleanup joined.")

    async def _shutdown(self):
        results = await asyncio.gather(*(host.close() for host in self._hosts.values()),
                                       return_exceptions=True)
        if any(isinstance(result, BaseException) for result in results):
            raise ServiceLoginUnavailable()
