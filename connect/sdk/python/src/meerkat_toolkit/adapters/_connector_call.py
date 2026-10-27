"""Private credential consumption for one native-owned MCP tools/call.

This module grants no account, scope, freshness or operation authority. Native
admission owns those decisions. Only the explicit serve-native child uses it.
"""

from __future__ import annotations

import asyncio
from contextlib import contextmanager
from dataclasses import replace
import json
import logging
import os
from types import MappingProxyType
from urllib.parse import urlsplit


KEY = "meerkat/connector_credential"
BINDING_ENV = "MEERKAT_CONNECTOR_BINDING"


class CredentialRefused(ValueError):
    def __init__(self):
        super().__init__("Connector credential refused")


def _origin(value):
    if (type(value) is not str or not value or len(value) > 2048
            or any(ord(c) <= 32 or ord(c) >= 127 for c in value) or "\\" in value):
        raise CredentialRefused()
    parsed = urlsplit(value)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username is not None
            or parsed.password is not None or "%" in parsed.netloc):
        raise CredentialRefused()
    port = parsed.port
    if port == 0:
        raise CredentialRefused()
    return parsed, (parsed.scheme, parsed.hostname, 443 if port is None else port)


def _closed(value, required, optional=()):
    if type(value) is not dict or set(value) - set(required) - set(optional) or set(required) - set(value):
        raise ValueError()


def _account(value):
    fields = ("issuer", "strategy_id", "subject")
    _closed(value, fields)
    if any(type(value[field]) is not str for field in fields):
        raise ValueError()
    # These are opaque native identity strings, not URLs or profile names.
    return tuple(value[field] for field in fields)


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError()
        value[key] = item
    return value


class ConnectorBinding:
    """One trusted startup expectation, independent of mutable call input."""

    __slots__ = ("_operations",)

    def __init__(self, encoded):
        failed = False
        try:
            if type(encoded) is not str:
                raise ValueError()
            value = json.loads(encoded, object_pairs_hook=_unique_object)
            _closed(value, ("version", "operations"))
            if type(value["version"]) is not int or value["version"] != 1 or type(value["operations"]) is not dict:
                raise ValueError()
            operations = {}
            for name, item in value["operations"].items():
                if type(name) is not str:
                    raise ValueError()
                _closed(item, ("account", "destination_origin"))
                account = _account(item["account"])
                destination, _ = _origin(item["destination_origin"])
                if destination.path not in ("", "/") or destination.query or destination.fragment:
                    raise ValueError()
                operations[name] = (account, item["destination_origin"])
            self._operations = MappingProxyType(operations)
        except Exception:
            failed = True
        if failed:
            raise CredentialRefused()

    @classmethod
    def from_environment(cls):
        # Called once by native server construction, never from a tool call.
        return cls(os.environ.get(BINDING_ENV))

    def __repr__(self):
        return "<private connector startup binding>"


