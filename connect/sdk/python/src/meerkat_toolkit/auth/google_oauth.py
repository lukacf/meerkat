"""Stateless Google Desktop code exchange and access-token refresh observation.

The host owns callback admission, ceremony state, credentials and all grants.
Successful results are private protocol observations, not native authority.
"""

from __future__ import annotations

import asyncio
import base64
from dataclasses import dataclass
from enum import Enum
import hashlib
import hmac
import json
import re
import time
from types import MappingProxyType, SimpleNamespace
from typing import Any
from urllib.parse import urlencode, urlsplit

import anyio
import httpx

from ..connectors._httpx_stream import require_prepared_httpx_client, send_with_custody


ISSUER = "https://accounts.google.com"
AUTHORIZATION_ENDPOINT = ISSUER + "/o/oauth2/v2/auth"
TOKEN_ENDPOINT = "https://oauth2.googleapis.com/token"
CERTIFICATES_ENDPOINT = "https://www.googleapis.com/oauth2/v1/certs"
USERINFO_ENDPOINT = "https://openidconnect.googleapis.com/v1/userinfo"
GMAIL_SCOPES = frozenset("https://www.googleapis.com/auth/gmail." + name for name in
    ("send", "readonly", "compose", "modify", "metadata")) | {"https://mail.google.com/"}
CALENDAR_SCOPES = frozenset("https://www.googleapis.com/auth/" + name for name in
    ("calendar", "calendar.readonly", "calendar.events", "calendar.events.readonly",
     "calendar.events.owned", "calendar.events.owned.readonly", "calendar.app.created",
     "calendar.calendarlist.readonly", "calendar.calendars.readonly"))
DRIVE_SCOPES = frozenset("https://www.googleapis.com/auth/" + name for name in
    ("drive", "drive.readonly", "drive.file", "drive.metadata", "drive.metadata.readonly"))
_SERVICE_SCOPES = MappingProxyType({
    "gmail": GMAIL_SCOPES, "calendar": CALENDAR_SCOPES, "drive": DRIVE_SCOPES,
})
_SERVICE_RESOURCES = MappingProxyType({
    "gmail": "https://gmail.googleapis.com/",
    "calendar": "https://www.googleapis.com/calendar/v3/",
    "drive": "https://www.googleapis.com/drive/v3/",
})
_TOKEN_REFUSAL_CODES = frozenset({"invalid_grant", "invalid_client"})


def _validate_service_scopes(service, scopes):
    # A host selection, never inferred from scopes or token response content.
    if (type(service) is not str or service not in _SERVICE_SCOPES
            or type(scopes) is not frozenset or "openid" not in scopes
            or len(scopes) < 2 or not scopes <= (_SERVICE_SCOPES[service] | {"openid"})):
        raise ValueError()


class ExchangeState(str, Enum):
    NOT_STARTED = "not_started"
    UNCONFIRMED = "unconfirmed"
    RESPONSE_RECEIVED = "response_received"


class GoogleOAuthError(Exception):
    """Fixed failure metadata and optional code; never a credential commit."""

    def __init__(self, reason: str, phase: str, exchange_state: ExchangeState, *, provider_code: str | None = None):
        self.reason, self.phase, self.exchange_state = reason, phase, exchange_state
        self.provider_code = (provider_code if reason == "provider_rejected" and phase == "token"
            and exchange_state == ExchangeState.RESPONSE_RECEIVED and type(provider_code) is str
            and provider_code in _TOKEN_REFUSAL_CODES else None)
        super().__init__(f"Google OAuth {reason}; phase: {phase}; exchange: {exchange_state.value}")


class GoogleOAuthCancelled(asyncio.CancelledError):
    def __init__(self, phase: str, exchange_state: ExchangeState):
        self.phase, self.exchange_state = phase, exchange_state
        super().__init__(f"Google OAuth cancelled; phase: {phase}; exchange: {exchange_state.value}")


@dataclass(frozen=True, repr=False)
class GoogleReconnectSpec:
    client_id: str
    redirect_uri: str
    expected_subject: str
    requested_scopes: frozenset[str]
    service: str = "gmail"

    def __post_init__(self):
        invalid = False
        try:
            _text(self.client_id, 512)
            _subject(self.expected_subject)
            _text(self.redirect_uri, 2048)
            url = urlsplit(self.redirect_uri)
            if (url.scheme != "http" or url.hostname not in {"127.0.0.1", "::1"}
                    or url.username is not None or url.password is not None
                    or url.port is None or not 1 <= url.port <= 65535
                    or url.query or url.fragment or "?" in self.redirect_uri or "#" in self.redirect_uri
                    or not url.path.startswith("/")):
                raise ValueError()
            _validate_service_scopes(self.service, self.requested_scopes)
        except Exception:
            invalid = True
        if invalid:
            raise GoogleOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)

    def __repr__(self):
        return "GoogleReconnectSpec(<host-private>)"


