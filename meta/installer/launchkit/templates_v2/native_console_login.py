"""Retain one native enrollment for the application's existing Console login.

The trusted application supplies its shared private client and exact login.
This adapter owns only its blocking calls and workers. It cannot authenticate
a human, select application policy, replace a login or close the host client.
"""
from __future__ import annotations

import asyncio
from copy import deepcopy
from concurrent.futures import ThreadPoolExecutor
import inspect

from meerkat_toolkit.host_control import (
    ConsoleLoginEnrollment, ConsoleLoginRetirement, ConsoleLoginSelection,
    PrivateHostClient,
)

if __package__:
    from .console_auth import ConsoleConnectLogin
else:
    from console_auth import ConsoleConnectLogin


class NativeConsoleLoginError(RuntimeError):
    """A bounded refusal without private native diagnostics."""
    def __init__(self, code):
        self.code = code
        super().__init__("The protected Console login is unavailable.")


class NativeConsoleLogin:
    """One original enrollment, retained observation and joined retirement.

    Public methods belong to the ConsoleFront event loop. Ordinary blocking
    waits and retirement have separate owned workers: an occupied result wait
    must never consume retirement's executor capacity.
    """
    def __init__(self, client, login, *, issuer, is_current=None):
        if (not isinstance(client, PrivateHostClient)
                or not isinstance(login, ConsoleConnectLogin)
                or not callable(login.is_current)
                or inspect.iscoroutinefunction(login.is_current)
                or is_current is not None and (
                    not callable(is_current) or inspect.iscoroutinefunction(is_current))):
            raise ValueError("Native Console login requires trusted application inputs.")
        self._selection = ConsoleLoginSelection(
            issuer, login.principal, login.login_scope_id, login.expires_at_ms)
        self._client, self._login = client, login
        self._application_current = is_current
        self._revoked = False
        self._begin_task = self._result_task = self._retire_task = None
        self._enrollment = None
        self._calls = self._retirement_calls = None

    @property
    def issuer(self):
        return self._selection.issuer

    @property
    def subject(self):
        return self._selection.subject

    @property
    def login_scope_id(self):
        return self._selection.login_scope_id

    @property
    def expires_at_ms(self):
        return self._selection.expires_at_ms

    @property
    def origin(self):
        return self._login.origin

    def is_current(self):
        """Require the captured login and any selected application owner."""
        try:
            return (not self._revoked and self._login.is_current() is True
                    and (self._application_current is None
                         or self._application_current() is True))
        except Exception:
            return False

    def _require_current(self):
        if not self.is_current():
            raise NativeConsoleLoginError("invalid_binding")

    @staticmethod
    def _retain(task):
        # The task remains the original owner's result, including its failure.
        task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        return task

    def start(self):
        """Claim once in the trusted login owner, before route token minting."""
        if self._begin_task is not None or self._retire_task is not None:
            raise NativeConsoleLoginError("invalid_binding")
        self._require_current()
        asyncio.get_running_loop()
        self._calls = ThreadPoolExecutor(max_workers=1, thread_name_prefix="forge-console-login")
        self._retirement_calls = ThreadPoolExecutor(
            max_workers=1, thread_name_prefix="forge-console-retire")
        self._begin_task = self._retain(asyncio.create_task(self._begin()))

    async def _begin(self):
        # start() already claimed this invocation. Retirement must collect its
        # original handle even if disclosure was revoked before this returns.
        try:
            enrollment = await asyncio.get_running_loop().run_in_executor(
                self._calls, self._client.begin_console_login, self._selection)
            if not isinstance(enrollment, ConsoleLoginEnrollment):
                raise ValueError("Invalid retained Console enrollment.")
            self._enrollment = enrollment
            self._result_task = self._retain(asyncio.create_task(self._result()))
        except BaseException:
            raise NativeConsoleLoginError("outcome_unknown") from None

    async def _result(self):
        try:
            await asyncio.get_running_loop().run_in_executor(
                self._calls, self._enrollment.result)
        except BaseException:
            # SDK result() caches its original exception. Never start another
            # enrollment or attempt to recover a discarded late observation.
            raise NativeConsoleLoginError("outcome_unknown") from None

    async def ready(self):
        """Trusted caller only; success returns the original same-pipe handle."""
        self._require_current()
        if self._begin_task is None:
            raise NativeConsoleLoginError("unavailable")
        await asyncio.shield(self._begin_task)
        self._require_current()
        await asyncio.shield(self._result_task)
        self._require_current()
        return self._enrollment

    def revoke(self):
        """Fence disclosure synchronously before any asynchronous cleanup."""
        self._revoked = True

    def hosted_mcp(self, context, target):
        """Dormant host-owned ceremony on this original enrollment and pipe."""
        self._require_current()
        if (not isinstance(context, ConsoleConnectLogin)
                or (context.principal, context.login_scope_id, context.expires_at_ms, context.origin)
                != (self.subject, self.login_scope_id, self.expires_at_ms, self.origin)):
            raise NativeConsoleLoginError("invalid_binding")
        return NativeConsoleMcp(self, context, target)

    async def retire(self, reason):
        self.revoke()
        if self._retire_task is None:
            self._retire_task = self._retain(asyncio.create_task(self._retire()))
        return await asyncio.shield(self._retire_task)

    async def _retire(self):
        try:
            if self._begin_task is None:
                return None
            begun = await asyncio.gather(asyncio.shield(self._begin_task), return_exceptions=True)
            if any(isinstance(result, BaseException) for result in begun):
                raise NativeConsoleLoginError("cleanup_unconfirmed")
            retirement = None
            try:
                # Do not join result first: it may still occupy the ordinary
                # worker. The SDK retirement lock and this worker are separate.
                retirement = await asyncio.get_running_loop().run_in_executor(
                    self._retirement_calls, self._enrollment.retire)
            except BaseException:
                pass
            # A result timeout stays unknown, but does not erase a separately
            # confirmed local fence/drain. Join the original call either way.
            await asyncio.gather(asyncio.shield(self._result_task), return_exceptions=True)
            if (not isinstance(retirement, ConsoleLoginRetirement)
                    or retirement.fenced is not True
                    or retirement.reservations_drained is not True
                    or retirement.uncertain is not False):
                raise NativeConsoleLoginError("cleanup_unconfirmed")
            return retirement
        finally:
            # Entered SDK calls are joined above before either worker is joined.
            # These pools belong only to this login; the client remains shared.
            for worker in (self._calls, self._retirement_calls):
                if worker is not None:
                    worker.shutdown(wait=True)

    async def close(self):
        return await self.retire("application_stopping")


