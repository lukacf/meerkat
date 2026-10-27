"""Stateless Slack PKCE user-token exchange, refresh and account observation.

The native host owns the ceremony, callback admission, credentials and grants.
This module has no scheduling, browser, persistence or installation authority.
"""

from __future__ import annotations

import asyncio
import base64
from dataclasses import dataclass
from enum import Enum
import json
import re
import time
from typing import Any
from urllib.parse import urlencode, urlsplit

import anyio
import httpx

from ..connectors._httpx_stream import require_prepared_httpx_client, send_with_custody


AUTHORIZATION_ENDPOINT = "https://slack.com/oauth/v2/authorize"
TOKEN_ENDPOINT = "https://slack.com/api/oauth.v2.access"
IDENTITY_ENDPOINT = "https://slack.com/api/auth.test"
READ_SCOPES = frozenset(f"{kind}:{operation}" for kind in
    ("channels", "groups", "im", "mpim") for operation in ("read", "history")) | {"users:read"}
_TOKEN_REFUSAL_CODES = frozenset({"bad_redirect_uri", "invalid_code", "invalid_refresh_token", "pkce_not_allowed"})


class ExchangeState(str, Enum):
    NOT_STARTED = "not_started"
    UNCONFIRMED = "unconfirmed"
    RESPONSE_RECEIVED = "response_received"


class ScopeEvidence(str, Enum):
    """Local provenance labels, not native owner objects or permission grants."""

    TOKEN_ENDPOINT_RESPONSE = "TokenEndpointResponse"
    PROVIDER_INTROSPECTION = "ProviderIntrospection"


class SlackOAuthError(Exception):
    def __init__(self, reason: str, phase: str, exchange_state: ExchangeState, *, provider_code: str | None = None):
        self.reason, self.phase, self.exchange_state = reason, phase, exchange_state
        self.provider_code = (provider_code if reason == "provider_rejected" and phase == "token"
            and exchange_state == ExchangeState.RESPONSE_RECEIVED and type(provider_code) is str
            and provider_code in _TOKEN_REFUSAL_CODES else None)
        super().__init__(f"Slack OAuth {reason}; phase: {phase}; exchange: {exchange_state.value}")


class SlackOAuthCancelled(asyncio.CancelledError):
    def __init__(self, phase: str, exchange_state: ExchangeState):
        self.phase, self.exchange_state = phase, exchange_state
        super().__init__(f"Slack OAuth cancelled; phase: {phase}; exchange: {exchange_state.value}")


def _text(value: Any, maximum: int) -> str:
    if (type(value) is not str or not 1 <= len(value) <= maximum
            or any(not 33 <= ord(char) <= 126 for char in value)):
        raise ValueError()
    return value


def _identifier(value: Any) -> str:
    if type(value) is not str or re.fullmatch(r"[A-Za-z0-9]{1,128}", value) is None:
        raise ValueError()
    return value


@dataclass(frozen=True, repr=False)
class SlackReconnectSpec:
    client_id: str
    app_id: str
    redirect_uri: str
    expected_team_id: str
    expected_user_id: str
    requested_scopes: frozenset[str]
    expected_enterprise_id: str | None = None

    def __post_init__(self):
        invalid = False
        try:
            _text(self.client_id, 512)
            for value in (self.app_id, self.expected_team_id, self.expected_user_id):
                _identifier(value)
            if self.expected_enterprise_id is not None:
                _identifier(self.expected_enterprise_id)
            _text(self.redirect_uri, 2048)
            url = urlsplit(self.redirect_uri)
            if (url.scheme != "http" or url.hostname != "localhost"
                    or url.username is not None or url.password is not None
                    or url.port is None or not 1 <= url.port <= 65535
                    or url.netloc != f"localhost:{url.port}"
                    or not url.path.startswith("/") or "\\" in self.redirect_uri
                    or url.query or url.fragment or "?" in self.redirect_uri or "#" in self.redirect_uri):
                raise ValueError()
            if (type(self.requested_scopes) is not frozenset or not self.requested_scopes
                    or not self.requested_scopes <= READ_SCOPES):
                raise ValueError()
        except Exception:
            invalid = True
        if invalid:
            raise SlackOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)

    def __repr__(self):
        return "SlackReconnectSpec(<host-private>)"