@dataclass(frozen=True, repr=False)
class GoogleRefreshSpec:
    client_id: str
    expected_subject: str
    required_scopes: frozenset[str]
    service: str = "gmail"

    def __post_init__(self):
        invalid = False
        try:
            _text(self.client_id, 512)
            _subject(self.expected_subject)
            _validate_service_scopes(self.service, self.required_scopes)
        except Exception:
            invalid = True
        if invalid:
            raise GoogleOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)

    def __repr__(self):
        return "GoogleRefreshSpec(<host-private>)"


@dataclass(frozen=True, repr=False)
class ProtectedAuthorizationRequest:
    url: str

    def __repr__(self):
        return "ProtectedAuthorizationRequest(<host-private>)"


@dataclass(frozen=True)
class GoogleObservation:
    issuer: str
    client_id: str
    subject: str
    reported_scopes: tuple[str, ...]
    verified_at: int
    scope_evidence: str = "google_token_endpoint"
    intended_resource: str = "https://gmail.googleapis.com/"


@dataclass(frozen=True, repr=False)
class GoogleProtocolResult:
    access_token: str
    refresh_token: str | None
    id_token: str
    expires_in: int
    token_received_at: int
    expires_at: int
    observation: GoogleObservation

    def safe_view(self) -> dict[str, Any]:
        return {"issuer": self.observation.issuer, "client_id": self.observation.client_id,
                "subject": self.observation.subject,
                "reported_scopes": list(self.observation.reported_scopes),
                "scope_evidence": self.observation.scope_evidence,
                "intended_resource": self.observation.intended_resource,
                "verified_at": self.observation.verified_at, "expires_in": self.expires_in,
                "token_received_at": self.token_received_at, "expires_at": self.expires_at}

    def __repr__(self):
        return "GoogleProtocolResult(<host-private>)"


@dataclass(frozen=True)
class GoogleRefreshObservation:
    configured_client_id: str
    subject: str
    reported_scopes: tuple[str, ...] | None
    scope_evidence: str | None
    verified_at: int
    identity_evidence: str = "google_userinfo"
    provider: str = "google"
    intended_resource: str = "https://gmail.googleapis.com/"


@dataclass(frozen=True, repr=False)
class GoogleRefreshResult:
    access_token: str
    refresh_token: str | None
    expires_in: int
    token_received_at: int
    expires_at: int
    refresh_token_expires_in: int | None
    refresh_token_expires_at: int | None
    observation: GoogleRefreshObservation

    def safe_view(self) -> dict[str, Any]:
        observed = self.observation
        return {"provider": observed.provider, "configured_client_id": observed.configured_client_id,
                "subject": observed.subject, "identity_evidence": observed.identity_evidence,
                "reported_scopes": None if observed.reported_scopes is None else list(observed.reported_scopes),
                "scope_evidence": observed.scope_evidence, "intended_resource": observed.intended_resource,
                "verified_at": observed.verified_at, "expires_in": self.expires_in,
                "token_received_at": self.token_received_at, "expires_at": self.expires_at,
                "refresh_token_expires_in": self.refresh_token_expires_in,
                "refresh_token_expires_at": self.refresh_token_expires_at}

    def __repr__(self):
        return "GoogleRefreshResult(<host-private>)"


def _text(value: Any, maximum: int) -> str:
    if type(value) is not str or not 1 <= len(value) <= maximum or any(not 33 <= ord(c) <= 126 for c in value):
        raise ValueError()
    return value


def _subject(value: Any) -> str:
    if (type(value) is not str or not 1 <= len(value) <= 255
            or any(not 32 <= ord(c) <= 126 for c in value)):
        raise ValueError()
    return value


def _spec(spec: GoogleReconnectSpec) -> GoogleReconnectSpec:
    if type(spec) is not GoogleReconnectSpec:
        raise ValueError()
    return GoogleReconnectSpec(spec.client_id, spec.redirect_uri, spec.expected_subject,
                               spec.requested_scopes, spec.service)


