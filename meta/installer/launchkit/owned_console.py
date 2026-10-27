"""Browser access to the native Console of a live Forge-owned runtime."""
from __future__ import annotations

import asyncio
from contextlib import suppress
from dataclasses import dataclass
import html
import re
from urllib.parse import urlsplit
import uuid

import httpx
from fastapi import HTTPException
from fastapi.responses import HTMLResponse, StreamingResponse

from .implementation import read_receipt


MAX_BODY = 8_000_000
MAX_EVENT_CURSOR = 4096
_KINDS = {"implementation": "build_id", "rehearsal": "rehearsal_id"}
_PUBLIC_HEADERS = {"Cache-Control": "no-store", "Referrer-Policy": "no-referrer",
                   "X-Content-Type-Options": "nosniff", "Content-Security-Policy": "frame-ancestors 'self'"}


def _console_origin(address):
    if not isinstance(address, str) or not re.fullmatch(r"http://127\.0\.0\.1:[0-9]+/console", address):
        return None
    parsed = urlsplit(address)
    if parsed.port is None or not 1024 <= parsed.port <= 65535:
        return None
    return f"http://127.0.0.1:{parsed.port}"


@dataclass(frozen=True)
class GuideConsoleOwner:
    service: object
    runtime: object
    generation: str
    origin: str

    @property
    def prefix(self):
        return f"/api/owned-console/guide/{self.generation}"

    def current(self):
        current = resolve_guide_owner(self.service, self.generation)
        return current is not None and current.runtime is self.runtime and current.origin == self.origin


def resolve_guide_owner(service, generation):
    """A guide link belongs to one live SDK runtime generation, never a port alone."""
    try:
        if str(uuid.UUID(generation)) != generation or service.console_generation != generation:
            return None
        runtime = service.runtime
        if (runtime is None or runtime.is_running is not True
                or service.provider_stop_failed or service.provider_paused):
            return None
        origin = _console_origin(runtime.rust_http_base_url + "/console")
        return GuideConsoleOwner(service, runtime, generation, origin) if origin else None
    except (ValueError, TypeError, AttributeError):
        return None


def guide_browser_console_url(service):
    owner = resolve_guide_owner(service, getattr(service, "console_generation", None))
    return owner.prefix + "/console" if owner else None


@dataclass(frozen=True)
class ConsoleOwner:
    service: object
    guide_id: str
    job_id: str
    kind: str
    process: object
    task: object
    origin: str

    @property
    def prefix(self):
        return f"/api/owned-console/{self.kind}/{self.guide_id}/{self.job_id}"

    def current(self):
        current = resolve_owner(self.service, self.guide_id, self.job_id, self.kind)
        return (current is not None and current.process is self.process
                and current.task is self.task and current.origin == self.origin)


def resolve_owner(service, guide_id, job_id, kind):
    """Receipts supply an address only while the original host owns the worker."""
    if kind not in _KINDS:
        return None
    try:
        if any(str(uuid.UUID(value)) != value for value in (guide_id, job_id)):
            return None
        directory = service._directory(guide_id, job_id)
        if service._directory(guide_id) != directory:
            return None
        process = service.processes.get(job_id)
        task = service.tasks.get(job_id)
        if process is None or process.returncode is not None or task is None or task.done():
            return None
        receipt = read_receipt(directory)
        if (receipt.get(_KINDS[kind]) != job_id or receipt.get("guide_id") != guide_id
                or receipt.get("worker_stop_confirmed") is True
                or receipt.get("cleanup_pending") or receipt.get("stop_requested")):
            return None
        origin = _console_origin(receipt.get("native_console_url"))
        if origin is None:
            return None
        return ConsoleOwner(service, guide_id, job_id, kind, process, task, origin)
    except (LookupError, ValueError, OSError, TypeError, AttributeError):
        return None


async def browser_console_url(service, guide_id, receipt, kind):
    owner = await asyncio.to_thread(resolve_owner, service, guide_id, receipt.get(_KINDS[kind]), kind)
    return owner.prefix + "/console" if owner else None


def _console_html(prefix):
    # The existing Console browser entry reads this meta tag for every API call.
    # Both spellings cover the selected Meta and predecessor MobKit bundles.
    base = html.escape(prefix, quote=True)
    return ("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">"
            '<meta name="viewport" content="width=device-width, initial-scale=1">'
            f'<meta name="meta-base-url" content="{base}">'
            f'<meta name="mobkit-base-url" content="{base}">'
            '<title>Forge agent Console</title>'
            f'<link rel="stylesheet" href="{base}/console/assets/console-app.css">'
            '</head><body><div id="root"></div>'
            f'<script src="{base}/console/assets/console-app.js" defer></script></body></html>')


