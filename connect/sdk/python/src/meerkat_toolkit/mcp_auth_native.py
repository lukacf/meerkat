"""Private callback transport to the application's existing native runtime.

The host supplies its acknowledged Console enrollment, exact configured target
and registered redirect. No public RPC process, OAuth implementation or token
store is created. Native retains the Started-only secret completion handle.
"""
from __future__ import annotations

import asyncio
import hmac
import re
from typing import Any

from .host_control import ConsoleLoginEnrollment, PrivateHostClient
from .mcp_auth import McpAuthError, McpTarget, NativeAuthFailure, _target, _text, _url


def _native_failure(value, target):
    if not isinstance(value, dict) or "auth_error" not in value:
        return None
    if set(value) != {"mcp", "auth_error"} or value["mcp"] != target._wire():
        raise ValueError
    error = value["auth_error"]
    if (not isinstance(error, dict) or set(error) not in ({"reason"}, {"reason", "detail"})
            or any(not isinstance(code, str) or re.fullmatch(r"[a-z][a-z0-9_]{0,127}", code) is None
                   for code in error.values())):
        raise ValueError
    return NativeAuthFailure(error["reason"], error.get("detail"))


class NativeHostedMcpClient:
    """One fixed host ceremony over the same PrivateHostClient as its login.

    Caller cancellation stops waiting only. The host must retain this client
    and call close_owned_hosted_login during its owned shutdown, including when
    begin was uncertain. Native independently observes enrollment/pipe loss.
    """

    def __init__(self, client: PrivateHostClient, login: ConsoleLoginEnrollment,
                 target: McpTarget, *, redirect_uri: str, timeout: float = 310):
        _target(target)
        parsed = _url(redirect_uri)
        if parsed.scheme != "https" or parsed.query:
            raise McpAuthError("invalid_hosted_redirect")
        if (not isinstance(client, PrivateHostClient)
                or not isinstance(login, ConsoleLoginEnrollment) or login._client is not client
                or not login._finished or login._error is not None or login._retiring):
            raise McpAuthError("host_enrollment_unconfirmed")
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 0 < timeout <= 600:
            raise McpAuthError("invalid_host_timeout")
        self._client, self._login, self._target = client, login, target
        self._redirect, self._timeout = redirect_uri, timeout
        self._start_task: asyncio.Task | None = None
        self._close_task: asyncio.Task | None = None
        self._ceremony_id: int | None = None
        self._state: str | None = None

    def __repr__(self):
        return "NativeHostedMcpClient(<host-private>)"

    @classmethod
    async def for_target(cls, client: PrivateHostClient, login: ConsoleLoginEnrollment,
                         target: McpTarget, *, timeout: float = 310):
        """Read Core's exact per-target redirect before mounting the host route.

        This read does not admit an OAuth attempt. The host supplies its fixed
        registered base during native composition, never from this request.
        """
        _target(target)
        if (not isinstance(client, PrivateHostClient)
                or not isinstance(login, ConsoleLoginEnrollment) or login._client is not client
                or not login._finished or login._error is not None or login._retiring):
            raise McpAuthError("host_enrollment_unconfirmed")
        failed = False
        native_failure = None
        try:
            reply = await asyncio.to_thread(client._call,
                {"kind":"hosted_mcp_auth", "enrollment_id":login.enrollment_id,
                 "operation":{"action":"describe", "target":target._wire()}},
                "hosted_mcp_auth", timeout=timeout, reply_fields={"kind", "value"})
            value = reply["value"]
            native_failure = _native_failure(value, target)
            if native_failure is None:
                if not isinstance(value, dict) or set(value) != {"mcp", "redirect_uri"} or value["mcp"] != target._wire():
                    raise ValueError
                return cls(client, login, target, redirect_uri=value["redirect_uri"], timeout=timeout)
        except asyncio.CancelledError:
            raise
        except Exception:
            failed = True
        if failed:
            raise McpAuthError("host_target_unconfirmed") from None
        raise McpAuthError("native_auth_failure", native_failure=native_failure) from None

    @property
    def redirect_uri(self):
        return self._redirect

    def _selection(self, name, url, account, redirect=None, selection=None):
        if (name != self._target.server_name or url != self._target.server_url
                or account != self._target.oauth_account
                or selection != self._target.oauth_account_selection
                or redirect is not None and redirect != self._redirect):
            raise McpAuthError("host_target_mismatch")

    async def _request(self, operation: dict[str, Any]):
        failure = False
        native_failure = None
        try:
            reply = await asyncio.to_thread(self._blocking_request, operation)
            if not isinstance(reply["value"], dict):
                failure = True
            else:
                native_failure = _native_failure(reply["value"], self._target)
                if native_failure is None:
                    return reply["value"]
        except asyncio.CancelledError:
            raise
        except Exception:
            failure = True
        if failure:
            # Do not chain a private frame, code, state or native error.
            raise McpAuthError("host_native_result_unconfirmed") from None
        raise McpAuthError("native_auth_failure", native_failure=native_failure) from None

    def _blocking_request(self, operation):
        identifier, deadline = self._client._start_call(
            {"kind": "hosted_mcp_auth", "enrollment_id": self._login.enrollment_id,
             "operation": operation}, timeout=self._timeout)
        reply = self._client._await_call(identifier, deadline, "hosted_mcp_auth", {"kind", "value"})
        value = reply.get("value")
        if (operation["action"] == "start" and isinstance(value, dict)
                and value.get("disposition") == "started"
                and (type(value.get("ceremony_id")) is not int or value["ceremony_id"] != identifier)):
            raise McpAuthError("host_start_binding_unconfirmed")
        return reply

    async def auth_mcp_login_start(self, server_name, server_url, redirect_uri, *, oauth_account=None, oauth_account_selection=None):
        self._selection(server_name, server_url, oauth_account, redirect_uri, oauth_account_selection)
        if self._close_task is not None:
            raise McpAuthError("host_ceremony_closed")
        if self._start_task is None:
            self._start_task = asyncio.create_task(self._start())
        return await asyncio.shield(self._start_task)

    async def _start(self):
        value = await self._request({"action": "start", "target": self._target._wire()})
        if value.get("mcp") != self._target._wire() or value.get("redirect_uri") != self._redirect:
            raise McpAuthError("host_start_binding_unconfirmed")
        if value.get("disposition") == "started":
            identifier = value.get("ceremony_id")
            if type(identifier) is not int or identifier <= 0 or not _text(value.get("state")):
                raise McpAuthError("host_start_binding_unconfirmed")
            self._ceremony_id, self._state = identifier, value["state"]
        elif value.get("disposition") != "joined" or value.get("ceremony_id") is not None:
            raise McpAuthError("host_start_binding_unconfirmed")
        if "host_attempt_handle" in value:
            raise McpAuthError("host_start_binding_unconfirmed")
        return value

    async def auth_mcp_login_complete_registered(self, server_name, server_url, *, code,
            state, redirect_uri, oauth_account=None, oauth_account_selection=None, iss=None):
        self._selection(server_name, server_url, oauth_account, redirect_uri, oauth_account_selection)
        if (self._ceremony_id is None or self._state is None
                or not isinstance(state, str) or not hmac.compare_digest(state, self._state)
                or not _text(code, 65536) or iss is not None and not _text(iss)):
            raise McpAuthError("host_callback_refused")
        return await self._request({"action": "complete", "ceremony_id": self._ceremony_id,
            "redirect_uri": self._redirect, "state": state, "code": code, "iss": iss})

    async def auth_mcp_login_complete(self, server_name, server_url, **kwargs):
        return await self.auth_mcp_login_complete_registered(server_name, server_url, **kwargs)

    async def auth_mcp_login_cancel(self, server_name, server_url, state, *, oauth_account=None, oauth_account_selection=None):
        self._selection(server_name, server_url, oauth_account, selection=oauth_account_selection)
        if self._state is None or not isinstance(state, str) or not hmac.compare_digest(state, self._state):
            raise McpAuthError("host_callback_refused")
        return await self.close_owned_hosted_login()

    async def auth_mcp_status(self, server_name, server_url, *, oauth_account=None, oauth_account_selection=None):
        self._selection(server_name, server_url, oauth_account, selection=oauth_account_selection)
        return await self._request({"action": "status", "target": self._target._wire()})

    async def close_owned_hosted_login(self):
        if self._close_task is None:
            self._close_task = asyncio.create_task(self._close())
        return await asyncio.shield(self._close_task)

    async def _close(self):
        if self._start_task is not None:
            try:
                await asyncio.shield(self._start_task)
            except McpAuthError:
                # A later response check can fail after exact native custody
                # was retained. Retire only that verified ceremony; an unknown
                # start still supplies no authority to cancel anything.
                if self._ceremony_id is None:
                    raise
        if self._ceremony_id is None:
            return {"mcp": self._target._wire(), "cancelled": False}
        result = await self._request({"action": "cancel", "ceremony_id": self._ceremony_id})
        # Disposal follows the final observation. It never cancels another
        # caller or turns a completed account into a logout.
        released = await self._request({"action": "release", "ceremony_id": self._ceremony_id})
        if released != {"released": True}:
            raise McpAuthError("host_release_unconfirmed")
        return result
