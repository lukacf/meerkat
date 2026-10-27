"""Local login front for the unchanged native Console and admitted human input.

The host authenticates this channel; native admission remains native authority.
Native timeline frames do not attest the authenticated human principal.
"""
from __future__ import annotations

import asyncio
import copy
from dataclasses import dataclass
import hashlib
import hmac
import html
import inspect
import json
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import time
from urllib.parse import parse_qsl, urlsplit

try:
    from .console_configuration import console_configuration, validate_console_credentials
except ImportError:
    from console_configuration import console_configuration, validate_console_credentials

ISSUER = "http://127.0.0.1/mobkit-gateway"
AUDIENCE = "persistent-gateway"
HEALTH_AUDIENCE = "mobkit-launch-host-health"
SESSION_SECONDS = 8 * 60 * 60
HEALTH_PATHS = {"/console/experience", "/console/identities"}
MAX_BODY = 101 * 1024 * 1024  # The pinned native multipart limit.
# A 4096-character password can use four UTF-8 bytes per character, each
# encoded as three form bytes. Leave bounded space for principal, nonce and keys.
MAX_LOGIN_BODY = 64 * 1024
FRONTEND_PATH = "/console/forge/frontends"
MAX_FRONTEND_BODY = 256 * 1024
FRONTEND_APPROVAL_SECONDS = 300


@dataclass(frozen=True)
class FrontendManagementRequest:
    """One authenticated host command, never exposed as an agent callback tool."""
    operation: str
    principal: str
    host: dict
    payload: dict
    approved: bool = False


def _frontend_json(value):
    """Canonical, bounded nonsecret protocol bytes for exact integrity binding."""
    def ordinary(item):
        if isinstance(item, dict):
            if any(not isinstance(key, str) for key in item):
                raise ValueError("JSON object keys must be strings.")
            for child in item.values():
                ordinary(child)
        elif isinstance(item, list):
            for child in item:
                ordinary(child)
        elif item is not None and not isinstance(item, (str, int, float, bool)):
            raise ValueError("Ordinary JSON is required.")
    ordinary(value)
    encoded = json.dumps(value, sort_keys=True, ensure_ascii=False,
                         allow_nan=False, separators=(",", ":")).encode("utf-8")
    if len(encoded) > MAX_FRONTEND_BODY:
        raise ValueError("Frontend JSON exceeds its limit.")
    return encoded


def _frontend_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON keys are refused.")
        result[key] = value
    return result


def _private_key(root, *, create=False):
    directory = Path(root) / ".mobkit"
    if create:
        directory.mkdir(mode=0o700, exist_ok=True)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("The Console private state directory is invalid.")
    info = directory.stat()
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise ValueError("The Console state directory must be owned privately by this user.")
    path = directory / "console-signing.key"
    if create:
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            pass
        else:
            with os.fdopen(descriptor, "w") as channel:
                channel.write(secrets.token_hex(32))
                channel.flush()
                os.fsync(channel.fileno())
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "r") as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) & 0o077):
            raise ValueError("The Console signing key must be a private owned ordinary file.")
        key = channel.read(65)
    if not re.fullmatch(r"[a-f0-9]{64}", key):
        raise ValueError("The Console signing key is invalid.")
    return key


def _token(key, principal, *, scope="native-console"):
    import jwt
    now = int(time.time())
    return jwt.encode({"iss": ISSUER, "aud": HEALTH_AUDIENCE if scope == "host-health" else AUDIENCE,
                       "sub": principal, "email": principal,
                       "email_verified": True, "iat": now, "exp": now + 120,
                       "host_scope": scope}, key, algorithm="HS256")


