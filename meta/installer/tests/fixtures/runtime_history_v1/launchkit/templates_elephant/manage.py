#!/usr/bin/env python3
"""Operator-only Elephant setup and schema-first verification for this project."""
from __future__ import annotations

import argparse
import asyncio
import base64
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
import subprocess
import sys
import time
import tempfile

ROOT = Path(__file__).resolve().parent
READ_SCOPES = ["read:assertion", "read:audit", "read:doc", "read:doc:metadata", "read:doc:raw", "read:entity", "read:event", "read:predicate", "read:relationship", "read:source", "read:source:raw", "read:space_config", "read:system", "read:system_event", "read:work_item"]
PREREQUISITE_TIMEOUT = 10


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


def write_env(path, values):
    if path.exists() and path.is_symlink():
        raise ValueError("Refusing to replace a symlinked environment file.")
    for key, value in values.items():
        if any(char in value for char in "\r\n\x00'\\"):
            raise ValueError(f"{key} contains characters unsupported by the local dotenv format.")
    fd, temporary = tempfile.mkstemp(prefix=".env-", dir=path.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w") as stream:
            # Single-quoted dotenv values disable Docker Compose interpolation.
            for key, value in sorted(values.items()):
                stream.write(f"{key}='{value}'\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def jwt_token(secret, space, *, operator=False, now=None):
    now = int(time.time()) if now is None else now
    claims = {"sub": f"mobkit-launch:{space}:" + ("operator" if operator else "agents"), "iat": now, "exp": now + 90 * 86400,
              "iss": "mobkit-launch", "aud": [space], "space_id": space, "scopes": ["*"] if operator else READ_SCOPES,
              "security_level": 100, "security_labels": [], "purpose": ["project-knowledge"]}
    encode = lambda value: base64.urlsafe_b64encode(json.dumps(value, separators=(",", ":")).encode()).decode().rstrip("=")
    body = encode({"alg": "HS256", "typ": "JWT"}) + "." + encode(claims)
    signature = base64.urlsafe_b64encode(hmac.new(secret.encode(), body.encode(), hashlib.sha256).digest()).decode().rstrip("=")
    return body + "." + signature


def setup(settings, renew=False):
    if settings["mode"] != "local":
        raise ValueError("Remote services are managed by their operator. Add the token and expected space to the project's .env, then run verify.")
    values = read_env(ROOT / ".env")
    project_values = read_env(ROOT.parent / ".env")
    if renew and not values.get("JWT_SECRET"):
        raise ValueError("Run setup before renewing tokens.")
    values.setdefault("JWT_SECRET", secrets.token_urlsafe(48))
    values.setdefault("SURREALDB_PASS", secrets.token_urlsafe(36))
    provider_key = {"openai": "OPENAI_API_KEY", "anthropic": "ANTHROPIC_API_KEY", "gemini": "GEMINI_API_KEY"}[settings["provider"]]
    for key in sorted({"OPENAI_API_KEY", provider_key}):
        value = values.get(key) or project_values.get(key) or os.environ.get(key)
        if not value:
            value = getpass.getpass(f"{key} (stored locally, input hidden): ").strip()
        if not value:
            raise ValueError(f"{key} is required for the knowledge pipeline.")
        values[key] = value
    token_name, space_name = settings["token_env"], settings["space_env"]
    values[token_name] = jwt_token(values["JWT_SECRET"], settings["project"])
    values["ELEPHANT_OPERATOR_TOKEN"] = jwt_token(values["JWT_SECRET"], settings["project"], operator=True)
    values[space_name] = settings["project"]
    project_values[token_name] = values[token_name]
    project_values[space_name] = values[space_name]
    write_env(ROOT / ".env", values)
    write_env(ROOT.parent / ".env", project_values)
    print("Knowledge credentials saved locally. Agent token is read-only; the operator token stays in knowledge/.env. Tokens expire in 90 days.")


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
    source.mkdir(mode=0o700)
    command(["git", "-C", str(source), "init"])
    command(["git", "-C", str(source), "remote", "add", "origin", repository])
    command(["git", "-C", str(source), "fetch", "--depth=1", "origin", revision])
    command(["git", "-C", str(source), "checkout", "--detach", "FETCH_HEAD"])
    actual = command(["git", "-C", str(source), "rev-parse", "HEAD"], capture=True).stdout.strip()
    if actual != revision:
        raise ValueError("Knowledge source checkout did not match the pinned revision.")


def compose(*args):
    if not shutil.which("docker"):
        raise ValueError("Install Docker with Compose before starting local knowledge.")
    command(["docker", "compose", "--env-file", str(ROOT / ".env"), "-f", str(ROOT / "compose.json"), *args])


def environment():
    return {**read_env(ROOT.parent / ".env"), **read_env(ROOT / ".env"), **os.environ}


def check_space(token, expected):
    # This preflight only decodes claims. Authentication is verified by Elephant.
    if token.count(".") != 2:
        return
    try:
        body = token.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(body + "=" * (-len(body) % 4)))
    except (ValueError, json.JSONDecodeError):
        raise ValueError("The Elephant JWT could not be decoded.") from None
    if claims.get("space_id") != expected:
        raise ValueError("Elephant JWT space does not match the configured project space.")
    if claims.get("exp", 0) <= time.time():
        raise ValueError("Elephant token has expired. Run renew for a local service or ask its operator for a new token.")


def result_data(result):
    if getattr(result, "isError", False):
        raise ValueError("Elephant rejected the operation; verify the token, profile, scopes, and runtime readiness.")
    structured = getattr(result, "structuredContent", None)
    if structured is not None:
        if isinstance(structured, dict) and structured.get("ok") is False:
            raise ValueError("Elephant denied or failed the tool operation. Check the principal scopes and native service diagnostics.")
        return structured
    content = []
    for block in getattr(result, "content", []):
        if getattr(block, "type", None) == "text":
            try:
                content.append(json.loads(block.text))
            except json.JSONDecodeError:
                content.append(block.text)
    if any(isinstance(item, dict) and item.get("ok") is False for item in content):
        raise ValueError("Elephant denied or failed the tool operation. Check the principal scopes and native service diagnostics.")
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
                    raise ValueError(f"Elephant did not expose {required}; check profile and read scopes.")
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
                raise ValueError("Elephant's semantic pipeline is not ready. Run verify and resolve the native readiness issues before ingesting.")
            tool = by_name.get("ingest_doc_simple")
            if tool is None or not {"title", "text", "metadata"}.issubset(tool.inputSchema.get("properties", {})):
                raise ValueError("The live ingest_doc_simple schema does not match this helper. Inspect it with the Elephant operator tools.")
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
            local_preflight()
            if not (ROOT / ".env").exists():
                setup(settings)
            source_checkout(settings)
            compose("up", "--build", "--detach", "--wait", "--wait-timeout", "180")
            result = asyncio.run(asyncio.wait_for(mcp_operation(settings, "verify"), timeout=60))
            print(json.dumps(result, indent=2))
    else:
        result = asyncio.run(asyncio.wait_for(mcp_operation(settings, args.command, query=getattr(args, "query", None), path=getattr(args, "path", None), title=getattr(args, "title", None)), timeout=90))
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
