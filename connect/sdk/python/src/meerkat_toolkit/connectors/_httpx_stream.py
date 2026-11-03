"""Retained custody for HTTPX's automatic or explicit response-stream close."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

import anyio
import httpx


class HttpxClientNotPrepared(ValueError):
    """Local client configuration refusal; no request has been attempted."""

    def __init__(self) -> None:
        super().__init__(
            "prepare_httpx_client must run before use; its response hook must remain first"
        )

    @property
    def request_attempted(self) -> bool:
        return False


def _protect_response(response: httpx.Response) -> None:
    if not isinstance(response.stream, JoinedResponseStream):
        response.stream = JoinedResponseStream(response.stream)


async def _retain_response_custody(response: httpx.Response) -> None:
    # No suspension: custody is installed before the next host hook can run.
    _protect_response(response)


def prepare_httpx_client(client: httpx.AsyncClient) -> httpx.AsyncClient:
    """Install response custody during host composition, before client use.

    This mutates only the response-hook list, retaining existing hook order.
    Repeating preparation is harmless. The same client is returned; the host
    still owns auth, routing, transport configuration and eventual closure.
    Do not change hook ordering or replace response streams during a borrow.
    """
    if not isinstance(client, httpx.AsyncClient):
        raise TypeError("expected an HTTPX AsyncClient")
    if client.is_closed:
        raise ValueError("cannot prepare a closed HTTPX client")
    hooks = client.event_hooks.setdefault("response", [])
    hooks[:] = [_retain_response_custody] + [
        hook for hook in hooks if hook is not _retain_response_custody
    ]
    return client


def require_prepared_httpx_client(client: httpx.AsyncClient) -> None:
    hooks = client.event_hooks.get("response", [])
    if not hooks or hooks[0] is not _retain_response_custody:
        raise HttpxClientNotPrepared()


async def send_with_custody(
    client: httpx.AsyncClient,
    request: httpx.Request,
    *,
    auth: httpx.Auth | None = None,
) -> httpx.Response:
    """Retain one send and join its unwind before propagating cancellation.

    A response hook runs before HTTPX returns the response. Isolating send from
    an outer AnyIO level-cancellation scope lets HTTPX finish cleanup entered
    when a hook receives cancellation. Repeated caller cancellation never
    cancels the send owner a second time.
    """
    require_prepared_httpx_client(client)
    owner = asyncio.create_task(
        client.send(
            request,
            stream=True,
            follow_redirects=False,
            auth=auth if auth is not None else httpx.USE_CLIENT_DEFAULT,
        )
    )
    try:
        response = await asyncio.shield(owner)
    except HttpxClientNotPrepared:
        # Only our validator before task creation proves no request entry.
        # Host code may raise the same public exception after sending.
        raise RuntimeError("HTTPX client configuration refused after send entry") from None
    except asyncio.CancelledError:
        with anyio.CancelScope(shield=True):
            if not owner.done():
                owner.cancel()
            while not owner.done():
                try:
                    await asyncio.shield(owner)
                except (asyncio.CancelledError, Exception):
                    pass
            try:
                response = owner.result()
            except (asyncio.CancelledError, Exception):
                pass
            else:
                # Cancellation can race with send returning a response. That
                # response remains ours until its close has completed.
                _protect_response(response)
                try:
                    await response.aclose()
                except (asyncio.CancelledError, Exception):
                    pass
        raise
    _protect_response(response)
    return response


class JoinedResponseStream(httpx.AsyncByteStream):
    """Join one underlying close despite deadline or repeated caller cancellation.

    HTTPX marks a response closed before awaiting its stream. Retaining the
    underlying close here covers iteration's implicit close as well as explicit
    response cleanup; a second response.aclose() alone cannot do that.
    """

    def __init__(self, stream: httpx.AsyncByteStream) -> None:
        self._stream = stream
        self._closing: asyncio.Task[None] | None = None

    async def __aiter__(self) -> AsyncIterator[bytes]:
        async for chunk in self._stream:
            yield chunk

    async def aclose(self) -> None:
        with anyio.CancelScope(shield=True):
            if self._closing is None:
                self._closing = asyncio.create_task(self._stream.aclose())
            cancellation: asyncio.CancelledError | None = None
            while not self._closing.done():
                try:
                    await asyncio.shield(self._closing)
                except asyncio.CancelledError as exc:
                    cancellation = exc
                except Exception:
                    break
            try:
                self._closing.result()
            except Exception:
                if cancellation is None:
                    raise
            if cancellation is not None:
                raise cancellation