def health_headers(root):
    """Host-owned read-only probe credentials. Never load application Python code."""
    try:
        from .application_configuration import application_fields
    except ImportError:
        from application_configuration import application_fields
    root = Path(root)
    if not console_configuration(root, application_fields(root)):
        return {}
    path = root / ".env"
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as channel:
        info = os.fstat(channel.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) & 0o077 or info.st_size > 2 * 1024 * 1024):
            raise ValueError("Console health needs an owned private configuration file.")
        rows = channel.read().splitlines()
    principal = None
    for row in rows:
        name, separator, value = row.partition("=")
        if separator and name.strip() == "CONSOLE_USER_ID":
            value = value.strip()
            principal = json.loads(value) if value.startswith('"') else value.strip("'")
    if not isinstance(principal, str) or not re.fullmatch(r"[A-Za-z0-9_.@+-]{1,200}", principal):
        raise ValueError("Console user identity is not configured.")
    return {"Authorization": "Bearer " + _token(_private_key(root), principal, scope="host-health")}


@dataclass(frozen=True)
class ConsoleInput:
    """Host-authenticated submission plus unmodified native admission evidence."""
    channel: str
    principal: str
    identity: str
    content: object
    interaction_id: str
    session_id: str
    input_frame_id: str
    cursor: str
    conversation_id: str | None
    handling_mode: str | None