class NativeConsoleMcp:
    """Retain the SDK's original binding across browser waiter cancellation.

    ConsoleFront owns this object before prepare enters native describe. It
    mounts redirect_uri before begin or presentation, and closes every attempt
    before retiring the original enrollment. This owns no credential store.
    """
    def __init__(self, owner, context, target):
        self.owner, self.context, self.target = owner, context, target
        self._original_target = deepcopy(target)
        self.client = self.binding = None
        self._prepare_task = self._close_task = None
        self._closed = False
        self._begin_entered = False
        self._presenter = None
        self._presentation_lock = asyncio.Lock()

    def __repr__(self):
        return "NativeConsoleMcp(<host-private>)"

    def _current(self):
        if (self._closed or self.target != self._original_target
                or not self.owner.is_current() or self.context.is_current() is not True):
            raise NativeConsoleLoginError("invalid_binding")

    async def prepare(self):
        self._current()
        if self._prepare_task is None:
            self._prepare_task = NativeConsoleLogin._retain(asyncio.create_task(self._prepare()))
        await asyncio.shield(self._prepare_task)
        self._current()
        return self.client.redirect_uri

    async def _prepare(self):
        from meerkat_toolkit.mcp_auth_host import HostedMcpAuthBinding
        from meerkat_toolkit.mcp_auth_native import NativeHostedMcpClient
        enrollment = await self.owner.ready()
        self._current()
        self.client = await NativeHostedMcpClient.for_target(self.owner._client, enrollment, self.target)
        # Retain even a late describe result before checking revoked admission.
        self.binding = HostedMcpAuthBinding(self.client, self.target,
            login_context=self.context, redirect_uri=self.client.redirect_uri,
            protected_presenter=self._present)
        self._current()

    async def begin(self):
        self._current()
        self._begin_entered = True
        result = await self.binding.begin(self.context)
        self._current()
        return result

    async def _present(self, url):
        self._current()
        if self._presenter is None:
            raise NativeConsoleLoginError("unavailable")
        await self._presenter(url)
        self._current()

    async def present(self, presenter):
        async with self._presentation_lock:
            self._current()
            self._presenter = presenter
            try:
                result = await self.binding.present(self.context)
                self._current()
                return result
            finally:
                self._presenter = None

    async def observe(self):
        self._current()
        result = await self.binding.observe(self.context)
        self._current()
        return result

    async def callback(self, **request):
        self._current()
        result = await self.binding.handle_callback(self.context, **request)
        self._current()
        return result

    async def close(self):
        self._closed = True
        if self._close_task is None:
            self._close_task = NativeConsoleLogin._retain(asyncio.create_task(self._close()))
        return await asyncio.shield(self._close_task)

    async def _close(self):
        if self._prepare_task is not None:
            await asyncio.gather(asyncio.shield(self._prepare_task), return_exceptions=True)
        if self.binding is not None:
            from meerkat_toolkit.mcp_auth import McpAuthError
            try:
                cleanup = await self.binding.shutdown()
            except McpAuthError as error:
                # A failed start retains its callback cleanup but shutdown can
                # raise before joining the native adapter's owned ceremony.
                cleanup = error.cleanup
                try:
                    await self.client.close_owned_hosted_login()
                except McpAuthError:
                    # A typed start error can follow native admission and
                    # failed compensation. It is not a cleanup receipt.
                    raise NativeConsoleLoginError("cleanup_unconfirmed") from None
            # An unstarted SDK binding has no claimed callback inbox and
            # reports callback_closed=False, native_cancel=not_requested.
            if (cleanup is None
                    or (self._begin_entered and not cleanup.callback_closed)
                    or cleanup.native_cancel == "unconfirmed"):
                raise NativeConsoleLoginError("cleanup_unconfirmed")
        elif self.client is not None:
            await self.client.close_owned_hosted_login()