class ConnectorCall:
    """Private per-call material, never a durable slot or a native outcome."""

    __slots__ = ("_authorization", "_origin", "request_entered")

    def __init__(self, context, binding, operation, expected_origin):
        failed = False
        try:
            meta = context.meta
            value = meta.get(KEY) if type(meta) is dict else None
            _closed(value, ("authorization", "destination_origin", "account", "scopes"), ("expires_at",))
            if (type(binding) is not ConnectorBinding or type(operation) is not str
                    or operation not in binding._operations):
                raise ValueError()
            account, declared_origin = binding._operations[operation]
            if _account(value["account"]) != account or value["destination_origin"] != declared_origin:
                raise ValueError()
            if (type(value["scopes"]) is not list or any(type(scope) is not str for scope in value["scopes"])
                    or (value.get("expires_at") is not None and type(value["expires_at"]) is not str)):
                raise ValueError()
            authorization = value.get("authorization")
            if (type(authorization) is not str or not authorization.startswith("Bearer ")
                    or not 8 <= len(authorization) <= 65536
                    or any(not 33 <= ord(c) <= 126 for c in authorization[7:])):
                raise ValueError()
            destination, origin = _origin(value.get("destination_origin"))
            _, expected = _origin(expected_origin)
            if (destination.path not in ("", "/") or destination.query
                    or destination.fragment or origin != expected):
                raise ValueError()
            self._authorization, self._origin = authorization, origin
            self.request_entered = False
            # Account equality correlates the native installation and call.
            # Scopes/expiry are shape-checked only, never permission/freshness.
        except Exception:
            failed = True
        if failed:
            raise CredentialRefused()

    def __repr__(self):
        return "<private connector call>"

    def check_url(self, value):
        failed = False
        try:
            parsed, origin = _origin(str(value))
            if origin != self._origin or parsed.fragment:
                raise ValueError()
        except Exception:
            failed = True
        if failed:
            raise CredentialRefused()

    def httpx_client(self, base_url):
        import httpx
        from ..httpx_client import prepare_httpx_client

        call = self

        class Auth(httpx.Auth):
            def auth_flow(self, request):
                call.check_url(request.url)
                request.headers["Authorization"] = call._authorization
                yield request

        class Transport(httpx.AsyncHTTPTransport):
            async def handle_async_request(self, request):
                # This actual request owner checks before delegating to the
                # provider transport. Entry only moves from false to true.
                call.check_url(request.url)
                call.request_entered = True
                return await super().handle_async_request(request)

        client = httpx.AsyncClient(base_url=base_url, auth=Auth(), trust_env=False,
            follow_redirects=False, timeout=None, transport=Transport(retries=0, trust_env=False))
        return prepare_httpx_client(client)

    def requests_session(self):
        import requests
        from .agno_transports import ServiceSession

        call = self

        class Auth(requests.auth.AuthBase):
            def __call__(self, request):
                call.check_url(request.url)
                request.headers["Authorization"] = call._authorization
                return request

        class Adapter(requests.adapters.HTTPAdapter):
            def send(self, request, **kwargs):
                call.check_url(request.url)
                call.request_entered = True
                return super().send(request, **kwargs)

        session = ServiceSession()
        try:
            session.trust_env, session.max_redirects = False, 0
            session.auth = Auth()
            previous = session.adapters["https://"]
            session.mount("https://", Adapter(max_retries=0))
            previous.close()
            return session
        except BaseException:
            session.close()
            raise


async def _private_middleware(context, call_next):
    """Keep private metadata away from typed params and fixed error output."""
    failed = False
    try:
        params = context.params
        if type(params) is dict and "_meta" in params:
            params = dict(params)
            meta = params.get("_meta")
            if type(meta) is dict:
                params["_meta"] = {key: value for key, value in meta.items() if key != KEY}
            context = replace(context, params=params)
        return await call_next(context)
    except asyncio.CancelledError:
        raise
    except Exception:
        failed = True
    if failed and context.request_id is not None:
        from mcp.shared.exceptions import MCPError
        raise MCPError(code=-32602, message="Connector request refused")
    return None


def protect_server(server):
    # This explicitly private worker opts out of the SDK's default OTel
    # middleware. Its generic exception recorder must not see credentials.
    server.middleware[:] = [_private_middleware]
    return server


@contextmanager
def private_diagnostics():
    """Fixed messages on the exact SDK channels that print raw input/errors.

    The scope starts before stdio decoding and ends after Server.run joins.
    It preserves log levels and stream/runtime ownership, including the SDK's
    pre-first-request replay. No third-party logger or global level is changed.
    """
    class Fixed(logging.Filter):
        def filter(self, record):
            record.msg, record.args = "Private connector MCP diagnostic", ()
            record.exc_info = record.exc_text = record.stack_info = None
            return True

    filters = []
    try:
        for name in ("mcp.server.runner", "mcp.shared.jsonrpc_dispatcher"):
            logger, fixed = logging.getLogger(name), Fixed()
            logger.addFilter(fixed)
            filters.append((logger, fixed))
        yield
    finally:
        for logger, fixed in filters:
            logger.removeFilter(fixed)