@dataclass(frozen=True, repr=False)
class SlackRefreshSpec:
    client_id: str
    app_id: str
    expected_team_id: str
    expected_user_id: str
    required_scopes: frozenset[str]
    expected_enterprise_id: str | None = None

    def __post_init__(self):
        invalid = False
        try:
            _text(self.client_id, 512)
            for value in (self.app_id, self.expected_team_id, self.expected_user_id):
                _identifier(value)
            if self.expected_enterprise_id is not None:
                _identifier(self.expected_enterprise_id)
            if (type(self.required_scopes) is not frozenset or not self.required_scopes
                    or not self.required_scopes <= READ_SCOPES):
                raise ValueError()
        except Exception:
            invalid = True
        if invalid:
            raise SlackOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)

    def __repr__(self):
        return "SlackRefreshSpec(<host-private>)"


@dataclass(frozen=True, repr=False)
class ProtectedAuthorizationRequest:
    url: str

    def __repr__(self):
        return "ProtectedAuthorizationRequest(<host-private>)"


@dataclass(frozen=True)
class SlackObservation:
    client_id: str
    app_id: str
    team_id: str
    user_id: str
    exchange_enterprise_present: bool
    exchange_enterprise_id: str | None
    probe_enterprise_present: bool
    probe_enterprise_id: str | None
    exchange_is_enterprise_install: bool | None
    probe_is_enterprise_install: bool | None
    exchange_scopes: tuple[str, ...]
    introspection_scopes: tuple[str, ...] | None
    granted_scopes: tuple[str, ...]
    scope_evidence: ScopeEvidence
    observed_at: int
    token_type: str = "user"


@dataclass(frozen=True, repr=False)
class SlackProtocolResult:
    access_token: str
    refresh_token: str | None
    expires_in: int | None
    token_received_at: int
    expires_at: int | None
    observation: SlackObservation

    def safe_view(self) -> dict[str, Any]:
        observed = self.observation
        return {"client_id": observed.client_id, "app_id": observed.app_id,
                "team_id": observed.team_id, "user_id": observed.user_id,
                "exchange_enterprise_present": observed.exchange_enterprise_present,
                "exchange_enterprise_id": observed.exchange_enterprise_id,
                "probe_enterprise_present": observed.probe_enterprise_present,
                "probe_enterprise_id": observed.probe_enterprise_id,
                "exchange_is_enterprise_install": observed.exchange_is_enterprise_install,
                "probe_is_enterprise_install": observed.probe_is_enterprise_install,
                "exchange_scopes": list(observed.exchange_scopes),
                "introspection_scopes": None if observed.introspection_scopes is None
                    else list(observed.introspection_scopes),
                "granted_scopes": list(observed.granted_scopes),
                "scope_evidence": observed.scope_evidence.value,
                "observed_at": observed.observed_at, "token_type": observed.token_type,
                "expires_in": self.expires_in, "token_received_at": self.token_received_at,
                "expires_at": self.expires_at}

    def __repr__(self):
        return "SlackProtocolResult(<host-private>)"


def _spec(spec: SlackReconnectSpec) -> SlackReconnectSpec:
    if type(spec) is not SlackReconnectSpec:
        raise ValueError()
    return SlackReconnectSpec(spec.client_id, spec.app_id, spec.redirect_uri,
        spec.expected_team_id, spec.expected_user_id, spec.requested_scopes, spec.expected_enterprise_id)


def authorization_request(spec: SlackReconnectSpec, *, state: str,
                          code_challenge: str) -> ProtectedAuthorizationRequest:
    invalid = False
    try:
        spec = _spec(spec)
        _text(state, 1024)
        if type(code_challenge) is not str or re.fullmatch(r"[A-Za-z0-9_-]{43}", code_challenge) is None:
            raise ValueError()
        decoded = base64.urlsafe_b64decode(code_challenge + "=")
        if len(decoded) != 32 or base64.urlsafe_b64encode(decoded).decode().rstrip("=") != code_challenge:
            raise ValueError()
        parameters = {"client_id": spec.client_id, "redirect_uri": spec.redirect_uri,
            "scope": "", "user_scope": ",".join(sorted(spec.requested_scopes)),
            "state": state, "code_challenge": code_challenge,
            "code_challenge_method": "S256", "team": spec.expected_team_id}
        result = ProtectedAuthorizationRequest(AUTHORIZATION_ENDPOINT + "?" + urlencode(parameters))
    except Exception:
        invalid = True
    if invalid:
        raise SlackOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)
    return result


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError()
        result[key] = value
    return result


