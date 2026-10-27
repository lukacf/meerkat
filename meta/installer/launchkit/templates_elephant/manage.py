#!/usr/bin/env python3
"""Operator-only knowledge service setup and schema-first verification for this project."""
from __future__ import annotations

import argparse
import asyncio
import base64
import ctypes
import getpass
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import secrets
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import time
import tempfile
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent
READ_SCOPES = ["read:assertion", "read:audit", "read:doc", "read:doc:metadata", "read:doc:raw", "read:entity", "read:event", "read:predicate", "read:relationship", "read:source", "read:source:raw", "read:space_config", "read:system", "read:system_event", "read:work_item"]
# The knowledge service's documented extractor identity: a read-only token for
# this space whose subject also holds an active grant with the same scopes.
WORKER_SUBJECT = "recall-extractor"
WORKER_SCOPES = ["read:assertion", "read:doc:metadata", "read:doc:raw", "read:entity", "read:predicate", "read:source"]
# Space and grant administration only; never stored.
BOOTSTRAP_SCOPES = ["manage:grant", "manage:provider", "read:space_config", "write:space_config"]
BOOTSTRAP_LIFETIME = 900
SPACE_SECURITY = {"policy_id": "default", "level": 0, "labels": [], "handling": []}
CLEARANCES = [{"policy_id": "default", "max_level": 0, "labels": [], "handling_allow": []}]
APPROVED_OPERATIONS = ["query_embedding", "document_embedding", "extraction", "support_verification"]
PROVIDER_KEYS = {"openai": "OPENAI_API_KEY", "anthropic": "ANTHROPIC_API_KEY", "gemini": "GEMINI_API_KEY"}
PROVIDER_ENDPOINTS = {"openai": "https://api.openai.com/v1", "anthropic": "https://api.anthropic.com",
                      "gemini": "https://generativelanguage.googleapis.com"}
PREREQUISITE_TIMEOUT = 10
REQUEST_TIMEOUT = 15
READY_TIMEOUT = 120
MAX_RESPONSE = 1024 * 1024


def load_settings():
    return json.loads((ROOT / "settings.json").read_text())


def read_env(path):
    result = {}
    if not path.exists():
        return result
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        name, equals, value = line.partition("=")
        if not equals:
            raise ValueError("Malformed environment file; expected NAME=value.")
        value = value.strip()
        if value.startswith('"'):
            value = json.loads(value)
        elif value.startswith("'") and value.endswith("'"):
            value = value[1:-1]
        result[name] = value
    return result


