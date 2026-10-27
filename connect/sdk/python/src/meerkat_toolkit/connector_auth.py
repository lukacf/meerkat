"""Host-only loopback-local transport for public connector OAuth methods.

The host admits the exact target and owns a separately protected presenter. This
module neither authorizes a caller nor owns credentials, refresh, provider OAuth
parameters or service clients. Its HTTP loopback callback profile is not a claim
of compatibility with the Toolkit Slack/Google profiles. Never expose private
handles, the client, authorization URL, state or callback to agents or logs.
"""
from __future__ import annotations

import asyncio
import math
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Protocol

# Reuse only bounded HTTP callback mechanics and value-format helpers. MCP target,
# join/disposition, observation, error and lifecycle types are deliberately separate.
from .mcp_auth import _Listener, _origin, _text, _url


class ConnectorAuthClient(Protocol):
    """Public MeerkatClient methods; no production SDK import or new wire API."""

    async def auth_connector_login_start(self, target: dict[str, Any],
                                          redirect_uri: str) -> Any: ...
    async def auth_connector_login_complete(self, target: dict[str, Any], *,
                                             code: str, state: str,
                                             redirect_uri: str) -> Any: ...
    async def auth_connector_login_cancel(self, realm_id: str, slot_id: str,
                                           state: str) -> Any: ...
    async def auth_connector_status(self, realm_id: str, slot_id: str) -> Any: ...


@dataclass(frozen=True)
class ConnectorSlot:
    """Host-selected credential storage address, never an account identity."""
    realm_id: str
    slot_id: str

    def _wire(self) -> dict[str, str]:
        return {"realm_id": self.realm_id, "slot_id": self.slot_id}


@dataclass(frozen=True)
class Known:
    """Reconnect the selected provider subject; compatibility belongs to native."""
    account: str


@dataclass(frozen=True)
class Discover:
    """Ask native to discover an account for an empty selected slot."""


AccountSelection = Known | Discover


@dataclass(frozen=True)
class ConnectorTarget:
    slot: ConnectorSlot
    issuer: str = field(repr=False)
    client: str = field(repr=False)
    resource: str = field(repr=False)
    scopes: tuple[str, ...]
    strategy_id: str
    account_selection: AccountSelection

    def _wire(self) -> dict[str, Any]:
        selection = ({"mode": "known", "account": self.account_selection.account}
                     if type(self.account_selection) is Known else {"mode": "discover"})
        return {"slot": self.slot._wire(), "issuer": self.issuer, "client": self.client,
                "resource": self.resource, "scopes": list(self.scopes),
                "strategy_id": self.strategy_id, "account_selection": selection}


@dataclass(frozen=True)
class ConnectorAuthPresentation:
    """Allowlisted host labels. Requested account/slot are not account proof."""
    authorization_origin: str
    slot: ConnectorSlot
    account_selection: AccountSelection
    callback_profile: str = "loopback_local"


@dataclass(frozen=True)
class VerifiedConnectorAccount:
    issuer: str
    strategy_id: str
    subject: str


@dataclass(frozen=True)
class ConnectorScopeEvidence:
    kind: str
    granted_at: str | None = None


@dataclass(frozen=True)
class ConnectorCredentialObservation:
    """Native observations, not current connection readiness or task permission.

    Missing status scopes remain None, distinct from an explicitly returned [].
    A native authorized phase does not manufacture missing account/evidence.
    """
    phase: str
    slot: ConnectorSlot
    verified_account: VerifiedConnectorAccount | None
    scopes: tuple[str, ...] | None
    scope_evidence: ConnectorScopeEvidence | None
    expires_at: str | None
    has_refresh_token: bool


@dataclass(frozen=True)
class ConnectorCleanupObservation:
    """Cancel acknowledges only pending-attempt cleanup, never credential removal."""
    callback_closed: bool
    native_cancel: str  # acknowledged, unconfirmed, not_requested, not_needed


class ConnectorAuthError(RuntimeError):
    """Value-free local failure, not a synthesized native refusal classification."""
    def __init__(self, code: str, cleanup: ConnectorCleanupObservation | None = None):
        self.code, self.cleanup = code, cleanup
        super().__init__(code)


class ConnectorAuthCancelled(asyncio.CancelledError):
    def __init__(self, cleanup: ConnectorCleanupObservation):
        self.cleanup = cleanup
        super().__init__("connector_auth_cancelled")