def _reject(_value):
    raise ValueError()


def _json(raw: bytes) -> dict[str, Any]:
    value = json.loads(raw.decode("utf-8"), object_pairs_hook=_object,
                       parse_constant=_reject, parse_float=_reject)
    pending, count = [(value, 0)], 0
    while pending:
        item, depth = pending.pop()
        count += 1
        if depth > 12 or count > 4096:
            raise ValueError()
        if type(item) is dict:
            for key in item:
                key.encode("utf-8")
            pending.extend((v, depth + 1) for v in item.values())
        elif type(item) is list:
            pending.extend((v, depth + 1) for v in item)
        elif type(item) is str:
            item.encode("utf-8")
    if type(value) is not dict:
        raise ValueError()
    return value


def _scopes(value: Any) -> tuple[str, ...]:
    if type(value) is not str or not 1 <= len(value) <= 16384:
        raise ValueError()
    values = [item.strip(" \t") for item in value.split(",")]
    if len(values) > 128 or len(set(values)) != len(values):
        raise ValueError()
    for item in values:
        if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9:_.*-]{0,255}", item) is None:
            raise ValueError()
    return tuple(sorted(values))


class _Failure(Exception):
    def __init__(self, reason, provider_code=None):
        self.reason = reason
        self.provider_code = provider_code


def _token_refusal_code(payload):
    # Diagnostic only: unknown or success-bearing envelopes remain generic.
    if (payload.get("ok") is not False or set(payload) - {"ok", "error", "error_description", "error_uri"}
            or any(type(payload[key]) is not str for key in ("error_description", "error_uri") if key in payload)):
        return None
    code = payload.get("error")
    return code if type(code) is str and code in _TOKEN_REFUSAL_CODES else None


@dataclass
class _Progress:
    phase: str = "inputs"
    exchange_state: ExchangeState = ExchangeState.NOT_STARTED
    token_started_at: int = 0
    token_started_monotonic: float = 0
    token_received_at: int = 0


async def _fetch(client, progress, url, *, content=b"", token=None, maximum=65536):
    # Previous cleanup may outlive the deadline; no next send before this check.
    await anyio.lowlevel.checkpoint()
    headers = {"Accept": "application/json", "Accept-Encoding": "identity",
               "Content-Type": "application/x-www-form-urlencoded"}
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    request = httpx.Request("POST", url, headers=headers, content=content)
    if url == TOKEN_ENDPOINT:
        progress.exchange_state = ExchangeState.UNCONFIRMED
        progress.token_started_at = int(time.time())
        progress.token_started_monotonic = time.monotonic()
    response = await send_with_custody(client, request, auth=httpx.Auth())
    if url == TOKEN_ENDPOINT:
        progress.exchange_state = ExchangeState.RESPONSE_RECEIVED
        progress.token_received_at = int(time.time())
    try:
        if response.status_code != 200:
            raise _Failure("provider_rejected")
        if (response.headers.get("content-encoding", "identity").lower() != "identity"
                or response.headers.get("content-type", "").split(";", 1)[0].strip().lower() != "application/json"):
            raise _Failure("malformed_response")
        lengths = response.headers.get_list("content-length")
        if len(lengths) > 1:
            raise _Failure("malformed_response")
        length = lengths[0] if lengths else None
        if length is not None and (not length.isascii() or not length.isdecimal()
                or len(length) > 9 or int(length) > maximum):
            raise _Failure("response_limit")
        body = bytearray()
        chunks = response.aiter_bytes() if response.is_stream_consumed else response.aiter_raw()
        async for chunk in chunks:
            if len(body) + len(chunk) > maximum:
                raise _Failure("response_limit")
            body.extend(chunk)
        if length is not None and len(body) != int(length):
            raise _Failure("malformed_response")
        payload = _json(bytes(body))
        if type(payload.get("ok")) is not bool:
            raise _Failure("malformed_response")
        if not payload["ok"]:
            raise _Failure("provider_rejected", _token_refusal_code(payload) if url == TOKEN_ENDPOINT else None)
        scope_headers = response.headers.get_list("x-oauth-scopes") if url == IDENTITY_ENDPOINT else []
        if len(scope_headers) > 1:
            raise _Failure("malformed_response")
        scopes = _scopes(scope_headers[0]) if scope_headers else None
        return payload, scopes
    finally:
        with anyio.CancelScope(shield=True):
            await response.aclose()