def write_env(path, values, *, compose=True):
    if path.exists() and path.is_symlink():
        raise ValueError("Refusing to replace a symlinked environment file.")
    for key, value in values.items():
        if any(char in value for char in ("\r\n\x00'\\" if compose else "\x00")):
            raise ValueError(f"{key} contains characters unsupported by the local dotenv format.")
    fd, temporary = tempfile.mkstemp(prefix=".env-", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as stream:
            # Compose needs literal single quotes; the host accepts JSON strings.
            for key, value in sorted(values.items()):
                encoded = "'" + value + "'" if compose else json.dumps(value)
                stream.write(f"{key}={encoded}\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def sign(secret, claims):
    encode = lambda value: base64.urlsafe_b64encode(json.dumps(value, separators=(",", ":")).encode()).decode().rstrip("=")
    body = encode({"alg": "HS256", "typ": "JWT"}) + "." + encode(claims)
    signature = base64.urlsafe_b64encode(hmac.new(secret.encode(), body.encode(), hashlib.sha256).digest()).decode().rstrip("=")
    return body + "." + signature


def jwt_token(secret, space, *, operator=False, now=None):
    now = int(time.time()) if now is None else now
    return sign(secret, {"sub": f"mobkit-launch:{space}:" + ("operator" if operator else "agents"), "iat": now, "exp": now + 90 * 86400,
                         "iss": "mobkit-launch", "aud": [space], "space_id": space, "scopes": ["*"] if operator else READ_SCOPES,
                         "security_level": 100, "security_labels": [], "purpose": ["project-knowledge"]})


def worker_token(secret, space, *, now=None):
    # Naming the space in `spaces` makes the service check the subject's grant.
    now = int(time.time()) if now is None else now
    return sign(secret, {"sub": WORKER_SUBJECT, "iat": now, "exp": now + 90 * 86400, "iss": "mobkit-launch", "aud": [space],
                         "space_id": space, "spaces": [space], "scopes": WORKER_SCOPES, "clearances": CLEARANCES})


def bootstrap_token(secret, space, *, now=None):
    now = int(time.time()) if now is None else now
    return sign(secret, {"sub": f"mobkit-launch:{space}:bootstrap", "iat": now, "exp": now + BOOTSTRAP_LIFETIME,
                         "iss": "mobkit-launch", "aud": [space], "space_id": space, "spaces": [space],
                         "bootstrap_space": space, "scopes": BOOTSTRAP_SCOPES, "clearances": CLEARANCES})


def add_missing_worker_token(settings):
    """Give a folder prepared before the extractor token existed that token alone."""
    values = read_env(ROOT / ".env")
    if not values.get("MCP_BEARER_TOKEN"):
        values["MCP_BEARER_TOKEN"] = worker_token(values["JWT_SECRET"], settings["project"])
        write_env(ROOT / ".env", values)


def require_retained_secrets(settings, values, project_values, *, renew=False):
    missing = any(not values.get(key) for key in ("JWT_SECRET", "SURREALDB_PASS"))
    names = (settings["token_env"], settings["space_env"])
    retained = (renew or any(values.get(key) for key in (*names, "ELEPHANT_OPERATOR_TOKEN", "JWT_SECRET", "SURREALDB_PASS"))
        or any(project_values.get(key) for key in names) or os.path.lexists(ROOT / ".source"))
    if missing and retained:
        raise ValueError("Previous local knowledge preparation is present, but its signing or database credentials are missing. "
            "Restore the original private knowledge/.env before setup, renew or up. No settings or service was changed. "
            "New credentials cannot recover existing knowledge data.")
    return missing


def setup(settings, renew=False):
    if settings["mode"] != "local":
        raise ValueError("Remote services are managed by their operator. Add the token and expected space to the project's .env, then run verify.")
    values = read_env(ROOT / ".env")
    project_values = read_env(ROOT.parent / ".env")
    require_retained_secrets(settings, values, project_values, renew=renew)
    if not values.get("JWT_SECRET"):
        values["JWT_SECRET"] = secrets.token_urlsafe(48)
    if not values.get("SURREALDB_PASS"):
        values["SURREALDB_PASS"] = secrets.token_urlsafe(36)
    provider_key = {"openai": "OPENAI_API_KEY", "anthropic": "ANTHROPIC_API_KEY", "gemini": "GEMINI_API_KEY"}[settings["provider"]]
    for key in sorted({"OPENAI_API_KEY", provider_key}):
        preferred, fallback = (values, project_values) if renew else (project_values, values)
        value = preferred.get(key) or fallback.get(key) or os.environ.get(key)
        if not value:
            value = getpass.getpass(f"{key} (stored locally, input hidden): ").strip()
        if not value:
            raise ValueError(f"{key} is required for the knowledge pipeline.")
        values[key] = value
    token_name, space_name = settings["token_env"], settings["space_env"]
    values[token_name] = jwt_token(values["JWT_SECRET"], settings["project"])
    values["ELEPHANT_OPERATOR_TOKEN"] = jwt_token(values["JWT_SECRET"], settings["project"], operator=True)
    values["MCP_BEARER_TOKEN"] = worker_token(values["JWT_SECRET"], settings["project"])
    values[space_name] = settings["project"]
    project_values[token_name] = values[token_name]
    project_values[space_name] = values[space_name]
    write_env(ROOT / ".env", values)
    write_env(ROOT.parent / ".env", project_values, compose=False)
    print("Knowledge credentials saved locally. Agent token is read-only; the operator and extractor tokens stay in knowledge/.env. Tokens expire in 90 days.")


def command(args, *, capture=False, timeout=None):
    return subprocess.run(args, cwd=ROOT, check=True, text=True, capture_output=capture, timeout=timeout)


def _docker_probe(args, failure):
    # Prerequisite output is consumed with a byte and time bound, never emitted.
    process = None
    finished = False
    try:
        process = subprocess.Popen(args, cwd=ROOT, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
        deadline = time.monotonic() + PREREQUISITE_TIMEOUT
        result = bytearray()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ValueError(failure)
                for key, _ in selector.select(remaining):
                    chunk = os.read(key.fd, 4096)
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        result.extend(chunk)
                        if len(result) > 64 * 1024:
                            raise ValueError(failure)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ValueError(failure)
            process.wait(timeout=remaining)
        if process.returncode:
            raise ValueError(failure)
        value = result.decode("utf-8")
        finished = True
        return value
    except (OSError, UnicodeError, subprocess.TimeoutExpired):
        raise ValueError(failure) from None
    finally:
        if process is not None:
            if not finished:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except OSError:
                    raise ValueError("The Docker prerequisite check could not be stopped. No knowledge setup was performed.") from None
            try:
                if process.poll() is None:
                    process.wait(timeout=3)
            except (OSError, subprocess.TimeoutExpired):
                raise ValueError("The Docker prerequisite check could not be stopped. No knowledge setup was performed.") from None
            finally:
                process.stdout.close()


def local_preflight():
    if not shutil.which("docker"):
        raise ValueError("Install Docker with Compose before starting local knowledge.")
    help_text = _docker_probe(["docker", "compose", "up", "--help"],
        "Docker Compose is unavailable. Install or update Docker Compose, then retry knowledge setup.")
    if any(not re.search(r"(?m)^\s*" + re.escape(flag) + r"(?:\s|$)", help_text)
           for flag in ("--wait", "--wait-timeout")):
        raise ValueError("Docker Compose needs --wait and --wait-timeout support. Update Docker Compose, then retry knowledge setup.")
    engine = _docker_probe(["docker", "info", "--format", "{{.OSType}}"],
        "The Docker engine is unreachable. Start Docker Desktop or connect to a Linux Docker engine, then retry knowledge setup.")
    if engine.strip() != "linux":
        raise ValueError("Local knowledge needs a Linux Docker engine. Select Linux containers, then retry knowledge setup.")


def source_stage_current(stage, identity):
    try:
        info = stage.lstat()
    except FileNotFoundError:
        return False
    if not stat.S_ISDIR(info.st_mode) or (info.st_dev, info.st_ino) != identity:
        raise ValueError("Knowledge staging directory changed. Preserve the pending paths and retry in a new attempt.")
    return True


def publish_source(stage, source, identity):
    """Publish this new checkout without replacing any competing path."""
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "linux":
        function = getattr(library, "renameat2", None)
        arguments = (-100, os.fsencode(stage), -100, os.fsencode(source), 1)
        signature = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    elif sys.platform == "darwin":
        function = getattr(library, "renamex_np", None)
        arguments = (os.fsencode(stage), os.fsencode(source), 0x4)
        signature = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
    else:
        function = None
    if function is None:
        raise ValueError("This computer cannot publish the knowledge checkout without replacing existing source. Preserve this folder.")
    function.argtypes, function.restype = signature, ctypes.c_int
    if not source_stage_current(stage, identity):
        raise ValueError("Knowledge staging directory disappeared before publication. Preserve this folder and retry.")
    if function(*arguments) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), str(source))


def source_checkout(settings):
    source = ROOT / ".source"
    repository, revision = settings["source_repository"], settings["source_revision"]
    if source.is_symlink():
        raise ValueError("Refusing a symlinked knowledge source directory.")
    if source.exists():
        head = command(["git", "-C", str(source), "rev-parse", "HEAD"], capture=True).stdout.strip()
        status = command(["git", "-C", str(source), "status", "--porcelain"], capture=True).stdout.strip()
        origin = command(["git", "-C", str(source), "remote", "get-url", "origin"], capture=True).stdout.strip()
        if head != revision or status or origin != repository:
            raise ValueError("Existing knowledge source differs from the pinned clean checkout. Preserve it and choose a new deployment directory.")
        return
    stage = Path(tempfile.mkdtemp(prefix=".source-pending-", dir=ROOT))
    info = stage.lstat()
    identity = (info.st_dev, info.st_ino)
    try:
        command(["git", "-C", str(stage), "init"])
        command(["git", "-C", str(stage), "remote", "add", "origin", repository])
        command(["git", "-C", str(stage), "fetch", "--depth=1", "origin", revision])
        command(["git", "-C", str(stage), "checkout", "--detach", "FETCH_HEAD"])
        actual = command(["git", "-C", str(stage), "rev-parse", "HEAD"], capture=True).stdout.strip()
        status = command(["git", "-C", str(stage), "status", "--porcelain"], capture=True).stdout.strip()
        origin = command(["git", "-C", str(stage), "remote", "get-url", "origin"], capture=True).stdout.strip()
        if actual != revision or status or origin != repository:
            raise ValueError("Knowledge source checkout did not match the pinned clean revision and repository.")
        publish_source(stage, source, identity)
    finally:
        # Refuse a replaced path. Never remove the final source or a moved stage.
        if source_stage_current(stage, identity):
            shutil.rmtree(stage)


def compose(*args):
    if not shutil.which("docker"):
        raise ValueError("Install Docker with Compose before starting local knowledge.")
    command(["docker", "compose", "--env-file", str(ROOT / ".env"), "-f", str(ROOT / "compose.json"), *args])


def environment():
    return {**read_env(ROOT.parent / ".env"), **read_env(ROOT / ".env"), **os.environ}


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


# Local administration never follows a redirect or an environment proxy with its bearer.
_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect)


