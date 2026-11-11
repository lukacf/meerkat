"""HTTP client of the running Forge server: the same routes and bodies the browser uses.

The client never imports or runs Forge's services in its own process. It reads
no proxy, netrc or certificate settings from the environment (trust_env=False),
sends no Origin header, and talks only to a loopback address, which is all the
server's host check admits. Every request is bounded by the command's
--timeout, when it has one.
"""
from __future__ import annotations

import asyncio
import ipaddress
import re
from urllib.parse import urlsplit

from .errors import CliError, from_response, step

DEFAULT_PORT = 8787
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
PREFIX = re.compile(r"[0-9a-f]{8}[0-9a-f-]{0,28}")
READ_SECONDS = 30.0
LOOPBACK_NAMES = {"localhost"}


def default_port(environ):
    """The port start.sh serves on: META_FORGE_PORT, else MOBKIT_LAUNCH_PORT, else 8787."""
    value = environ.get("META_FORGE_PORT") or environ.get("MOBKIT_LAUNCH_PORT") or str(DEFAULT_PORT)
    if not re.fullmatch(r"[1-9][0-9]{0,4}", value) or int(value) > 65535:
        raise CliError("usage", "META_FORGE_PORT (or MOBKIT_LAUNCH_PORT) must be a port number from 1 to 65535.",
                       next=[step("meta-forge doctor", "Check the local Forge settings.")])
    return int(value)


def server_url(environ, override=None):
    """The Forge server address. Only a loopback http URL is accepted."""
    if override is None:
        return f"http://127.0.0.1:{default_port(environ)}"
    parts = urlsplit(override)
    host = parts.hostname or ""
    loopback = host in LOOPBACK_NAMES
    if not loopback:
        try:
            loopback = ipaddress.ip_address(host).is_loopback
        except ValueError:
            loopback = False
    if (parts.scheme != "http" or not loopback or parts.username or parts.password
            or parts.path not in ("", "/") or parts.query or parts.fragment):
        raise CliError("usage", "Use --server http://127.0.0.1:PORT: Forge serves only this computer's loopback "
                       "address. Over SSH, run meta-forge on the computer that runs Forge.",
                       next=[step("meta-forge doctor", "Check which Forge server answers on this computer.")])
    try:
        port = parts.port
    except ValueError:
        port = None
    if port is None:
        raise CliError("usage", "Give the Forge server's port, for example --server http://127.0.0.1:8787.",
                       next=[step("meta-forge doctor", "Check which Forge server answers on this computer.")])
    return f"http://{parts.netloc}"


def short(identity):
    return identity[:8] if isinstance(identity, str) else str(identity)