def _mode(payload):
    if "is_enterprise_install" not in payload:
        return None
    value = payload["is_enterprise_install"]
    if type(value) is not bool:
        raise _Failure("malformed_response")
    if value:
        raise _Failure("unsupported_installation")
    return value


def _enterprise(payload, field, nested=False):
    if field not in payload:
        return False, None
    value = payload[field]
    if value is None:
        return True, None
    if nested:
        if type(value) is not dict:
            raise _Failure("malformed_response")
        value = value.get("id")
    return True, _identifier(value)


def _not_expired(progress, expires):
    if expires is not None and (time.time() >= progress.token_started_at + expires
            or time.monotonic() >= progress.token_started_monotonic + expires):
        raise _Failure("expired")


async def _run(spec, form, client, scope, progress):
    error, result = None, None
    try:
        with scope:
            progress.phase = "token"
            token, _ = await _fetch(client, progress, TOKEN_ENDPOINT, content=urlencode(form).encode())
            refreshing = type(spec) is SlackRefreshSpec
            required_scopes = spec.required_scopes if refreshing else spec.requested_scopes
            # Refresh responses put the selected token at the top level. They
            # do not assert a user ID; auth.test below must observe that ID.
            user = token if refreshing else token.get("authed_user")
            team = token.get("team")
            if type(user) is not dict or type(team) is not dict:
                raise _Failure("malformed_response")
            if (user.get("token_type") != "user"
                    or (not refreshing and _identifier(user.get("id")) != spec.expected_user_id)
                    or _identifier(team.get("id")) != spec.expected_team_id
                    or _identifier(token.get("app_id")) != spec.app_id):
                raise _Failure("identity_mismatch")
            if (refreshing and "user_id" in token
                    and _identifier(token["user_id"]) != spec.expected_user_id):
                raise _Failure("identity_mismatch")
            exchange_mode = _mode(token)
            exchange_enterprise = _enterprise(token, "enterprise", nested=True)
            if (spec.expected_enterprise_id is not None and exchange_enterprise[0]
                    and exchange_enterprise[1] != spec.expected_enterprise_id):
                raise _Failure("identity_mismatch")
            access = _text(user.get("access_token"), 8192)
            exchange_scopes = _scopes(user.get("scope"))
            if not required_scopes <= set(exchange_scopes):
                raise _Failure("missing_scope")
            expires, refresh = None, None
            if refreshing or "expires_in" in user or "refresh_token" in user:
                expires = user.get("expires_in")
                refresh = _text(user.get("refresh_token"), 8192)
                if type(expires) is not int or not 1 <= expires <= 2**31 - 1:
                    raise _Failure("malformed_response")
                if refreshing and refresh == form["refresh_token"]:
                    raise _Failure("malformed_response")
            _not_expired(progress, expires)
            progress.phase = "identity"
            probe, introspection_scopes = await _fetch(client, progress, IDENTITY_ENDPOINT,
                                                       token=access, maximum=32768)
            if (_identifier(probe.get("team_id")) != spec.expected_team_id
                    or _identifier(probe.get("user_id")) != spec.expected_user_id
                    or "bot_id" in probe
                    or ("app_id" in probe and _identifier(probe["app_id"]) != spec.app_id)
                    or ("token_type" in probe and probe["token_type"] != "user")):
                raise _Failure("identity_mismatch")
            probe_mode = _mode(probe)
            probe_enterprise = _enterprise(probe, "enterprise_id")
            if ((exchange_enterprise[0] and probe_enterprise[0]
                    and exchange_enterprise[1] != probe_enterprise[1])
                    or (spec.expected_enterprise_id is not None
                        and probe_enterprise[1] != spec.expected_enterprise_id)):
                raise _Failure("identity_mismatch")
            if introspection_scopes is not None and not required_scopes <= set(introspection_scopes):
                raise _Failure("missing_scope")
            _not_expired(progress, expires)
            result = SlackProtocolResult(access, refresh, expires, progress.token_received_at,
                None if expires is None else progress.token_started_at + expires,
                SlackObservation(spec.client_id, spec.app_id, spec.expected_team_id, spec.expected_user_id,
                    *exchange_enterprise, *probe_enterprise, exchange_mode, probe_mode, exchange_scopes,
                    introspection_scopes, exchange_scopes if introspection_scopes is None else introspection_scopes,
                    ScopeEvidence.TOKEN_ENDPOINT_RESPONSE if introspection_scopes is None
                        else ScopeEvidence.PROVIDER_INTROSPECTION, int(time.time())))
        if scope.cancel_called:
            error = SlackOAuthError("deadline", progress.phase, progress.exchange_state)
    except _Failure as failure:
        error = SlackOAuthError(failure.reason, progress.phase, progress.exchange_state, provider_code=failure.provider_code)
    except asyncio.CancelledError:
        error = SlackOAuthCancelled(progress.phase, progress.exchange_state)
    except (httpx.HTTPError, TimeoutError):
        error = SlackOAuthError("transport_unavailable", progress.phase, progress.exchange_state)
    except Exception:
        error = SlackOAuthError("malformed_response", progress.phase, progress.exchange_state)
    return result, error