def api_request(settings, method, path, token, *, body=None):
    """One bounded JSON request to the local knowledge service, selecting the project space."""
    parsed = urllib.parse.urlsplit(settings["url"])
    headers = {"Authorization": "Bearer " + token, "X-Space-Id": settings["project"], "Accept": "application/json"}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(f"{parsed.scheme}://{parsed.netloc}{path}", data=data, method=method, headers=headers)
    try:
        with _OPENER.open(request, timeout=REQUEST_TIMEOUT) as response:
            status, raw = response.status, response.read(MAX_RESPONSE + 1)
    except urllib.error.HTTPError as error:
        with error:
            status, raw = error.code, error.read(MAX_RESPONSE + 1)
    except (urllib.error.URLError, OSError):
        raise ValueError("The local knowledge service is unreachable. Check `docker compose ps` for the knowledge project.") from None
    if len(raw) > MAX_RESPONSE:
        raise ValueError("The knowledge service returned an oversized response.")
    try:
        return status, json.loads(raw) if raw else None
    except (UnicodeError, ValueError):
        return status, None


def refusal(action, status, data):
    code = data.get("error_code") if isinstance(data, dict) else None
    return ValueError(f"The knowledge service refused to {action} (HTTP {status}" + (f", {code}" if isinstance(code, str) else "") + ").")