class ForgeClient:
    """One Forge server conversation for the duration of a command."""

    def __init__(self, base_url, *, transport=None, trace=None, remaining=None, timed_out=None):
        import httpx
        self.httpx = httpx
        self.base_url = base_url
        self.trace = trace
        # The command's --timeout: the seconds left, and the typed outcome when none are.
        self.remaining = remaining
        self.timed_out = timed_out
        self.http = httpx.AsyncClient(base_url=base_url, transport=transport, trust_env=False,
                                      follow_redirects=False, timeout=httpx.Timeout(READ_SECONDS, connect=10.0))
        self.capabilities = None
        self.resolved = None  # the setup this command resolved, for its next steps after an interruption

    async def close(self):
        await self.http.aclose()

    def unavailable(self):
        return CliError("server_unavailable", f"No Forge server answers at {self.base_url}.",
                        next=[step("meta-forge serve", "Start Forge from its folder in another terminal, then retry."),
                              step("meta-forge doctor", "Check the port and the local Forge setup.")])

    async def within_timeout(self, awaitable, what):
        """Await one request within the command's --timeout, when it has one (wait_timeout, exit 87)."""
        left = self.remaining() if self.remaining is not None else None
        if left is None:
            return await awaitable
        if left <= 0:
            awaitable.close()
            raise self.timed_out(what)
        try:
            return await asyncio.wait_for(awaitable, left)
        except asyncio.TimeoutError:
            raise self.timed_out(what) from None

    async def request(self, method, path, *, json=None, params=None, files=None, read_timeout=READ_SECONDS,
                      expect="json"):
        """Send one request; return (status, payload) for 2xx, else raise a typed error."""
        httpx = self.httpx
        timeout = httpx.Timeout(read_timeout, connect=10.0, write=60.0, pool=10.0)
        if self.trace:
            self.trace(f"> {method} {path}")
        try:
            response = await self.within_timeout(
                self.http.request(method, path, json=json, params=params, files=files, timeout=timeout),
                f"waiting for Forge to answer {method} {path}")
        except (httpx.ConnectError, httpx.ConnectTimeout):
            raise self.unavailable() from None
        except httpx.TimeoutException:
            raise CliError("outcome_unknown", f"The Forge server did not answer {method} {path} in time. "
                           "The request may still be running.") from None
        except httpx.TransportError:
            raise CliError("outcome_unknown", f"The connection to the Forge server ended before {method} {path} "
                           "answered. The request may have taken effect.") from None
        if self.trace:
            self.trace(f"< {response.status_code} {method} {path}")
        if response.is_redirect or response.status_code < 200:
            raise CliError("server_incompatible", f"The server at {self.base_url} is not a Forge server "
                           f"(it answered HTTP {response.status_code}).",
                           next=[step("meta-forge doctor", "Check which server answers on this port.")])
        if response.status_code >= 300:
            try:
                payload = response.json()
            except ValueError:
                payload = None
            raise from_response(response.status_code, payload)
        if expect == "bytes":
            return response.status_code, response.content
        try:
            return response.status_code, response.json()
        except ValueError:
            raise CliError("server_incompatible", f"The server at {self.base_url} answered {path} without JSON; "
                           "it is not a compatible Forge server.",
                           next=[step("meta-forge doctor", "Check which server answers on this port.")]) from None

    async def get(self, path, **options):
        return (await self.request("GET", path, **options))[1]

    async def post(self, path, **options):
        options.setdefault("read_timeout", None)
        return await self.request("POST", path, **options)

    async def check_compatible(self, cli_version):
        """Require the server of this Forge release before any other request."""
        try:
            capabilities = await self.get("/api/capabilities")
        except CliError as error:
            if error.code in ("not_found", "invalid_request"):
                raise CliError("server_incompatible", f"The server at {self.base_url} does not answer "
                               "/api/capabilities; it is not a Forge server.",
                               next=[step("meta-forge doctor", "Check which server answers on this port.")]) from None
            raise
        version = capabilities.get("version") if isinstance(capabilities, dict) else None
        if version != cli_version:
            raise CliError("server_incompatible",
                           f"This meta-forge belongs to Forge {cli_version}; the server at {self.base_url} is "
                           f"Forge {version}. Use the meta-forge in that server's own folder, or stop it and "
                           "start this folder's Forge.",
                           next=[step("meta-forge version", "Compare this command's Forge with the server's."),
                                 step("meta-forge serve", "Start this folder's Forge on a free port.")])
        self.capabilities = capabilities
        return capabilities

    async def list_setups(self, include_all=True):
        value = await self.get("/api/guide/list", params={"include_all": "true" if include_all else "false"})
        setups = value.get("setups") if isinstance(value, dict) else None
        if not isinstance(setups, list):
            raise CliError("server_incompatible", "The Forge server answered the setup list in an unknown form.",
                           next=[step("meta-forge version", "Check the server's Forge release.")])
        return setups

    async def resolve_setup(self, value):
        """A full setup ID, a unique prefix of at least 8 hex digits, or last.

        Errors never repeat the value given: a secret may have been typed in its place.
        """
        self.resolved = await self._resolve_setup(value)
        return self.resolved

    async def _resolve_setup(self, value):
        if not isinstance(value, str):
            raise CliError("usage", "Name a setup: its ID, a unique ID prefix or last.",
                           next=[step("meta-forge setup list", "List your setups and their IDs.")])
        candidate = value.strip().lower()
        if UUID.fullmatch(candidate):
            return candidate
        if candidate == "last":
            setups = await self.list_setups()
            if not setups:
                raise CliError("not_found", "There are no saved setups yet.",
                               next=[step("meta-forge setup new", "Start a new setup."),
                                     step("meta-forge chat --new", "Start a new setup and talk to the guide.")])
            return setups[0]["id"]
        if not PREFIX.fullmatch(candidate):
            raise CliError("usage", "The SETUP given is not a setup ID. Use a full ID, a unique prefix of at least "
                           "8 hex digits, or last.",
                           next=[step("meta-forge setup list", "List your setups and their IDs.")])
        matches = [item["id"] for item in await self.list_setups()
                   if isinstance(item, dict) and isinstance(item.get("id"), str) and item["id"].startswith(candidate)]
        if not matches:
            raise CliError("not_found", "No saved setup ID starts with the SETUP given.",
                           next=[step("meta-forge setup list", "List your setups and their IDs.")])
        if len(matches) > 1:
            raise CliError("usage", f"The SETUP given matches {len(matches)} setups: "
                           + ", ".join(sorted(matches)) + ". Give more of the ID.",
                           next=[step("meta-forge setup list", "List your setups and their IDs.")])
        return matches[0]