def _request_allowed(request, path):
    origin = request.headers.get("origin")
    if (origin and origin != f"{request.url.scheme}://{request.url.netloc}"
            or request.headers.get("sec-fetch-site") == "cross-site"):
        raise HTTPException(403, "Open this Console through its Forge setup.")
    if (request.method not in {"GET", "HEAD", "POST"}
            or not (path in {"console", "console/", "favicon.ico"}
                    or path.startswith(("console/", "blobs/")))
            or any(part in {".", ".."} for part in path.split("/"))
            or any(character in path for character in ("\\", "%", "?", "#"))):
        raise HTTPException(404, "Unknown Console route.")


async def console_response(request, owner, path, *, transport=None):
    _request_allowed(request, path)
    if owner is None or not await asyncio.to_thread(owner.current):
        raise HTTPException(410, "This agent Console has closed. Return to the saved build or trial.")
    if path in {"console", "console/"}:
        if request.method not in {"GET", "HEAD"}:
            raise HTTPException(405, "Open the Console with GET.")
        return HTMLResponse("" if request.method == "HEAD" else _console_html(owner.prefix), headers=_PUBLIC_HEADERS)
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > MAX_BODY:
            raise HTTPException(413, "This Console request exceeds the size limit.")
    try:
        query = request.scope.get("query_string", b"").decode("ascii")
    except UnicodeDecodeError:
        raise HTTPException(400, "Invalid Console query.") from None
    if len(query) > 16_384:
        raise HTTPException(400, "The Console query exceeds its size limit.")
    cursor = request.headers.get("last-event-id")
    if cursor is not None and (len(cursor) > MAX_EVENT_CURSOR
            or any(not 32 <= ord(character) <= 126 for character in cursor)):
        raise HTTPException(400, "Invalid Console event cursor.")
    if not await asyncio.to_thread(owner.current):
        raise HTTPException(410, "This agent Console has closed.")
    url = owner.origin + "/" + path + ("?" + query if query else "")
    headers = {name: request.headers[name] for name in ("accept", "content-type", "range", "last-event-id")
               if name in request.headers}
    # Authoring cookies, bearer credentials and forwarded-host headers stay here.
    client = httpx.AsyncClient(trust_env=False, follow_redirects=False, transport=transport,
                               timeout=httpx.Timeout(None, connect=5, write=30, pool=5))
    upstream = None
    try:
        async with asyncio.timeout(20):
            upstream = await client.send(client.build_request(request.method, url, content=bytes(body), headers=headers), stream=True)
        if not await asyncio.to_thread(owner.current):
            raise HTTPException(410, "This agent Console has closed.")
        if 300 <= upstream.status_code < 400:
            raise HTTPException(502, "The agent Console returned an unsupported redirect.")
    except BaseException as error:
        if upstream is not None:
            await upstream.aclose()
        await client.aclose()
        if isinstance(error, TimeoutError):
            raise HTTPException(504, "The agent Console did not respond.") from None
        if isinstance(error, httpx.HTTPError):
            raise HTTPException(502, "The agent Console is unavailable.") from None
        raise
    forwarded = {name: upstream.headers[name] for name in ("content-type", "content-range", "accept-ranges")
                 if name in upstream.headers}
    forwarded.update(_PUBLIC_HEADERS)
    if "text/event-stream" in upstream.headers.get("content-type", ""):
        forwarded["X-Accel-Buffering"] = "no"

    async def stream():
        iterator = upstream.aiter_bytes()
        pending = None
        try:
            while await asyncio.to_thread(owner.current):
                if pending is None:
                    pending = asyncio.create_task(anext(iterator))
                done, _ = await asyncio.wait({pending}, timeout=0.5)
                if not done:
                    continue
                try:
                    chunk = pending.result()
                except StopAsyncIteration:
                    return
                pending = None
                if not await asyncio.to_thread(owner.current):
                    return
                yield chunk
        except httpx.HTTPError:
            # A broken stream is a disconnect, never a fabricated terminal event.
            return
        finally:
            if pending is not None:
                pending.cancel()
                with suppress(asyncio.CancelledError, Exception):
                    await pending
            await upstream.aclose()
            await client.aclose()

    return StreamingResponse(stream(), status_code=upstream.status_code, headers=forwarded)
