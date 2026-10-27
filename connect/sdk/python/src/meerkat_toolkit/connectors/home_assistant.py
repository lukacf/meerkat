"""Home Assistant REST operations on a borrowed, host-authenticated HTTP client.

The host selects the instance, credential and allowed operations. This module
does not infer authority, retry a request, follow redirects or own the client.
HA can keep a service running after disconnect: all failures after a service
request is attempted have an unknown write outcome, including HTTP refusals.
"""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from dataclasses import dataclass
from enum import Enum
import json
import math
import re
from typing import Any

import httpx

from ._httpx_stream import (
    HttpxClientNotPrepared,
    require_prepared_httpx_client,
    send_with_custody,
)


class FailureKind(str, Enum):
    AUTHENTICATION = "authentication"
    PERMISSION = "permission"
    NOT_FOUND = "not_found"
    RATE_LIMITED = "rate_limited"
    REDIRECT = "redirect"
    HTTP_STATUS = "http_status"
    INVALID_RESPONSE = "invalid_response"
    RESPONSE_TOO_LARGE = "response_too_large"
    UNSUPPORTED_ENCODING = "unsupported_encoding"
    TIMEOUT = "timeout"
    TRANSPORT = "transport"
    CLIENT = "client"


class WriteOutcome(str, Enum):
    NOT_APPLICABLE = "not_applicable"
    UNKNOWN = "unknown"


class HomeAssistantError(Exception):
    """Safe metadata only, never a provider body, credential or request URL.

    ``retry_after_seconds`` preserves a decimal Retry-After value when valid.
    It is information, not permission to retry an operation with unknown outcome.
    """

    def __init__(
        self,
        kind: FailureKind,
        *,
        write_outcome: WriteOutcome,
        status_code: int | None = None,
        retry_after_seconds: int | None = None,
    ) -> None:
        self.kind = kind
        self.write_outcome = write_outcome
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds
        status = "" if status_code is None else f" (HTTP {status_code})"
        super().__init__(
            f"Home Assistant {kind.value}{status}; write outcome: {write_outcome.value}"
        )


class ServiceCallCancelled(asyncio.CancelledError):
    """Cancellation is preserved; the remote service may still be running."""

    write_outcome = WriteOutcome.UNKNOWN

    def __init__(self) -> None:
        super().__init__("Home Assistant service call cancelled; write outcome: unknown")


@dataclass(frozen=True)
class CameraImage:
    """Exact camera bytes and the provider's normalized image media type.

    This is a response receipt, not decoded image validation or scene truth.
    """

    data: bytes
    content_type: str


@dataclass(frozen=True)
class ServiceCallResult:
    """Provider receipt, not proof of a physical device's final state.

    Changed states retain provider order and all fields. They may be empty.
    ``response_requested`` distinguishes an absent response from a requested
    response whose JSON value is null. Returned JSON containers remain mutable.
    """

    changed_states: list[dict[str, Any]]
    response_requested: bool
    service_response: dict[str, Any] | None


_SEGMENT = re.compile(r"[a-z0-9_]+\Z")


def _segment(value: str) -> str:
    # Supported profile: canonical HA names, no normalization or URL escape hatch.
    if not isinstance(value, str) or not _SEGMENT.fullmatch(value):
        raise ValueError("expected a canonical Home Assistant identifier")
    return value


def _entity(value: str) -> str:
    if not isinstance(value, str) or value.count(".") != 1:
        raise ValueError("expected a canonical Home Assistant entity identifier")
    domain, object_id = value.split(".")
    _segment(domain)
    _segment(object_id)
    return value


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def _number(value: str) -> float:
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("non-finite JSON number")
    return number


def _constant(_value: str) -> Any:
    raise ValueError("non-JSON numeric constant")


def _json_keys(value: Any) -> None:
    # json.dumps otherwise silently coerces integer/bool/null object keys.
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise ValueError("JSON object keys must be strings")
            _json_keys(item)
    elif isinstance(value, (list, tuple)):
        for item in value:
            _json_keys(item)


def _state(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("entity_id"), str)
        and isinstance(value.get("state"), str)
        and isinstance(value.get("attributes"), dict)
    )