def _bounds(*values: float) -> None:
    if any(type(v) not in (int, float) or not math.isfinite(v) or not 0 < v <= 300
           for v in values):
        raise ConnectorAuthError("invalid_wait_bound")


def _target(target: ConnectorTarget) -> None:
    if (type(target) is not ConnectorTarget or type(target.slot) is not ConnectorSlot
            or not _text(target.slot.realm_id, 2048) or not target.slot.realm_id.strip()
            or not _text(target.slot.slot_id, 2048) or not target.slot.slot_id.strip()
            or not _text(target.client) or not target.client.strip()
            or not _text(target.strategy_id, 2048) or not target.strategy_id.strip()
            or type(target.scopes) is not tuple or len(target.scopes) > 256
            or any(not _text(s, 2048) or not s.strip() for s in target.scopes)
            or len(set(target.scopes)) != len(target.scopes)):
        raise ValueError
    _url(target.issuer)
    # Resource is an OAuth audience identifier, not necessarily an HTTP URL.
    if not _text(target.resource) or not target.resource.strip():
        raise ValueError
    if type(target.account_selection) is Known:
        if not _text(target.account_selection.account, 2048) or not target.account_selection.account.strip():
            raise ValueError
    elif type(target.account_selection) is not Discover:
        raise ValueError


def _matches(value: Any, slot: ConnectorSlot) -> bool:
    return isinstance(value, dict) and value == slot._wire()


def _account(value: Any, target: ConnectorTarget) -> VerifiedConnectorAccount:
    if (not isinstance(value, dict) or set(value) != {"issuer", "strategy_id", "subject"}
            or value.get("issuer") != target.issuer
            or value.get("strategy_id") != target.strategy_id
            or not _text(value.get("subject"), 2048)):
        raise ValueError
    if type(target.account_selection) is Known and value["subject"] != target.account_selection.account:
        raise ValueError
    return VerifiedConnectorAccount(value["issuer"], value["strategy_id"], value["subject"])


def _evidence(value: Any) -> ConnectorScopeEvidence:
    if not isinstance(value, dict):
        raise ValueError
    if value == {"kind": "token_endpoint_response"}:
        return ConnectorScopeEvidence("token_endpoint_response")
    if (set(value) == {"kind", "granted_at"} and value.get("kind") == "retained_on_refresh"
            and _text(value.get("granted_at"), 128)):
        return ConnectorScopeEvidence("retained_on_refresh", value["granted_at"])
    raise ValueError


def _observation(value: Any, target: ConnectorTarget, *, completed: bool) -> ConnectorCredentialObservation:
    if not isinstance(value, dict) or not _matches(value.get("connector"), target.slot):
        raise ValueError
    expires, refresh = value.get("expires_at"), value.get("has_refresh_token")
    if (expires is not None and not _text(expires, 128)) or type(refresh) is not bool:
        raise ValueError
    account = value.get("verified_account")
    account = None if account is None else _account(account, target)
    evidence = value.get("scope_evidence")
    evidence = None if evidence is None else _evidence(evidence)
    scopes = None
    if "scopes" in value:
        raw = value["scopes"]
        if (not isinstance(raw, list) or len(raw) > 256
                or any(not _text(s, 2048) for s in raw) or len(set(raw)) != len(raw)):
            raise ValueError
        scopes = tuple(raw)
    phase = "authorized" if completed else value.get("phase")
    if phase not in ("authorized", "reauth_required", "authorization_required"):
        raise ValueError
    if completed and (account is None or evidence is None or scopes is None):
        raise ValueError
    return ConnectorCredentialObservation(phase, target.slot, account, scopes, evidence,
                                          expires, refresh)


async def observe_connector_auth(client: ConnectorAuthClient, target: ConnectorTarget, *,
                                  rpc_timeout: float = 10) -> ConnectorCredentialObservation:
    """One admitted native read; no refresh, polling, start or readiness inference."""
    _bounds(rpc_timeout)
    failure, cancelled = "invalid_target", False
    try:
        _target(target)
        failure = "status_unconfirmed"
        value = await asyncio.wait_for(client.auth_connector_status(
            target.slot.realm_id, target.slot.slot_id), rpc_timeout)
        failure = "invalid_status_response"
        return _observation(value, target, completed=False)
    except asyncio.CancelledError:
        cancelled = True
    except Exception:
        pass
    if cancelled:
        raise asyncio.CancelledError("connector_auth_status_wait_cancelled")
    raise ConnectorAuthError(failure)