def authorization_request(spec: GoogleReconnectSpec, *, state: str, nonce: str,
                          code_challenge: str, offline: bool = False,
                          prompt_consent: bool = False) -> ProtectedAuthorizationRequest:
    invalid = False
    try:
        spec = _spec(spec)
        _text(state, 1024)
        _text(nonce, 1024)
        if (type(offline) is not bool or type(prompt_consent) is not bool
                or type(code_challenge) is not str
                or re.fullmatch(r"[A-Za-z0-9_-]{43}", code_challenge) is None):
            raise ValueError()
        decoded = base64.urlsafe_b64decode(code_challenge + "=")
        if len(decoded) != 32 or base64.urlsafe_b64encode(decoded).decode().rstrip("=") != code_challenge:
            raise ValueError()
        parameters = {"client_id": spec.client_id, "redirect_uri": spec.redirect_uri,
            "response_type": "code", "scope": " ".join(sorted(spec.requested_scopes)),
            "state": state, "nonce": nonce, "code_challenge": code_challenge,
            "code_challenge_method": "S256", "login_hint": spec.expected_subject,
            "access_type": "offline" if offline else "online"}
        if prompt_consent:
            parameters["prompt"] = "consent"
        result = ProtectedAuthorizationRequest(AUTHORIZATION_ENDPOINT + "?" + urlencode(parameters))
    except Exception:
        invalid = True
    if invalid:
        raise GoogleOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)
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
            pending.extend((v, depth + 1) for v in item.values())
        elif type(item) is list:
            pending.extend((v, depth + 1) for v in item)
        elif type(item) is str:
            item.encode("utf-8")
    if type(value) is not dict:
        raise ValueError()
    return value


def _reported_scopes(reported: Any) -> tuple[str, ...]:
    if type(reported) is not str or not 1 <= len(reported) <= 16384:
        raise _Failure("malformed_response")
    scopes = reported.split(" ")
    if len(scopes) > 128 or len(set(scopes)) != len(scopes):
        raise _Failure("malformed_response")
    for item in scopes:
        _text(item, 1024)
        if '"' in item or "\\" in item:
            raise _Failure("malformed_response")
    return tuple(sorted(scopes))


def _jwt_part(part: str, maximum: int) -> dict[str, Any]:
    if not part or len(part) > maximum * 2 or re.fullmatch(r"[A-Za-z0-9_-]+", part) is None:
        raise ValueError()
    raw = base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))
    if len(raw) > maximum or base64.urlsafe_b64encode(raw).decode().rstrip("=") != part:
        raise ValueError()
    return _json(raw)


def _claims(token: str) -> tuple[dict[str, Any], dict[str, Any]]:
    parts = token.split(".")
    if len(parts) != 3:
        raise ValueError()
    header, claims = _jwt_part(parts[0], 2048), _jwt_part(parts[1], 16384)
    if (not {"alg", "kid"} <= set(header) or set(header) - {"alg", "kid", "typ"}
            or header["alg"] != "RS256" or header.get("typ", "JWT") != "JWT"):
        raise ValueError()
    _text(header["kid"], 128)
    if not parts[2] or len(parts[2]) > 2048 or re.fullmatch(r"[A-Za-z0-9_-]+", parts[2]) is None:
        raise ValueError()
    return header, claims


class _Failure(Exception):
    def __init__(self, reason, provider_code=None):
        self.reason = reason
        self.provider_code = provider_code


def _token_refusal_code(payload):
    # Diagnostic only: unknown or success-bearing envelopes remain generic.
    if (set(payload) - {"error", "error_description", "error_uri"}
            or any(type(payload[key]) is not str for key in ("error_description", "error_uri") if key in payload)):
        return None
    code = payload.get("error")
    return code if type(code) is str and code in _TOKEN_REFUSAL_CODES else None


async def _response_json(response, maximum):
    if (response.headers.get("content-encoding", "identity").lower() != "identity"
            or response.headers.get("content-type", "").split(";", 1)[0].strip().lower() != "application/json"):
        raise _Failure("malformed_response")
    length = response.headers.get("content-length")
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
    return _json(bytes(body))


@dataclass
class _Progress:
    phase: str = "inputs"
    exchange_state: ExchangeState = ExchangeState.NOT_STARTED
    token_started_at: int = 0
    token_started_monotonic: float = 0
    token_received_at: int = 0