class HomeAssistantOperations:
    """Small REST profile: entity read, service discovery and service invocation.

    ``client.base_url`` is captured once, including a reverse-proxy path prefix.
    Client default query parameters are deliberately not inherited. Per-call
    redirects are disabled and response decoding is bounded and identity-only.
    The host must supply an authenticated client whose auth, hooks and transport
    do not retarget or replay requests. Those host-owned behaviors are opaque to
    this module. A default ``httpx.AsyncClient`` transport has no retry loop.
    """

    def __init__(
        self,
        client: httpx.AsyncClient,
        *,
        timeout_seconds: float = 20.0,
        max_response_bytes: int = 1_048_576,
        max_media_bytes: int | None = None,
        base_url: str | httpx.URL | None = None,
        auth: httpx.Auth | None = None,
    ) -> None:
        base = client.base_url if base_url is None else httpx.URL(base_url)
        if (
            base.scheme not in {"http", "https"}
            or not base.host
            or base.username
            or base.password
            or base.query
            or base.fragment
        ):
            raise ValueError("client must select one HTTP(S) base URL without userinfo or query")
        if (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        if type(max_response_bytes) is not int or max_response_bytes <= 0:
            raise ValueError("max_response_bytes must be a positive integer")
        self._client = client
        self._base = base.copy_with(raw_path=base.raw_path.rstrip(b"/") + b"/")
        self._auth = auth
        if max_media_bytes is not None and (
            type(max_media_bytes) is not int or max_media_bytes <= 0
        ):
            raise ValueError("max_media_bytes must be a positive integer or None")
        self._max_media_bytes = max_media_bytes
        self._timeout = float(timeout_seconds)
        self._max_bytes = max_response_bytes

    def _failure(
        self,
        kind: FailureKind,
        *,
        write: bool,
        response: httpx.Response | None = None,
    ) -> HomeAssistantError:
        retry_after = None
        if response is not None:
            value = response.headers.get("retry-after", "")
            if value.isascii() and value.isdecimal() and len(value) <= 9:
                retry_after = int(value)
        return HomeAssistantError(
            kind,
            write_outcome=WriteOutcome.UNKNOWN if write else WriteOutcome.NOT_APPLICABLE,
            status_code=None if response is None else response.status_code,
            retry_after_seconds=retry_after,
        )

    async def _json(
        self,
        method: str,
        path: str,
        *,
        write: bool = False,
        content: bytes | None = None,
        response_requested: bool = False,
        media: bool = False,
    ) -> Any:
        require_prepared_httpx_client(self._client)
        url = self._base.join(path)
        if response_requested:
            url = url.copy_with(query=b"return_response")
        failure: BaseException | None = None
        response: httpx.Response | None = None
        payload: Any = None
        limit = self._max_media_bytes if media else self._max_bytes
        try:
            # One deadline covers send and the entire streamed response. On
            # expiry response closure is still awaited; no detached cleanup task.
            async with asyncio.timeout(self._timeout):
                request = self._client.build_request(
                    method,
                    url,
                    content=content,
                    headers={
                        "Accept": "image/*" if media else "application/json",
                        "Accept-Encoding": "identity",
                        **({"Content-Type": "application/json"} if content is not None else {}),
                    },
                    timeout=self._timeout,
                )
                # build_request merges client.params; that must not change the
                # selected operation, especially return_response's presence.
                request.url = url
                for name in tuple(request.headers):
                    if name.lower() in {
                        "range",
                        "if-range",
                        "if-match",
                        "if-none-match",
                        "if-modified-since",
                        "if-unmodified-since",
                        "x-http-method-override",
                        "x-method-override",
                    }:
                        del request.headers[name]
                response = await send_with_custody(self._client, request, auth=self._auth)
                try:
                    if response.status_code != 200:
                        kind = {
                            401: FailureKind.AUTHENTICATION,
                            403: FailureKind.PERMISSION,
                            404: FailureKind.NOT_FOUND,
                            429: FailureKind.RATE_LIMITED,
                        }.get(response.status_code, FailureKind.HTTP_STATUS)
                        if 300 <= response.status_code < 400:
                            kind = FailureKind.REDIRECT
                        failure = self._failure(kind, write=write, response=response)
                    elif response.headers.get("content-encoding", "identity").lower() != "identity":
                        failure = self._failure(
                            FailureKind.UNSUPPORTED_ENCODING, write=write, response=response
                        )
                    else:
                        types = response.headers.get_list("content-type")
                        mime = types[0].split(";", 1)[0].strip().lower() if len(types) == 1 else ""
                        valid_type = (
                            bool(re.fullmatch(r"image/[a-z0-9!#$&^_.+-]+", mime))
                            if media
                            else mime == "application/json"
                        )
                        sizes = response.headers.get_list("content-length")
                        size = sizes[0] if len(sizes) == 1 else None
                        if (
                            not valid_type
                            or len(sizes) > 1
                            or (
                                size is not None
                                and (not size.isascii() or not size.isdecimal() or len(size) > 20)
                            )
                        ):
                            failure = self._failure(
                                FailureKind.INVALID_RESPONSE, write=write, response=response
                            )
                        elif size is not None and limit is not None and int(size) > limit:
                            failure = self._failure(
                                FailureKind.RESPONSE_TOO_LARGE, write=write, response=response
                            )
                        else:
                            body = bytearray()
                            chunks = (
                                response.aiter_bytes()
                                if response.is_stream_consumed
                                else response.aiter_raw()
                            )
                            async for chunk in chunks:
                                if limit is not None and len(body) + len(chunk) > limit:
                                    failure = self._failure(
                                        FailureKind.RESPONSE_TOO_LARGE,
                                        write=write,
                                        response=response,
                                    )
                                    break
                                body.extend(chunk)
                            if failure is None:
                                if (size is not None and len(body) != int(size)) or (
                                    media and not body
                                ):
                                    failure = self._failure(
                                        FailureKind.INVALID_RESPONSE, write=write, response=response
                                    )
                                elif media:
                                    payload = CameraImage(bytes(body), mime)
                                else:
                                    try:
                                        payload = json.loads(
                                            body.decode("utf-8"),
                                            object_pairs_hook=_object,
                                            parse_float=_number,
                                            parse_constant=_constant,
                                        )
                                    except (ValueError, RecursionError):
                                        failure = self._failure(
                                            FailureKind.INVALID_RESPONSE,
                                            write=write,
                                            response=response,
                                        )
                finally:
                    await response.aclose()
        except HttpxClientNotPrepared:
            failure = (
                HttpxClientNotPrepared()
                if response is None
                else self._failure(FailureKind.CLIENT, write=write)
            )
        except asyncio.CancelledError:
            failure = ServiceCallCancelled() if write else asyncio.CancelledError()
        except (TimeoutError, httpx.TimeoutException):
            failure = self._failure(FailureKind.TIMEOUT, write=write)
        except httpx.HTTPError:
            failure = self._failure(FailureKind.TRANSPORT, write=write)
        except Exception:
            # Host auth/hook errors may contain secrets too. Do not retain their
            # exception object or raise inside its handler (implicit context).
            failure = self._failure(FailureKind.CLIENT, write=write)
        if failure is not None:
            raise failure from None
        return payload

    async def get_state(self, entity_id: str) -> dict[str, Any]:
        """Read exactly one entity; preserve its nested attributes and metadata."""
        entity_id = _entity(entity_id)
        result = await self._json("GET", f"api/states/{entity_id}")
        if not _state(result) or result["entity_id"] != entity_id:
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        return result

    async def list_states(self) -> list[dict[str, Any]]:
        """Read one complete states response; selection and policy remain host-owned."""
        result = await self._json("GET", "api/states")
        if not isinstance(result, list) or any(not _state(row) for row in result):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        return result

    async def get_camera_image(self, entity_id: str) -> CameraImage:
        """Read one current still. No JPEG assumption, decoding or camera selection.

        The default media body has no size cap, preserving existing still-image
        use. Hosts may select max_media_bytes; the JSON body limit is separate.
        The one-operation deadline still covers send, reading and joined close.
        """
        entity_id = _entity(entity_id)
        if not entity_id.startswith("camera."):
            raise ValueError("expected a camera entity identifier")
        return await self._json("GET", f"api/camera_proxy/{entity_id}", media=True)

    async def list_services(self) -> list[dict[str, Any]]:
        """Read the complete advertised service metadata without flattening it."""
        result = await self._json("GET", "api/services")
        if not isinstance(result, list) or any(
            not isinstance(row, dict)
            or not isinstance(row.get("domain"), str)
            or not isinstance(row.get("services"), dict)
            for row in result
        ):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=False)
        return result

    async def call_service(
        self,
        domain: str,
        service: str,
        *,
        data: Mapping[str, Any] | None = None,
        return_response: bool = False,
    ) -> ServiceCallResult:
        """Invoke once; a failure does not establish that no effect occurred.

        Domain, service, return_response and finite JSON data are checked before
        transport entry. This supported profile does not support noncanonical
        names. Authorization, entity selection and retries remain host decisions.
        """
        domain, service = _segment(domain), _segment(service)
        if type(return_response) is not bool:
            raise ValueError("return_response must be boolean")
        if data is not None and not isinstance(data, Mapping):
            raise ValueError("service data must be a JSON object")
        content = None
        try:
            value = {} if data is None else dict(data)
            _json_keys(value)
            content = json.dumps(value, allow_nan=False, ensure_ascii=False).encode("utf-8")
        except (TypeError, ValueError, OverflowError, RecursionError):
            pass
        if content is None:
            raise ValueError("service data must be a finite JSON object") from None
        result = await self._json(
            "POST",
            f"api/services/{domain}/{service}",
            write=True,
            content=content,
            response_requested=return_response,
        )
        service_response = None
        changed_states = result
        if return_response:
            if not isinstance(result, dict) or not {"changed_states", "service_response"}.issubset(
                result
            ):
                raise self._failure(FailureKind.INVALID_RESPONSE, write=True)
            changed_states = result["changed_states"]
            service_response = result["service_response"]
            if service_response is not None and not isinstance(service_response, dict):
                raise self._failure(FailureKind.INVALID_RESPONSE, write=True)
        if not isinstance(changed_states, list) or any(not _state(row) for row in changed_states):
            raise self._failure(FailureKind.INVALID_RESPONSE, write=True)
        return ServiceCallResult(changed_states, return_response, service_response)