def space_request(settings):
    """Create the project space with Recall's documented processing approvals."""
    space, provider, model = settings["project"], settings["provider"], settings["model"]
    service = json.loads((ROOT / "compose.json").read_text())["services"]["elephant"]["environment"]
    embedding_model, dimensions = service["PIPELINE_EMBED_MODEL"], int(service["PIPELINE_EMBED_DIMS"])
    connection = lambda name, kind: {"id": name, "provider": kind, "account_id": space, "endpoint": PROVIDER_ENDPOINTS[kind],
                                     "credential_ref": PROVIDER_KEYS[kind], "credential_generation": 1, "revoked": False}
    connections = {"embedding": connection("embedding", "openai")}
    if provider != "openai":
        connections["generation"] = connection("generation", provider)
    profiles = {
        "embedding-v1": {"id": "embedding-v1", "connection_id": "embedding", "model": embedding_model,
                         "model_version": embedding_model, "dimensions": dimensions, "preprocessing": "text-v1"},
        "generation-v1": {"id": "generation-v1", "connection_id": "generation" if provider != "openai" else "embedding",
                          "model": model, "model_version": model, "dimensions": None, "preprocessing": "text-v1"},
    }
    bindings = {operation: {"id": f"{operation}-v1", "revoked": False,
                            "profile_id": "embedding-v1" if operation.endswith("_embedding") else "generation-v1"}
                for operation in APPROVED_OPERATIONS}
    return {"name": space, "security": SPACE_SECURITY, "embedding_provider": "openai",
            "embedding_model": embedding_model, "embedding_dims": dimensions,
            "processing": {"generation": 1, "index_generation": 0, "policy_id": "mobkit-launch", "connections": connections,
                           "profiles": profiles, "bindings": bindings, "active_embedding_profile_id": "embedding-v1",
                           "pending_reindex": None}}