async def _fetch(client, progress, url, *, content=None, token=None, maximum=65536):
    # A previous response close may have completed after the overall deadline.
    # Observe cancellation before creating the next physical request owner.
    await anyio.lowlevel.checkpoint()
    headers = {"Accept": "application/json", "Accept-Encoding": "identity"}
    if content is not None:
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    # Explicit Request avoids client default headers, params and cookies.
    request = httpx.Request("POST" if content is not None else "GET", url, headers=headers, content=content)
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
            code = None
            if url == TOKEN_ENDPOINT and response.status_code in (400, 401):
                try:
                    code = _token_refusal_code(await _response_json(response, maximum))
                except Exception:
                    # Reading optional diagnostics cannot erase an observed
                    # HTTP refusal. Cancellation still escapes to its owner.
                    pass
            raise _Failure("provider_rejected", code)
        return await _response_json(response, maximum)
    finally:
        with anyio.CancelScope(shield=True):
            await response.aclose()


def _verify(spec, token, access, nonce, certificates):
    from google.oauth2.id_token import verify_oauth2_token

    header, claims = _claims(token)
    if not 1 <= len(certificates) <= 32 or "keys" in certificates:
        raise _Failure("verification_unavailable")
    for key, certificate in certificates.items():
        _text(key, 128)
        if (type(certificate) is not str or len(certificate) > 8192
                or not certificate.startswith("-----BEGIN CERTIFICATE-----\n")
                or not certificate.rstrip().endswith("-----END CERTIFICATE-----")):
            raise _Failure("verification_unavailable")
    if header["kid"] not in certificates:
        raise _Failure("verification_unavailable")
    raw = json.dumps(certificates).encode()

    def certificate_response(url, method="GET", **kwargs):
        if url != CERTIFICATES_ENDPOINT or method != "GET" or kwargs:
            raise ValueError()
        return SimpleNamespace(status=200, data=raw)

    # This verifier receives only the prefetched map and cannot perform I/O.
    verified = verify_oauth2_token(token, certificate_response, audience=spec.client_id,
                                  clock_skew_in_seconds=0)
    now = int(time.time())
    if (verified != claims or type(claims.get("aud")) is not str or claims["aud"] != spec.client_id
            or claims.get("iss") not in {ISSUER, "accounts.google.com"}
            or _subject(claims.get("sub")) != spec.expected_subject
            or claims.get("nonce") != nonce
            or ("azp" in claims and claims["azp"] != spec.client_id)):
        raise _Failure("identity_mismatch")
    for field in ("iat", "exp", "nbf", "auth_time"):
        if field not in claims and field in {"nbf", "auth_time"}:
            continue
        value = claims.get(field)
        if type(value) is not int or not 0 <= value <= 2**53:
            raise _Failure("identity_mismatch")
        if (field == "exp" and value <= now) or (field != "exp" and value > now):
            raise _Failure("identity_mismatch")
    if claims["exp"] <= claims["iat"]:
        raise _Failure("identity_mismatch")
    expected = base64.urlsafe_b64encode(hashlib.sha256(access.encode("ascii")).digest()[:16]).decode().rstrip("=")
    if type(claims.get("at_hash")) is not str or not hmac.compare_digest(claims["at_hash"], expected):
        raise _Failure("identity_mismatch")
    return claims["exp"]


