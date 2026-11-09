"""Run meta-forge in-process for tests: injected streams, environment and HTTP transport.

Polling runs with no delay (poll_scale=0). The CLI's environment is its own
dictionary, separate from the test process's, so a server running in this
process never shares the CLI's environment by accident.
"""
from __future__ import annotations

import io
import json

import httpx

from launchkit.cli import run_async

GUIDE_ID = "67f2f55d-8fd4-4d92-b999-14fb240e5730"
OTHER_ID = "67f2f55d-1111-4d92-b999-14fb240e5731"
DEPLOYMENT_ID = "6a0e5c1d-2b3c-4d5e-8f90-a1b2c3d4e5f6"


class Recording(httpx.AsyncBaseTransport):
    """Forward to the Forge app in this process; record each request; optionally lose answers.

    A lost route is handled by the server first, then its answer is replaced by
    a read timeout, as when a connection drops after the server acted.
    """

    def __init__(self, app, *, lose=()):
        self.inner = httpx.ASGITransport(app=app)
        self.lose = set(lose)
        self.requests = []

    async def handle_async_request(self, request):
        try:
            content = request.content
        except httpx.RequestNotRead:
            content = None
        body = json.loads(content) if content and request.headers.get("content-type") == "application/json" else None
        self.requests.append((request.method, request.url.path, dict(request.url.params), body))
        response = await self.inner.handle_async_request(request)
        if (request.method, request.url.path) in self.lose:
            await response.aread()
            self.lose.discard((request.method, request.url.path))
            raise httpx.ReadTimeout("The answer was lost after the server handled the request.", request=request)
        return response

    def sent(self, method, path):
        return [body for seen, route, _, body in self.requests if (seen, route) == (method, path)]


class Terminal(io.StringIO):
    """A stream that reports itself as a terminal."""

    def isatty(self):
        return True


class Run:
    def __init__(self, code, stdout, stderr):
        self.code, self.stdout, self.stderr = code, stdout, stderr

    @property
    def document(self):
        return json.loads(self.stdout)

    @property
    def events(self):
        return [json.loads(line) for line in self.stdout.splitlines() if line.strip()]

    @property
    def text(self):
        return self.stdout + self.stderr


async def cli(argv, *, transport=None, environ=None, stdin=None, terminal=False, executor=None):
    stdout = Terminal() if terminal else io.StringIO()
    stderr = Terminal() if terminal else io.StringIO()
    if stdin is None:
        stdin = Terminal() if terminal else io.StringIO()
    code = await run_async(list(argv), stdin=stdin, stdout=stdout, stderr=stderr,
                           environ={"META_FORGE_PORT": "8787"} if environ is None else environ,
                           transport=transport, poll_scale=0, executor=executor)
    return Run(code, stdout.getvalue(), stderr.getvalue())