def provision(settings, values):
    """Create the project space and the extractor grant once; later runs only read them back.

    A short-lived bootstrap token does the administration. It is never stored.
    """
    space = settings["project"]
    token = bootstrap_token(values["JWT_SECRET"], space)
    status, config = api_request(settings, "GET", f"/v1/spaces/{space}", token)
    if status == 404:
        status, created = api_request(settings, "POST", "/v1/spaces", token, body=space_request(settings))
        if status not in (201, 409):
            raise refusal("create the project space", status, created)
        status, config = api_request(settings, "GET", f"/v1/spaces/{space}", token)
    if status != 200 or not isinstance(config, dict):
        raise refusal("read the project space", status, config)
    processing = config.get("processing") or {}
    missing = [operation for operation in APPROVED_OPERATIONS if operation not in (processing.get("bindings") or {})]
    if missing:
        raise ValueError("The project space exists without approved " + ", ".join(missing)
                         + " processing. Approve them with the knowledge service's space administration, then run up again.")
    grant_path = f"/v1/spaces/{space}/grants/{WORKER_SUBJECT}"
    status, grant = api_request(settings, "GET", grant_path, token)
    if status == 404:
        # The grant write is compare-and-swap, so it is made only when absent.
        status, written = api_request(settings, "PUT", grant_path, token,
                                      body={"active": True, "scopes": WORKER_SCOPES, "expected_generation": None})
        if status != 200:
            raise refusal("grant the extractor read access", status, written)
        status, grant = api_request(settings, "GET", grant_path, token)
    if (status != 200 or not isinstance(grant, dict) or grant.get("active") is not True
            or not set(WORKER_SCOPES) <= set(grant.get("scopes") or [])):
        raise ValueError(f"The {WORKER_SUBJECT} grant for this space is missing, inactive or narrower than the extractor's "
                         "read scopes. Restore it with the knowledge service's grant administration, then run up again.")
    return {"space": space, "processing_generation": processing.get("generation"),
            "extractor_grant": {"subject": WORKER_SUBJECT, "generation": grant.get("generation"),
                                "scopes": sorted(grant.get("scopes") or [])}}


def space_status(settings, *, wait=0):
    """Read the project space's semantic readiness, waiting up to `wait` seconds for it."""
    token = environment().get("ELEPHANT_OPERATOR_TOKEN", "")
    if not token:
        raise ValueError("Configure ELEPHANT_OPERATOR_TOKEN locally before reading the project space status.")
    deadline = time.monotonic() + wait
    while True:
        status, data = api_request(settings, "GET", f"/v1/spaces/{settings['project']}/status", token)
        if status not in (200, 503) or not isinstance(data, dict):
            raise refusal("report the project space status", status, data)
        if data.get("semantic_ingest_ready") is True or time.monotonic() >= deadline:
            return data
        time.sleep(2)


def check_space(token, expected):
    # This preflight only decodes claims. Authentication is verified by the knowledge service.
    if token.count(".") != 2:
        return
    try:
        body = token.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(body + "=" * (-len(body) % 4)))
    except (ValueError, json.JSONDecodeError):
        raise ValueError("The knowledge service JWT could not be decoded.") from None
    if claims.get("space_id") != expected:
        raise ValueError("Knowledge service JWT space does not match the configured project space.")
    if claims.get("exp", 0) <= time.time():
        raise ValueError("Knowledge service token has expired. Run renew for a local service or ask its operator for a new token.")


def result_data(result):
    if getattr(result, "isError", False):
        raise ValueError("The knowledge service rejected the operation; verify the token, profile, scopes, and runtime readiness.")
    structured = getattr(result, "structuredContent", None)
    if structured is not None:
        if isinstance(structured, dict) and structured.get("ok") is False:
            raise ValueError("The knowledge service denied or failed the tool operation. Check the principal scopes and native service diagnostics.")
        return structured
    content = []
    for block in getattr(result, "content", []):
        if getattr(block, "type", None) == "text":
            try:
                content.append(json.loads(block.text))
            except json.JSONDecodeError:
                content.append(block.text)
    if any(isinstance(item, dict) and item.get("ok") is False for item in content):
        raise ValueError("The knowledge service denied or failed the tool operation. Check the principal scopes and native service diagnostics.")
    return content[0] if len(content) == 1 else content


def unwrap(data):
    if isinstance(data, dict) and "data" in data:
        return data["data"]
    return data