async def _run(spec, form, nonce, client, scope, progress):
    error = None
    result = None
    try:
        with scope:
            progress.phase = "token"
            token = await _fetch(client, progress, TOKEN_ENDPOINT, content=urlencode(form).encode())
            access = _text(token.get("access_token"), 8192)
            identity = _text(token.get("id_token"), 32768)
            refresh = None if "refresh_token" not in token else _text(token["refresh_token"], 8192)
            expires = token.get("expires_in")
            if token.get("token_type") != "Bearer" or type(expires) is not int or not 1 <= expires <= 2**31 - 1:
                raise _Failure("malformed_response")
            scopes = _reported_scopes(token.get("scope"))
            if not spec.requested_scopes <= set(scopes):
                raise _Failure("missing_scope")
            # Refuse unsupported JOSE before any key request, never follow key URLs.
            progress.phase = "identity"
            _claims(identity)
            progress.phase = "certificates"
            certificates = await _fetch(client, progress, CERTIFICATES_ENDPOINT, maximum=262144)
            progress.phase = "identity"
            identity_expires_at = _verify(spec, identity, access, nonce, certificates)
            await anyio.lowlevel.checkpoint()
            progress.phase = "userinfo"
            user = await _fetch(client, progress, USERINFO_ENDPOINT, token=access, maximum=32768)
            if _subject(user.get("sub")) != spec.expected_subject:
                raise _Failure("identity_mismatch")
            now = int(time.time())
            # Conservatively start the reported lifetime at POST entry, not at
            # handoff after key retrieval and UserInfo. Monotonic time also
            # prevents a backwards wall-clock change extending this attempt.
            expires_at = progress.token_started_at + expires
            if (now >= identity_expires_at or now >= expires_at
                    or time.monotonic() >= progress.token_started_monotonic + expires):
                raise _Failure("expired")
            result = GoogleProtocolResult(access, refresh, identity, expires,
                progress.token_received_at, expires_at,
                GoogleObservation(ISSUER, spec.client_id, spec.expected_subject, tuple(sorted(scopes)), now,
                                  intended_resource=_SERVICE_RESOURCES[spec.service]))
        if scope.cancel_called:
            error = GoogleOAuthError("deadline", progress.phase, progress.exchange_state)
    except _Failure as failure:
        error = GoogleOAuthError(failure.reason, progress.phase, progress.exchange_state, provider_code=failure.provider_code)
    except asyncio.CancelledError:
        error = GoogleOAuthCancelled(progress.phase, progress.exchange_state)
    except (httpx.HTTPError, TimeoutError):
        error = GoogleOAuthError("transport_unavailable", progress.phase, progress.exchange_state)
    except Exception:
        reason = "identity_mismatch" if progress.phase == "identity" else "malformed_response"
        error = GoogleOAuthError(reason, progress.phase, progress.exchange_state)
    return result, error


def _refresh_not_expired(progress, expires):
    if (time.time() >= progress.token_started_at + expires
            or time.monotonic() >= progress.token_started_monotonic + expires):
        raise _Failure("expired")


async def _run_refresh(spec, form, client, scope, progress):
    error, result = None, None
    try:
        with scope:
            progress.phase = "token"
            token = await _fetch(client, progress, TOKEN_ENDPOINT, content=urlencode(form).encode())
            if "error" in token:
                raise _Failure("provider_rejected")
            access = _text(token.get("access_token"), 8192)
            replacement = None if "refresh_token" not in token else _text(token["refresh_token"], 8192)
            expires = token.get("expires_in")
            if token.get("token_type") != "Bearer" or type(expires) is not int or not 1 <= expires <= 2**31 - 1:
                raise _Failure("malformed_response")
            refresh_expires = None
            if "refresh_token_expires_in" in token:
                refresh_expires = token["refresh_token_expires_in"]
                if type(refresh_expires) is not int or not 1 <= refresh_expires <= 2**31 - 1:
                    raise _Failure("malformed_response")
            scopes = None if "scope" not in token else _reported_scopes(token["scope"])
            if scopes is not None and not spec.required_scopes <= set(scopes):
                raise _Failure("missing_scope")
            # This operation consumes only the access token. Any id_token is
            # discarded, never decoded, verified, returned or used as evidence.
            _refresh_not_expired(progress, expires)
            progress.phase = "userinfo"
            user = await _fetch(client, progress, USERINFO_ENDPOINT, token=access, maximum=32768)
            if "error" in user:
                raise _Failure("provider_rejected")
            if _subject(user.get("sub")) != spec.expected_subject:
                raise _Failure("identity_mismatch")
            _refresh_not_expired(progress, expires)
            result = GoogleRefreshResult(access, replacement, expires, progress.token_received_at,
                progress.token_started_at + expires, refresh_expires,
                None if refresh_expires is None else progress.token_started_at + refresh_expires,
                GoogleRefreshObservation(spec.client_id, spec.expected_subject, scopes,
                    None if scopes is None else "google_token_endpoint", int(time.time()),
                    intended_resource=_SERVICE_RESOURCES[spec.service]))
        if scope.cancel_called:
            error = GoogleOAuthError("deadline", progress.phase, progress.exchange_state)
    except _Failure as failure:
        error = GoogleOAuthError(failure.reason, progress.phase, progress.exchange_state, provider_code=failure.provider_code)
    except asyncio.CancelledError:
        error = GoogleOAuthCancelled(progress.phase, progress.exchange_state)
    except (httpx.HTTPError, TimeoutError):
        error = GoogleOAuthError("transport_unavailable", progress.phase, progress.exchange_state)
    except Exception:
        error = GoogleOAuthError("malformed_response", progress.phase, progress.exchange_state)
    return result, error