async def _join(task: asyncio.Task):
    """Finish owned cleanup despite waiter cancellation, then report interruption."""
    interrupted = False
    while True:
        try:
            return await asyncio.shield(task), interrupted
        except asyncio.CancelledError:
            if task.cancelled():
                raise ConnectorAuthError("local_cleanup_unconfirmed") from None
            interrupted = True


@dataclass(frozen=True, repr=False)
class _Outcome:
    observation: ConnectorCredentialObservation | None
    error: str | None
    cleanup: ConnectorCleanupObservation


class StartedConnectorLogin:
    """An owned loopback-local callback transport retained outside UI panels.

    Cancelling wait() or presentation leaves the attempt running. Only the
    admitted host custodian explicitly closes it. This handle has no logout.
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
        """Pass the private URL only to the host's separately protected presenter.

        True means the presenter returned. It proves no browser isolation, account
        authorization or completed login. Failed/cancelled presentation never
        cancels or restarts native auth.
        """
        if self._run_task.done() or self._close_task is not None or not self._authorize_url:
            raise ConnectorAuthError("presentation_no_longer_available")
        try:
            await presenter(self._authorize_url)
            return True
        except asyncio.CancelledError:
            pass
        except Exception:
            return False
        raise asyncio.CancelledError("connector_auth_presentation_wait_cancelled")

    async def wait(self) -> ConnectorCredentialObservation:
        cancelled = False
        try:
            outcome = await asyncio.shield(self._run_task)
        except asyncio.CancelledError:
            cancelled = True
        if cancelled:
            raise asyncio.CancelledError("connector_auth_completion_wait_cancelled")
        if outcome.error:
            raise ConnectorAuthError(outcome.error, outcome.cleanup)
        return outcome.observation

    async def close(self) -> ConnectorCleanupObservation:
        if self._close_task is None:
            self._close_task = asyncio.create_task(self._close())
        outcome, interrupted = await _join(self._close_task)
        if interrupted:
            raise ConnectorAuthCancelled(outcome.cleanup)
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
        state = self._listener.state
        try:
            code = await asyncio.wait_for(asyncio.shield(self._listener.callback), callback_timeout)
            if code is None:
                error = "callback_reported_error"
            else:
                error = "completion_uncertain"
                value = await asyncio.wait_for(self._client.auth_connector_login_complete(
                    self._target._wire(), code=code, state=state,
                    redirect_uri=self._listener.redirect), self._rpc_timeout)
                error = "invalid_completion_response_uncertain"
                observation = _observation(value, self._target, completed=True)
                error = None
        except asyncio.TimeoutError:
            error = error or "callback_wait_abandoned"
        except asyncio.CancelledError:
            error = error or "custodian_closed_uncertain"
        except Exception:
            error = error or "callback_unconfirmed"
        try:
            cleanup, _ = await _join(asyncio.create_task(self._cleanup(state, observation is None)))
        except Exception:
            cleanup = ConnectorCleanupObservation(False, "unconfirmed")
            error = error or "local_cleanup_unconfirmed"
        self._authorize_url = None
        return _Outcome(observation, error, cleanup)

    async def _cleanup(self, state, cancel):
        await self._listener.close()
        disposition = "not_needed"
        if cancel:
            disposition = "unconfirmed"
            try:
                value = await asyncio.wait_for(self._client.auth_connector_login_cancel(
                    self._target.slot.realm_id, self._target.slot.slot_id, state), self._rpc_timeout)
                if (isinstance(value, dict) and _matches(value.get("connector"), self._target.slot)
                        and value.get("cancelled") is True):
                    disposition = "acknowledged"
            except (Exception, asyncio.CancelledError):
                pass
        return ConnectorCleanupObservation(True, disposition)


async def _begin_connector_login(client, target, callback_timeout, rpc_timeout):
    listener, failure = _Listener(), "unsupported_client"
    try:
        if any(not callable(getattr(client, name, None)) for name in (
                "auth_connector_login_start", "auth_connector_login_complete",
                "auth_connector_login_cancel", "auth_connector_status")):
            raise ValueError
        failure = "callback_bind_failed"
        await listener.bind()
        failure = "start_uncertain"
        value = await asyncio.wait_for(client.auth_connector_login_start(
            target._wire(), listener.redirect), rpc_timeout)
        failure = "invalid_start_response_uncertain"
        if not isinstance(value, dict) or not _matches(value.get("connector"), target.slot):
            raise ValueError
        state, url = value.get("state"), value.get("authorize_url")
        if not _text(state) or value.get("redirect_uri") != listener.redirect:
            raise ValueError
        presentation = ConnectorAuthPresentation(_origin(url), target.slot, target.account_selection)
        listener.state = state
        return StartedConnectorLogin(client, target, listener, url, presentation,
                                     callback_timeout, rpc_timeout)
    except (Exception, asyncio.CancelledError):
        # A cancelled native request can already have started the ceremony. Keep
        # uncertainty, without trusting partial state or guessing a cancel target.
        pass
    _, _ = await _join(asyncio.create_task(listener.close()))
    raise ConnectorAuthError(failure, ConnectorCleanupObservation(True, "not_requested"))


@dataclass(frozen=True, repr=False)
class _CustodianResult:
    handle: StartedConnectorLogin | None = None
    cleanup: ConnectorCleanupObservation | None = None
    error: str | None = None

    def raise_error(self):
        # Retain values, not private native exceptions and growing tracebacks.
        if self.error is not None:
            raise ConnectorAuthError(self.error, self.cleanup) from None


class ConnectorLoginCustodian:
    """Retain exactly one admitted host step's connector login start.

    The client, immutable target and bounded loopback-local callback profile stay
    fixed. begin() retains its first result, including uncertainty. Cancelled
    waiters and disposed panels must not call close(); they do not own this step.
    The host revalidates every operation and owns its protected presenter.

    Explicit close() prevents further begin(), joins pending start and retires
    only this owned pending attempt. It never removes a credential or retries a
    lost request. This is in-memory custody, not recovery after process loss.
    """
    __slots__ = ("_client", "_target", "_callback_timeout", "_rpc_timeout",
                 "_begin_task", "_close_task", "_closed")

    def __init__(self, client: ConnectorAuthClient, target: ConnectorTarget, *,
                 callback_timeout: float = 240, rpc_timeout: float = 10):
        _bounds(callback_timeout, rpc_timeout)
        invalid = False
        try:
            _target(target)
        except Exception:
            invalid = True
        if invalid:
            raise ConnectorAuthError("invalid_target")
        self._client, self._target = client, target
        self._callback_timeout, self._rpc_timeout = callback_timeout, rpc_timeout
        self._begin_task = self._close_task = None
        self._closed = False

    async def begin(self) -> StartedConnectorLogin:
        """Share one start result across waiters, without a native join protocol."""
        if self._closed:
            raise ConnectorAuthError("custodian_closed")
        if self._begin_task is None:
            self._begin_task = asyncio.create_task(self._begin())
        cancelled = False
        try:
            result = await asyncio.shield(self._begin_task)
        except asyncio.CancelledError:
            cancelled = True
        if cancelled:
            raise asyncio.CancelledError("connector_auth_start_wait_cancelled")
        if self._closed:
            raise ConnectorAuthError("custodian_closed")
        result.raise_error()
        return result.handle

    async def _begin(self):
        try:
            handle = await _begin_connector_login(self._client, self._target,
                                                  self._callback_timeout, self._rpc_timeout)
            return _CustodianResult(handle=handle)
        except ConnectorAuthError as error:
            return _CustodianResult(cleanup=error.cleanup, error=error.code)
        except (Exception, asyncio.CancelledError):
            return _CustodianResult(cleanup=ConnectorCleanupObservation(False, "not_requested"),
                                    error="start_uncertain")

    async def close(self) -> ConnectorCleanupObservation:
        """Join explicit host shutdown; report waiter cancellation after cleanup."""
        self._closed = True
        if self._close_task is None:
            self._close_task = asyncio.create_task(self._close())
        result, interrupted = await _join(self._close_task)
        if interrupted:
            raise ConnectorAuthCancelled(result.cleanup) from None
        result.raise_error()
        return result.cleanup

    async def _close(self):
        try:
            if self._begin_task is None:
                return _CustodianResult(cleanup=ConnectorCleanupObservation(True, "not_requested"))
            result = await asyncio.shield(self._begin_task)
            if result.error is not None:
                return result
            return _CustodianResult(cleanup=await result.handle.close())
        except ConnectorAuthError as error:
            return _CustodianResult(cleanup=error.cleanup, error=error.code)
        except (Exception, asyncio.CancelledError):
            return _CustodianResult(cleanup=ConnectorCleanupObservation(False, "unconfirmed"),
                                    error="custodian_cleanup_unconfirmed")