class ConsoleFront:
    def __init__(self, root, port, principal, password, on_input=None, *, transport=None):
        validate_console_credentials(principal, password)
        self.root, self.port, self.principal = Path(root), port, principal
        self.origin = f"http://127.0.0.1:{port}"
        self.key = _private_key(root, create=True)
        self.password_digest = hashlib.sha256(password.encode()).digest()
        self.cookie = "mobkit_console_" + hashlib.sha256((str(self.root.resolve()) + str(port)).encode()).hexdigest()[:16]
        self.sessions, self.delivered = {}, set()
        self.login_tokens, self.failures = {}, []
        self.delivery_lock = asyncio.Lock()
        self.on_input, self.transport = on_input, transport
        self.frontend_management = self.frontend_host = None
        self.frontend_private_fields = ()
        self.client = self.server = self.task = self.listener = None

    @property
    def native_auth(self):
        return {"provider": "jwt", "shared_secret": self.key, "issuer": ISSUER,
                "audience": AUDIENCE, "email_allowlist": [self.principal]}

    def application(self, native_url):
        import httpx
        from starlette.applications import Starlette
        from starlette.routing import Route
        parsed = urlsplit(native_url)
        if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port
                or parsed.username or parsed.password or parsed.path not in ("", "/") or parsed.query or parsed.fragment):
            raise ValueError("Console upstream must be this runtime's loopback HTTP endpoint.")
        self.native_url = native_url.rstrip("/")
        self.client = httpx.AsyncClient(transport=self.transport, trust_env=False,
                                        timeout=httpx.Timeout(60, connect=5), follow_redirects=False)
        return Starlette(routes=[Route("/{path:path}", self.handle, methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"])])

    async def start(self, native_url):
        import uvicorn
        app = self.application(native_url)
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            # A stopped front can leave accepted connections in TIME_WAIT.
            # Reuse its loopback address without allowing a second live listener.
            self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self.listener.bind(("127.0.0.1", self.port))
            self.listener.listen(128)
            self.listener.setblocking(False)
            self.server = uvicorn.Server(uvicorn.Config(app, access_log=False, log_level="error", lifespan="off"))
            # The generated app owns signals and shutdown, including its native process.
            self.server.capture_signals = _no_signals
            self.task = asyncio.create_task(self.server.serve(sockets=[self.listener]))
            deadline = asyncio.get_running_loop().time() + 10
            while not self.server.started:
                if self.task.done():
                    await self.task
                    raise RuntimeError("The Console login front did not start.")
                if asyncio.get_running_loop().time() >= deadline:
                    raise TimeoutError("The Console login front did not become ready.")
                await asyncio.sleep(0.02)
        except BaseException:
            await self.close()
            raise

    async def close(self):
        if self.server:
            self.server.should_exit = True
        try:
            if self.task:
                await asyncio.wait_for(self.task, 5)
        except asyncio.TimeoutError:
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)
        except asyncio.CancelledError:
            if self.task:
                self.task.cancel()
                await asyncio.gather(self.task, return_exceptions=True)
            raise
        except Exception:
            # A failed server must not prevent native/application cleanup.
            pass
        finally:
            if self.listener:
                self.listener.close()
            if self.client:
                await self.client.aclose()
            self.sessions.clear()

    async def _body(self, request, limit):
        body = bytearray()
        async for chunk in request.stream():
            body.extend(chunk)
            if len(body) > limit:
                raise ValueError("Request too large.")
        return bytes(body)

    def _login_response(self, now, *, error=None, status_code=200):
        from starlette.responses import HTMLResponse, Response
        if len(self.login_tokens) >= 100:
            return Response("Try again shortly.", status_code=429)
        nonce = secrets.token_urlsafe(32)
        self.login_tokens[nonce] = now + 300
        response = HTMLResponse(_login_form(nonce, error), status_code=status_code)
        response.set_cookie(self.cookie + "_login", nonce, httponly=True, samesite="strict", max_age=300)
        response.headers.update({"Cache-Control": "no-store", "Content-Security-Policy": "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'"})
        return response

    async def handle(self, request):
        from starlette.responses import JSONResponse, RedirectResponse, Response, StreamingResponse
        path = request.url.path
        if (request.headers.get("host") != self.origin.removeprefix("http://")
                or any(key == "auth_token" for key, _ in parse_qsl(request.url.query))):
            return Response("Invalid Console address.", status_code=400)
        if request.headers.get("origin") not in (None, self.origin):
            return Response("Cross-origin Console requests are refused.", status_code=403)
        now = time.time()
        self.sessions = {key: expiry for key, expiry in self.sessions.items() if expiry > now}
        if path == "/login":
            self.login_tokens = {key: expiry for key, expiry in self.login_tokens.items() if expiry > now}
            if request.method == "GET":
                return self._login_response(now)
            if request.method != "POST" or request.headers.get("origin") != self.origin:
                return Response("Login requires a same-origin form.", status_code=403)
            try:
                body = await self._body(request, MAX_LOGIN_BODY)
                form = dict(parse_qsl(body.decode(), max_num_fields=4))
            except (ValueError, UnicodeError):
                return Response("Invalid login form.", status_code=400)
            nonce = form.get("nonce", "")
            if (not nonce or nonce != request.cookies.get(self.cookie + "_login")
                    or not self.login_tokens.pop(nonce, None)):
                return self._login_response(now, error="This sign-in form expired. Enter your Console password again.", status_code=403)
            self.failures = [at for at in self.failures if at > now - 60]
            if len(self.failures) >= 10:
                return Response("Too many login attempts. Try again in one minute.", status_code=429)
            if not hmac.compare_digest(hashlib.sha256(form.get("password", "").encode()).digest(), self.password_digest):
                self.failures.append(now)
                return self._login_response(now, error="That password didn't open this Console. Try the Console password you chose during installation.", status_code=401)
            if len(self.sessions) >= 100:
                return Response("Too many active sessions. Restart the application to clear them.", status_code=429)
            session = secrets.token_urlsafe(32)
            self.sessions[session] = now + SESSION_SECONDS
            response = RedirectResponse("/console", status_code=303)
            response.set_cookie(self.cookie, session, httponly=True, samesite="strict", max_age=SESSION_SECONDS)
            response.delete_cookie(self.cookie + "_login")
            response.headers["Cache-Control"] = "no-store"
            return response
        is_human = request.cookies.get(self.cookie) in self.sessions
        health = False
        if not is_human:
            import jwt
            authorization = request.headers.get("authorization", "")
            if authorization.startswith("Bearer ") and request.method == "GET" and path in HEALTH_PATHS:
                try:
                    claims = jwt.decode(authorization[7:], self.key, algorithms=["HS256"], issuer=ISSUER, audience=HEALTH_AUDIENCE,
                                        options={"require": ["exp", "sub", "host_scope"]})
                    health = claims.get("sub") == self.principal and claims.get("host_scope") == "host-health"
                except jwt.PyJWTError:
                    pass
            if not health:
                if request.method == "GET" and path in ("/", "/console", "/console/"):
                    return RedirectResponse("/login", status_code=303)
                return JSONResponse({"error": "Console login required."}, status_code=401)
        if request.method not in ("GET", "HEAD") and request.headers.get("origin") != self.origin:
            return Response("Console changes require a same-origin authenticated browser.", status_code=403)
        if path == "/logout" and request.method == "POST":
            self.sessions.pop(request.cookies.get(self.cookie), None)
            response = RedirectResponse("/login", status_code=303)
            response.delete_cookie(self.cookie)
            return response
        if path == "/":
            return RedirectResponse("/console", status_code=303)
        if path == FRONTEND_PATH or path.startswith(FRONTEND_PATH + "/"):
            return await self._frontend_request(request)
        if path != "/" and not path.startswith(("/console", "/blobs/")) and path != "/favicon.ico":
            return Response("Unknown Console route.", status_code=404)
        try:
            body = await self._body(request, MAX_BODY)
            submission = await self._submission(request, body) if is_human and request.method == "POST" else None
        except (ValueError, UnicodeError, json.JSONDecodeError):
            return Response("Invalid Console request.", status_code=400)
        headers = {"Authorization": "Bearer " + _token(self.key, self.principal)}
        for name in ("content-type", "accept", "last-event-id", "range"):
            if name in request.headers:
                headers[name] = request.headers[name]
        url = self.client.base_url.copy_with(scheme="http", host="127.0.0.1", port=urlsplit(self.native_url).port,
                                              path=path, query=request.url.query.encode())
        try:
            import httpx
            timeout = httpx.Timeout(None, connect=5, write=60, pool=5) if path.endswith("/stream") else 60
            upstream = await self.client.send(self.client.build_request(request.method, url, content=body, headers=headers,
                                                                         timeout=timeout), stream=True)
        except Exception:
            return Response("The native Console is unavailable.", status_code=502)
        forwarded = {key: value for key, value in upstream.headers.items()
                     if key.lower() in {"content-type", "content-range", "accept-ranges", "etag"}}
        forwarded["Cache-Control"] = "no-store"
        forwarded["X-Content-Type-Options"] = "nosniff"
        forwarded["Content-Security-Policy"] = "frame-ancestors 'none'"
        forwarded["Referrer-Policy"] = "no-referrer"
        if "text/event-stream" in upstream.headers.get("content-type", ""):
            async def stream():
                try:
                    async for chunk in upstream.aiter_raw():
                        yield chunk
                finally:
                    await upstream.aclose()
            return StreamingResponse(stream(), status_code=upstream.status_code, headers=forwarded)
        try:
            payload = await upstream.aread()
        finally:
            await upstream.aclose()
        if submission is not None and upstream.is_success and self.on_input:
            try:
                envelope = json.loads(payload)
                accepted = envelope.get("result") if path != "/console/send" else envelope
                if isinstance(accepted, dict) and not envelope.get("error"):
                    await self._deliver(submission, accepted)
            except Exception:
                # Native may already have admitted the turn. Retrying its same
                # idempotency key lets the application retry its own delivery.
                return JSONResponse({"error": "Native input was admitted but application delivery failed. Retry the same submission."}, status_code=503)
        return Response(payload, status_code=upstream.status_code, headers=forwarded)

    async def _frontend_request(self, request):
        """Delegate component truth to one explicit application owner, once."""
        from starlette.responses import JSONResponse
        def error(code, detail, *, change_id=None):
            result = {"detail": detail}
            if change_id is not None:
                result.update(status="unknown", change_id=change_id)
            return JSONResponse(result, status_code=code, headers={"Cache-Control": "no-store"})

        path = request.url.path
        if path == FRONTEND_PATH:
            operation, method = "inventory", "GET"
        elif path in (FRONTEND_PATH + "/prepare", FRONTEND_PATH + "/submit"):
            operation, method = path.rsplit("/", 1)[1], "POST"
        elif path.startswith(FRONTEND_PATH + "/changes/"):
            operation, method = "status", "GET"
            change_id = path.removeprefix(FRONTEND_PATH + "/changes/")
            if not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", change_id):
                return error(400, "Use the exact frontend change identifier.")
        else:
            return error(404, "Unknown frontend management route.")
        if request.method != method:
            return error(405, "This frontend management method is not supported.")
        if not callable(self.frontend_management) or not callable(self.frontend_host):
            return error(501, "This application has not declared frontend management support.")
        if request.url.query:
            return error(400, "Frontend management does not accept query parameters.")
        try:
            host = self.frontend_host()
            _frontend_json(host)
        except Exception:
            return error(409, "The loaded application source changed. No frontend command was admitted.")

        payload, approved = {}, False
        try:
            if method == "GET":
                if await self._body(request, MAX_FRONTEND_BODY):
                    raise ValueError("A read command has no body.")
                if operation == "status":
                    payload = {"change_id": change_id}
            else:
                if request.headers.get("content-type", "").split(";", 1)[0].strip() != "application/json":
                    raise ValueError("JSON is required.")
                payload = json.loads(await self._body(request, MAX_FRONTEND_BODY),
                                     object_pairs_hook=_frontend_object)
                _frontend_json(payload)
                self._frontend_no_private_literals(payload)
                if not isinstance(payload, dict):
                    raise ValueError("A command is an object.")
                if operation == "prepare":
                    if set(payload) != {"configuration", "expected_revision"} or not isinstance(payload["configuration"], dict):
                        raise ValueError("Prepare needs exact configuration and revision.")
                else:
                    if set(payload) != {"proposal", "approval"}:
                        raise ValueError("Submit needs the exact proposal and protected approval.")
                    self._frontend_proposal(payload["proposal"])
                    approval = payload["approval"]
                    if (not isinstance(approval, dict) or set(approval) != {"approved", "expires_at", "proof"}
                            or approval["approved"] is not True or type(approval["expires_at"]) is not int
                            or not time.time() < approval["expires_at"] <= time.time() + FRONTEND_APPROVAL_SECONDS
                            or not isinstance(approval["proof"], str)
                            or not hmac.compare_digest(approval["proof"], self._frontend_proof(host, payload["proposal"], approval["expires_at"]))):
                        return error(403, "Explicit approval of this exact current proposal is required.")
                    approved = True
                    payload = {"proposal": payload["proposal"],
                               "approval": {"principal": self.principal, "expires_at": approval["expires_at"]}}
            command = FrontendManagementRequest(operation, self.principal, copy.deepcopy(host), copy.deepcopy(payload), approved)
        except (ValueError, TypeError, UnicodeError, RecursionError, KeyError):
            return error(400, "Invalid frontend command. Use nonsecret configuration and the exact proposal.")

        try:
            result = self.frontend_management(command)
            if inspect.isawaitable(result):
                result = await result
            if not isinstance(result, dict):
                raise ValueError("The application observation must be an object.")
            _frontend_json(result)
            self._frontend_no_private_literals(result)
            if self.frontend_host() != host:
                raise ValueError("The application source changed during the command.")
            envelope = {"protocol": 1, "host": host}
            if operation == "prepare":
                proposal = result.get("proposal")
                self._frontend_proposal(proposal)
                expires_at = int(time.time()) + FRONTEND_APPROVAL_SECONDS
                envelope.update(proposal=proposal, approval={"expires_at": expires_at,
                    "proof": self._frontend_proof(host, proposal, expires_at)})
            else:
                envelope["observation"] = result
            _frontend_json(envelope)
            return JSONResponse(envelope, headers={"Cache-Control": "no-store"})
        except Exception:
            if operation == "submit":
                return error(503, "Frontend submission outcome is unknown. Read this exact change status before any further action.",
                             change_id=payload["proposal"]["change_id"])
            return error(503, "The application frontend owner is unavailable. No activation is claimed.")

    def _frontend_proposal(self, proposal):
        if (not isinstance(proposal, dict) or not {"change_id", "configuration", "expected_revision"} <= set(proposal)
                or not isinstance(proposal["configuration"], dict)
                or not isinstance(proposal["change_id"], str)
                or not re.fullmatch(r"[A-Za-z0-9._:-]{1,128}", proposal["change_id"])):
            raise ValueError("An exact application proposal is required.")
        _frontend_json(proposal)
        self._frontend_no_private_literals(proposal)

    def _frontend_proof(self, host, proposal, expires_at):
        binding = {"scope": "forge-frontend-proposal-v1", "host": host, "principal": self.principal,
                   "proposal": proposal, "expires_at": expires_at}
        return hmac.new(self.key.encode(), _frontend_json(binding), hashlib.sha256).hexdigest()

    def _frontend_no_private_literals(self, value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key in self.frontend_private_fields and child != {"private_field": key}:
                    raise ValueError("Private values stay with the existing private setup owner.")
                self._frontend_no_private_literals(child)
        elif isinstance(value, list):
            for child in value:
                self._frontend_no_private_literals(child)

    async def _submission(self, request, body):
        path = request.url.path
        if path == "/console/send":
            return json.loads(body)
        if path == "/console/rpc":
            value = json.loads(body)
        elif path == "/console/rpc/multipart":
            # Inspect the same bounded bytes forwarded verbatim to native.
            # Upload substitutions stay native-owned, content is submitted data.
            from starlette.requests import Request
            sent = False
            async def receive():
                nonlocal sent
                chunk = body if not sent else b""
                sent = True
                return {"type": "http.request", "body": chunk, "more_body": False}
            copy = Request(request.scope, receive)
            async with copy.form(max_files=4, max_fields=8, max_part_size=2 * 1024 * 1024) as form:
                value = json.loads(form.get("payload", "{}"))
        else:
            return None
        if not isinstance(value, dict):
            raise ValueError("Console RPC must be an object.")
        return value.get("params") if value.get("method") == "mobkit/console/send" else None

    async def _deliver(self, submission, accepted):
        fields = ("identity", "interaction_id", "session_id", "input_frame_id", "cursor")
        if (not isinstance(submission, dict) or "content" not in submission
                or any(not isinstance(accepted.get(key), str) or not accepted[key] for key in fields)
                or accepted["identity"] != submission.get("identity")):
            raise ValueError("Native admission evidence is incomplete.")
        event = ConsoleInput(channel="host-authenticated-console", principal=self.principal,
            identity=accepted["identity"], content=submission["content"], interaction_id=accepted["interaction_id"],
            session_id=accepted["session_id"], input_frame_id=accepted["input_frame_id"], cursor=accepted["cursor"],
            conversation_id=accepted.get("conversation_id"), handling_mode=submission.get("handling_mode"))
        async with self.delivery_lock:
            if event.interaction_id in self.delivered:
                return
            if len(self.delivered) >= 10000:
                raise RuntimeError("Application delivery deduplication is full; restart after persisting application evidence.")
            result = self.on_input(event)
            if inspect.isawaitable(result):
                await asyncio.wait_for(result, 10)
            self.delivered.add(event.interaction_id)


def _login_form(nonce, error=None):
    notice = f'<p id="login-error" role="alert">{html.escape(error)}</p>' if error else ""
    description = ' aria-describedby="login-error"' if error else ""
    return f'''<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>MobKit Console login</title><style>body{{font:16px system-ui;background:#faf9f6;color:#242522;margin:0;padding:6vh 20px}}main{{max-width:400px;margin:auto}}p{{line-height:1.6}}input,button{{font:inherit;padding:12px;box-sizing:border-box;width:100%;margin:12px 0}}button{{background:#242522;color:white;border:0;border-radius:6px}}#login-error{{padding:12px;background:#fbece5;color:#783d2e;border:1px solid #dab19d;border-radius:6px}}</style></head><body><main><h1>Open your Console</h1><p>Enter the private Console password you chose during installation. Your configured user identity is applied automatically. Your provider API key is separate.</p>{notice}<form method="post" action="/login"><input type="hidden" name="nonce" value="{html.escape(nonce)}"><label for="password">Console password</label><input id="password" type="password" name="password" autocomplete="current-password" required autofocus{description}><button>Open Console</button></form></main></body></html>'''


from contextlib import contextmanager


@contextmanager
def _no_signals():
    yield