async def mcp_operation(settings, operation, *, query=None, path=None, title=None):
    from mcp import ClientSession
    from mcp.client.streamable_http import streamablehttp_client
    env = environment()
    key = "ELEPHANT_OPERATOR_TOKEN" if operation == "ingest" else settings["token_env"]
    token, expected = env.get(key, ""), env.get(settings["space_env"], "")
    if not token or not expected:
        raise ValueError(f"Configure {key} and {settings['space_env']} locally before connecting.")
    check_space(token, expected)
    url = settings["url"].rstrip("/")
    if operation == "ingest":
        url = url.removesuffix("/agent")
    async with streamablehttp_client(url, headers={"Authorization": "Bearer " + token}) as (read, write, _):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools = (await session.list_tools()).tools
            by_name = {tool.name: tool for tool in tools}
            required_tools = ["get_runtime_status"]
            if operation != "ingest":
                required_tools.append("get_capabilities")
            for required in required_tools:
                if required not in by_name:
                    raise ValueError(f"The knowledge service did not expose {required}; check profile and read scopes.")
            health = result_data(await session.call_tool("get_health", {})) if "get_health" in by_name else {"scope": "project", "detail": "Global health is unavailable to this space-scoped principal; authenticated runtime status and capabilities follow."}
            capabilities = result_data(await session.call_tool("get_capabilities", {})) if "get_capabilities" in by_name else {"tools": sorted(by_name)}
            runtime = result_data(await session.call_tool("get_runtime_status", {}))
            if operation == "verify":
                return {"connected": True, "expected_space": expected, "profile": "agent", "tools": sorted(by_name), "health": health, "runtime": runtime, "capabilities": capabilities}
            if operation == "search":
                if "search_all" not in by_name:
                    raise ValueError("The read-only token cannot access search_all.")
                if "query" not in by_name["search_all"].inputSchema.get("properties", {}):
                    raise ValueError("The live search_all schema does not match this helper.")
                return result_data(await session.call_tool("search_all", {"query": query}))
            status = unwrap(runtime)
            if not isinstance(status, dict) or status.get("semantic_ingest_ready") is not True:
                raise ValueError("The knowledge service's semantic pipeline is not ready. Run verify and resolve the native readiness issues before ingesting.")
            tool = by_name.get("ingest_doc_simple")
            if tool is None or not {"title", "text", "metadata"}.issubset(tool.inputSchema.get("properties", {})):
                raise ValueError("The live ingest_doc_simple schema does not match this helper. Inspect it with the knowledge service operator tools.")
            source = Path(path).resolve(strict=True)
            if not source.is_file() or source.stat().st_size > 2_000_000:
                raise ValueError("Choose a UTF-8 text file of at most 2 MB for this ingestion helper.")
            raw = source.read_bytes()
            text = raw.decode("utf-8")
            if not text.strip():
                raise ValueError("The file is empty.")
            sha = hashlib.sha256(raw).hexdigest()
            receipt = result_data(await session.call_tool("ingest_doc_simple", {"title": title or source.name, "text": text, "collection": settings["project"], "metadata": {"original_filename": source.name, "source_sha256": sha, "ingested_by": "mobkit-launch-operator"}}))
            return {"source_sha256": sha, "receipt": receipt, "note": "This is an ingestion receipt. Follow native pipeline status for extraction completion."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ["setup", "renew", "up", "stop", "verify"]:
        commands.add_parser(name)
    search = commands.add_parser("search")
    search.add_argument("query")
    ingest = commands.add_parser("ingest")
    ingest.add_argument("path")
    ingest.add_argument("--title")
    args = parser.parse_args()
    settings = load_settings()
    if args.command in {"setup", "renew"}:
        setup(settings, renew=args.command == "renew")
    elif args.command in {"up", "stop"}:
        if settings["mode"] != "local":
            raise ValueError("This command applies only to a bundled local knowledge service.")
        if args.command == "stop":
            compose("stop")
        else:
            missing_secrets = require_retained_secrets(settings, read_env(ROOT / ".env"), read_env(ROOT.parent / ".env"))
            local_preflight()
            if missing_secrets:
                setup(settings)
            source_checkout(settings)
            # Only after a valid checkout, so a failed fetch leaves every private file as it was.
            add_missing_worker_token(settings)
            compose("up", "--build", "--detach", "--wait", "--wait-timeout", "180")
            values = read_env(ROOT / ".env")
            provisioned = provision(settings, values)
            readiness = space_status(settings, wait=READY_TIMEOUT)
            result = asyncio.run(asyncio.wait_for(mcp_operation(settings, "verify"), timeout=60))
            print(json.dumps({**result, "provisioning": provisioned, "space_status": readiness}, indent=2))
    else:
        result = asyncio.run(asyncio.wait_for(mcp_operation(settings, args.command, query=getattr(args, "query", None), path=getattr(args, "path", None), title=getattr(args, "title", None)), timeout=90))
        if args.command == "verify" and settings["mode"] == "local":
            result["space_status"] = space_status(settings)
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError, TimeoutError) as error:
        # Provider/HTTP errors can contain credential context; unexpected SDK
        # errors are deliberately summarized instead of printing transport data.
        print(f"Knowledge setup stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
    except Exception as error:
        print(f"Knowledge connection failed ({type(error).__name__}). Check local credentials and native service logs.", file=sys.stderr)
        raise SystemExit(1)
