"""Bounded image input to the existing PairedHost lease-authenticated live route.

The host owns the lease, agent, channel, admission and native content commit.
An HTTP response from this module is not a content-commit or delivery receipt.
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable
from enum import Enum
import json
import math
import re
from typing import Any
from urllib.parse import urlsplit

import anyio
import httpx


class ImageFailure(str, Enum):
    LEASE_UNAVAILABLE = "lease_unavailable"
    REDIRECT = "redirect"
    HTTP_STATUS = "http_status"
    INVALID_RESPONSE = "invalid_response"
    RESPONSE_TOO_LARGE = "response_too_large"
    TIMEOUT = "timeout"
    TRANSPORT = "transport"
    CLIENT = "client"


class ImageSubmissionError(Exception):
    """Safe metadata only; entered requests have an unconfirmed effect.

    No raw provider body, URL, authorization, image, or underlying exception is
    retained. The observation never authorizes automatic retransmission.
    """

    def __init__(
        self, kind: ImageFailure, *, request_entered: bool, status: int | None = None
    ) -> None:
        self.kind = kind
        self.request_entered = request_entered
        self.status = status
        super().__init__(f"PairedHost image input: {kind.value}")


_AUTHORIZATION = re.compile(r"PairedHostLease [A-Za-z0-9\-._~+/]{32,512}={0,2}\Z")
_ROUTE = "/api/paired_host/robot/live/send_image"


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def _number(value: str) -> float:
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("non-finite JSON number")
    return number


def _constant(_value: str) -> None:
    raise ValueError("non-finite JSON constant")


def _positive_integer(value: object, label: str) -> int:
    if type(value) is not int or value <= 0:
        raise ValueError(f"{label} must be a positive integer")
    return value


class PairedHostLiveImageClient:
    """One selected loopback route, with current authorization supplied by its host.

    No construction or import effect opens a connection or live session. Each
    explicit submission owns an isolated HTTPX client with no ambient proxy,
    authentication hook, cookies or redirect handling. The operation deadline
    covers request/body work; cooperative response/client cleanup is joined and
    can finish after that deadline. Uncooperative transport cannot be preempted.
    """

    def __init__(
        self,
        base_url: str,
        authorization_provider: Callable[[], str | None],
        *,
        timeout_seconds: float = 30.0,
        max_request_bytes: int = 8_388_608,
        max_response_bytes: int = 65_536,
    ) -> None:
        if not isinstance(base_url, str) or any(
            ord(char) <= 32 or ord(char) >= 127 for char in base_url
        ):
            raise ValueError("base_url must be an explicit loopback HTTP origin")
        try:
            parsed = urlsplit(base_url)
            port = parsed.port
        except ValueError:
            raise ValueError(
                "base_url must be an explicit loopback HTTP origin"
            ) from None
        if (
            parsed.scheme != "http"
            or parsed.hostname not in {"127.0.0.1", "::1"}
            or port is None
            or not 1 <= port <= 65535
            or parsed.username is not None
            or parsed.password is not None
            or parsed.path not in {"", "/"}
            or "?" in base_url
            or "#" in base_url
            or "\\" in base_url
        ):
            raise ValueError("base_url must be an explicit loopback HTTP origin")
        if not callable(authorization_provider):
            raise TypeError("authorization_provider must be callable")
        if (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (float, int))
            or not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        self._url = base_url.rstrip("/") + _ROUTE
        self._authorization_provider = authorization_provider
        self._timeout = float(timeout_seconds)
        self._max_request = _positive_integer(max_request_bytes, "max_request_bytes")
        self._max_response = _positive_integer(max_response_bytes, "max_response_bytes")

    async def submit_image(
        self, *, idempotency_key: str, mime_type: str, data_b64: str
    ) -> dict[str, Any]:
        """Submit exactly once and return validated HTTP JSON, never a commit.

        The caller supplies its existing job key and must separately observe the
        native user_content_committed event for that job and channel generation.
        Cancellation is propagated only after entered transport cleanup is joined.
        """
        if not isinstance(idempotency_key, str) or not idempotency_key:
            raise ValueError("idempotency_key must be nonempty text")
        if not isinstance(mime_type, str) or mime_type not in {
            "image/jpeg",
            "image/png",
        }:
            raise ValueError("mime_type must be image/jpeg or image/png")
        if not isinstance(data_b64, str) or not data_b64:
            raise ValueError("data_b64 must be nonempty text")
        if len(data_b64) + len(idempotency_key) > self._max_request:
            raise ValueError("image request exceeds its byte bound")
        try:
            body = json.dumps(
                {
                    "data": data_b64,
                    "idempotency_key": idempotency_key,
                    "mime": mime_type,
                },
                allow_nan=False,
                separators=(",", ":"),
                ensure_ascii=True,
            ).encode("ascii")
        except (ValueError, TypeError):
            raise ValueError("image request is not JSON encodable") from None
        if len(body) > self._max_request:
            raise ValueError("image request exceeds its byte bound")
        authorization: object = None
        try:
            authorization = self._authorization_provider()
        except Exception:
            pass
        if not isinstance(authorization, str) or not _AUTHORIZATION.fullmatch(
            authorization
        ):
            raise ImageSubmissionError(
                ImageFailure.LEASE_UNAVAILABLE, request_entered=False
            )

        scope = anyio.CancelScope(deadline=anyio.current_time() + self._timeout)
        owner = asyncio.create_task(self._request(body, authorization, scope))
        cancelled: asyncio.CancelledError | None = None
        while not owner.done():
            try:
                await asyncio.shield(owner)
            except asyncio.CancelledError as error:
                if cancelled is None:
                    cancelled = error
                scope.cancel()
            except Exception:
                break
        if cancelled is not None:
            try:
                owner.result()
            except (Exception, asyncio.CancelledError):
                pass
            raise cancelled from None
        return owner.result()

    async def _request(
        self, body: bytes, authorization: str, scope: anyio.CancelScope
    ) -> dict[str, Any]:
        entered = False
        response: httpx.Response | None = None
        client: httpx.AsyncClient | None = None
        failure: ImageSubmissionError | None = None
        payload: dict[str, Any] = {}
        with scope:
            try:
                client = httpx.AsyncClient(
                    trust_env=False, follow_redirects=False, timeout=self._timeout
                )
                request = client.build_request(
                    "POST",
                    self._url,
                    content=body,
                    headers={
                        "Authorization": authorization,
                        "Content-Type": "application/json",
                        "Accept": "application/json",
                        "Accept-Encoding": "identity",
                    },
                )
                entered = True
                response = await client.send(
                    request, stream=True, follow_redirects=False
                )
                failure, payload = await self._read(response)
            except (TimeoutError, httpx.TimeoutException):
                failure = ImageSubmissionError(
                    ImageFailure.TIMEOUT, request_entered=entered
                )
            except httpx.HTTPError:
                failure = ImageSubmissionError(
                    ImageFailure.TRANSPORT, request_entered=entered
                )
            except Exception:
                failure = ImageSubmissionError(
                    ImageFailure.CLIENT, request_entered=entered
                )
            finally:
                # Raw Task.cancel never reaches this owner. AnyIO cancellation
                # respects these cleanup shields, including on repeat cancel.
                with anyio.CancelScope(shield=True):
                    for resource in (response, client):
                        if resource is not None:
                            try:
                                await resource.aclose()
                            except Exception:
                                if failure is None:
                                    failure = ImageSubmissionError(
                                        ImageFailure.CLIENT, request_entered=entered
                                    )
        if scope.cancel_called and failure is None:
            failure = ImageSubmissionError(
                ImageFailure.TIMEOUT, request_entered=entered
            )
        if failure is not None:
            raise failure from None
        return payload

    async def _read(
        self, response: httpx.Response
    ) -> tuple[ImageSubmissionError | None, dict[str, Any]]:
        def failed(kind: ImageFailure) -> tuple[ImageSubmissionError, dict[str, Any]]:
            return ImageSubmissionError(
                kind, request_entered=True, status=response.status_code
            ), {}

        if 300 <= response.status_code < 400:
            return failed(ImageFailure.REDIRECT)
        if response.status_code != 200:
            return failed(ImageFailure.HTTP_STATUS)
        if (
            response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
            != "application/json"
            or response.headers.get("content-encoding", "identity").lower()
            != "identity"
        ):
            return failed(ImageFailure.INVALID_RESPONSE)
        size = response.headers.get("content-length")
        if size is not None:
            if not size.isascii() or not size.isdecimal():
                return failed(ImageFailure.INVALID_RESPONSE)
            if len(size) > 12 or int(size) > self._max_response:
                return failed(ImageFailure.RESPONSE_TOO_LARGE)
        body = bytearray()
        async for chunk in response.aiter_raw():
            if len(body) + len(chunk) > self._max_response:
                return failed(ImageFailure.RESPONSE_TOO_LARGE)
            body.extend(chunk)
        try:
            value = json.loads(
                body.decode("utf-8"),
                object_pairs_hook=_object,
                parse_float=_number,
                parse_constant=_constant,
            )
            if not isinstance(value, dict) or "error" in value:
                return failed(ImageFailure.INVALID_RESPONSE)
        except (ValueError, RecursionError):
            return failed(ImageFailure.INVALID_RESPONSE)
        return None, value


__all__ = ["PairedHostLiveImageClient", "ImageFailure", "ImageSubmissionError"]