async def exchange_and_observe(spec: GoogleReconnectSpec, *, code: str, code_verifier: str,
        expected_nonce: str, http_client: httpx.AsyncClient, client_secret: str | None = None,
        timeout_seconds: float = 20) -> GoogleProtocolResult:
    """Exchange once after host callback admission; never retry a consumed code.

    Uses a prepared borrowed client with explicit requests and empty HTTPX auth.
    Host hooks/transport must not add credentials, retarget, replay or detach I/O.
    Cancellation/deadlines join response cleanup even if cleanup finishes later.
    """
    invalid = False
    try:
        spec = _spec(spec)
        _text(code, 8192)
        _text(expected_nonce, 1024)
        if type(code_verifier) is not str or re.fullmatch(r"[A-Za-z0-9._~-]{43,128}", code_verifier) is None:
            raise ValueError()
        if client_secret is not None:
            _text(client_secret, 4096)
        if type(timeout_seconds) not in {int, float} or not 0 < timeout_seconds <= 120:
            raise ValueError()
        if not isinstance(http_client, httpx.AsyncClient) or http_client.is_closed:
            raise ValueError()
        require_prepared_httpx_client(http_client)
        # Fail missing optional verifier dependency before the one-time POST.
        from google.oauth2.id_token import verify_oauth2_token  # noqa: F401
        form = {"code": code, "client_id": spec.client_id, "redirect_uri": spec.redirect_uri,
                "code_verifier": code_verifier, "grant_type": "authorization_code"}
        if client_secret is not None:
            form["client_secret"] = client_secret
    except Exception:
        invalid = True
    if invalid:
        raise GoogleOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)
    return await _execute(_run, (spec, form, expected_nonce), http_client, timeout_seconds)


async def refresh_and_observe(spec: GoogleRefreshSpec, *, refresh_token: str,
        http_client: httpx.AsyncClient, client_secret: str | None = None,
        timeout_seconds: float = 20) -> GoogleRefreshResult:
    """Refresh a host-selected unbound Desktop bearer credential once.

    Observe only the new token's UserInfo subject and explicitly reported
    scopes. Absent scopes stay unknown: a returned result is not a grant or
    commit decision. No ID token, nonce, retry or retained credential is used.
    """
    invalid = False
    try:
        if type(spec) is not GoogleRefreshSpec:
            raise ValueError()
        spec = GoogleRefreshSpec(spec.client_id, spec.expected_subject, spec.required_scopes, spec.service)
        _text(refresh_token, 8192)
        if client_secret is not None:
            _text(client_secret, 4096)
        if type(timeout_seconds) not in {int, float} or not 0 < timeout_seconds <= 120:
            raise ValueError()
        if not isinstance(http_client, httpx.AsyncClient) or http_client.is_closed:
            raise ValueError()
        require_prepared_httpx_client(http_client)
        form = {"client_id": spec.client_id, "refresh_token": refresh_token, "grant_type": "refresh_token"}
        if client_secret is not None:
            form["client_secret"] = client_secret
    except Exception:
        invalid = True
    if invalid:
        raise GoogleOAuthError("invalid_inputs", "inputs", ExchangeState.NOT_STARTED)
    return await _execute(_run_refresh, (spec, form), http_client, timeout_seconds)


async def _execute(run, arguments, http_client, timeout_seconds):
    cancelled = False
    try:
        await anyio.lowlevel.checkpoint()
    except asyncio.CancelledError:
        cancelled = True
    if cancelled:
        raise GoogleOAuthCancelled("inputs", ExchangeState.NOT_STARTED)
    progress = _Progress()
    scope = anyio.CancelScope(deadline=anyio.current_time() + timeout_seconds)
    owner = asyncio.create_task(run(*arguments, http_client, scope, progress))
    try:
        await asyncio.shield(owner)
    except asyncio.CancelledError:
        cancelled = True
        scope.cancel()
        # Outer AnyIO level cancellation must not repeatedly interrupt this
        # join. Direct repeated Task.cancel is still observed without sending
        # cancellation into the retained protocol/response-cleanup owner.
        with anyio.CancelScope(shield=True):
            while not owner.done():
                try:
                    await asyncio.shield(owner)
                except asyncio.CancelledError:
                    pass
    result, error = owner.result()
    if cancelled:
        raise GoogleOAuthCancelled(progress.phase, progress.exchange_state)
    if error is not None:
        raise error
    return result