async def exchange_and_observe(spec: SlackReconnectSpec, *, code: str, code_verifier: str,
        http_client: httpx.AsyncClient, timeout_seconds: float = 20) -> SlackProtocolResult:
    """Exchange one admitted code and observe the same token's exact account.

    No automatic replay, client-secret fallback, refresh or credential commit.
    The borrowed client must be prepared; host hooks/transport remain trusted.
    """
    invalid = False
    try:
        spec = _spec(spec)
        _text(code, 8192)
        if type(code_verifier) is not str or re.fullmatch(r"[A-Za-z0-9._~-]{43,128}", code_verifier) is None:
            raise ValueError()
        if type(timeout_seconds) not in {int, float} or not 0 < timeout_seconds <= 120:
            raise ValueError()
        if not isinstance(http_client, httpx.AsyncClient) or http_client.is_closed:
            raise ValueError()
        require_prepared_httpx_client(http_client)
        form = {"code": code, "client_id": spec.client_id, "redirect_uri": spec.redirect_uri,
                "code_verifier": code_verifier, "grant_type": "authorization_code"}
    except Exception:
        invalid = True
    if invalid:
        raise SlackOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)
    return await _execute(spec, form, http_client, timeout_seconds)


async def refresh_and_observe(spec: SlackRefreshSpec, *, refresh_token: str,
        http_client: httpx.AsyncClient, timeout_seconds: float = 20) -> SlackProtocolResult:
    """Refresh one host-selected public-client user token and observe its account.

    The host serializes attempts and commits the replacement credentials. No
    retry, old-token fallback, scheduling, client secret or persistence occurs.
    A failure after send entry may follow consumption of the refresh token.
    """
    invalid = False
    try:
        if type(spec) is not SlackRefreshSpec:
            raise ValueError()
        spec = SlackRefreshSpec(spec.client_id, spec.app_id, spec.expected_team_id,
            spec.expected_user_id, spec.required_scopes, spec.expected_enterprise_id)
        _text(refresh_token, 8192)
        if type(timeout_seconds) not in {int, float} or not 0 < timeout_seconds <= 120:
            raise ValueError()
        if not isinstance(http_client, httpx.AsyncClient) or http_client.is_closed:
            raise ValueError()
        require_prepared_httpx_client(http_client)
        form = {"client_id": spec.client_id, "grant_type": "refresh_token", "refresh_token": refresh_token}
    except Exception:
        invalid = True
    if invalid:
        raise SlackOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)
    return await _execute(spec, form, http_client, timeout_seconds)


async def _execute(spec, form, http_client, timeout_seconds):
    cancelled = False
    try:
        await anyio.lowlevel.checkpoint()
    except asyncio.CancelledError:
        cancelled = True
    if cancelled:
        raise SlackOAuthCancelled("inputs", ExchangeState.NOT_STARTED)
    progress = _Progress()
    scope = anyio.CancelScope(deadline=anyio.current_time() + timeout_seconds)
    owner = asyncio.create_task(_run(spec, form, http_client, scope, progress))
    try:
        await asyncio.shield(owner)
    except asyncio.CancelledError:
        cancelled = True
        scope.cancel()
        with anyio.CancelScope(shield=True):
            while not owner.done():
                try:
                    await asyncio.shield(owner)
                except asyncio.CancelledError:
                    pass
    result, error = owner.result()
    if cancelled:
        raise SlackOAuthCancelled(progress.phase, progress.exchange_state)
    if error is not None:
        raise error
    return result
