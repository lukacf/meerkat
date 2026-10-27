"""One-shot, host-authorized MCP observations using MCP SDK 2.2.

This command does not install, authenticate, grant, attach, or judge task success.
The request and emitted launch file are independently pinned UTF-8 JSON. Environment
values are literal: the supplied environment must cover SDK inherited defaults
and preserve launch-file entries. Output budgets bound this report, not the SDK's
transport buffering. SDK-visible schemas are observations, not native digests.
"""
from __future__ import annotations

import argparse
from contextlib import asynccontextmanager
from dataclasses import dataclass
import hashlib
from http.cookiejar import CookieJar, DefaultCookiePolicy
import importlib.metadata
import json
import logging
import math
import os
from pathlib import Path
import re
import signal
import ssl
import stat
import sys
from urllib.parse import urlsplit

import anyio
import httpx2
from jsonschema.validators import validator_for
from referencing import Registry
from mcp import Client, StdioServerParameters, types
from mcp.client.stdio import get_default_environment, stdio_client
from mcp.client.streamable_http import streamable_http_client

MAX_INPUT = 1024 * 1024
MAX_TOOLS = 256
MAX_PAGES = 16
PORTABLE_SCHEMA = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"
HEADER_TOKEN = re.compile(r"[!#$%&'*+.^_`|~0-9A-Za-z-]+")
RESERVED_HEADERS = frozenset((
    "host", "cookie", "cookie2", "set-cookie", "set-cookie2", "authorization-proxy",
    "proxy-authorization", "proxy-authenticate", "proxy-connection", "connection",
    "keep-alive", "transfer-encoding", "content-length", "trailer", "te", "upgrade",
    "accept", "accept-encoding", "content-type", "cache-control", "expect",
    "mcp-session-id", "mcp-protocol-version", "last-event-id",
))


class Rejected(ValueError):
    """A bounded, nonsecret reason code suitable for the report."""


def encoded(value):
    # Local JSON equality/size only; this is not a canonical Toolkit digest.
    return json.dumps(value, ensure_ascii=True, allow_nan=False,
                      sort_keys=True, separators=(",", ":")).encode("utf-8")


def pairs(rows):
    result = {}
    for key, value in rows:
        if key in result:
            raise Rejected("duplicate_json_key")
        result[key] = value
    return result


def pinned_bytes(path, digest):
    if not isinstance(path, str) or not Path(path).is_absolute():
        raise Rejected("absolute_input_path_required")
    if not isinstance(digest, str) or not re.fullmatch("[0-9a-f]{64}", digest):
        raise Rejected("invalid_input_digest")
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_INPUT:
            raise Rejected("regular_bounded_input_required")
        raw = stream.read(MAX_INPUT + 1)
    if len(raw) > MAX_INPUT or hashlib.sha256(raw).hexdigest() != digest:
        raise Rejected("input_size_or_digest_mismatch")
    return raw


def pinned_json(path, digest):
    raw = pinned_bytes(path, digest)
    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=pairs, parse_float=finite_float,
                          parse_constant=lambda _: (_ for _ in ()).throw(Rejected("nonfinite_json")))
        return value
    except (UnicodeError, json.JSONDecodeError, RecursionError) as exc:
        raise Rejected("invalid_json") from exc


def finite_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise Rejected("nonfinite_json")
    return number


def fields(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= value.keys() or value.keys() - set(required) - set(optional):
        raise Rejected("invalid_object_fields")


def text(value, limit=1024):
    if not isinstance(value, str) or not value or len(value) > limit or "\0" in value:
        raise Rejected("invalid_string")
    return value


def environment(value):
    if not isinstance(value, dict) or len(value) > 128:
        raise Rejected("invalid_environment")
    for key, val in value.items():
        text(key, 256)
        if "=" in key or not isinstance(val, str) or "\0" in val or len(val) > 32768:
            raise Rejected("invalid_environment")
    return value


def http_headers(value):
    if not isinstance(value, dict) or len(value) > 64:
        raise Rejected("invalid_http_headers")
    seen, size = set(), 0
    for key, val in value.items():
        if (not isinstance(key, str) or len(key) > 256 or not HEADER_TOKEN.fullmatch(key)
                or not isinstance(val, str) or len(val) > 8192
                or any(ord(char) < 32 or ord(char) > 126 for char in val)):
            raise Rejected("invalid_http_headers")
        name = key.lower()
        if name in seen or name in RESERVED_HEADERS or name.startswith("mcp-param-"):
            raise Rejected("reserved_or_duplicate_http_header")
        seen.add(name)
        size += len(key) + len(val)
    if size > 65536:
        raise Rejected("http_headers_size_limit")
    return value


@dataclass(frozen=True, repr=False)
class HttpConnection:
    # Never include credentials or endpoint text in the object's repr.
    url: httpx2.URL
    headers: dict[str, str]
    verify: ssl.SSLContext | bool


class NoCookies(DefaultCookiePolicy):
    def set_ok(self, cookie, request):
        return False

    def return_ok(self, cookie, request):
        return False


def load_http_request(request):
    if request.get("action") not in ("discover", "call"):
        raise Rejected("unsupported_request_action")
    required = ("schema_version", "action", "connection", "timeout_seconds", "max_report_bytes")
    fields(request, required + (("expected_tools", "calls") if request["action"] == "call" else ()))
    request_bounds(request)
    connection = request["connection"]
    fields(connection, ("transport", "mcp_json", "server", "headers", "mode"), ("ca_certificate",))
    if connection["transport"] != "streamable-http":
        raise Rejected("unsupported_transport")
    if connection["mode"] not in ("auto", "legacy"):
        raise Rejected("unsupported_connection_mode")
    if importlib.metadata.version("httpx2") != "2.13.1":
        raise Rejected("httpx2_2_13_1_required")
    fields(connection["mcp_json"], ("path", "sha256"))
    config = pinned_json(connection["mcp_json"]["path"], connection["mcp_json"]["sha256"])
    fields(config, ("mcpServers",), ("$schema",))
    if "$schema" in config and config["$schema"] != PORTABLE_SCHEMA:
        raise Rejected("unsupported_mcp_schema")
    if not isinstance(config["mcpServers"], dict):
        raise Rejected("invalid_server_map")
    server = config["mcpServers"].get(text(connection["server"]))
    fields(server, ("type", "url"), ("headers",))
    if server["type"] != "streamable-http":
        raise Rejected("unsupported_transport")
    endpoint = text(server["url"], 8192)
    if any(ord(char) <= 32 or ord(char) >= 127 for char in endpoint) or "\\" in endpoint or "#" in endpoint:
        raise Rejected("invalid_https_endpoint")
    parsed = urlsplit(endpoint)
    if parsed.scheme != "https" or not parsed.netloc or "@" in parsed.netloc:
        raise Rejected("invalid_https_endpoint")
    url = httpx2.URL(endpoint)
    if url.scheme != "https" or not url.host or url.userinfo or url.fragment:
        raise Rejected("invalid_https_endpoint")
    configured = http_headers(server.get("headers", {}))
    additional = http_headers(connection["headers"])
    if {name.lower() for name in configured} & {name.lower() for name in additional}:
        raise Rejected("http_header_overlap")
    headers = http_headers(dict(configured, **additional))
    verify = True
    if "ca_certificate" in connection:
        certificate = connection["ca_certificate"]
        fields(certificate, ("path", "sha256"))
        pem = pinned_bytes(certificate["path"], certificate["sha256"]).decode("ascii")
        # Only these verified bytes are trusted. No environment-selected CA path.
        verify = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        verify.load_verify_locations(cadata=pem)
    expected = call_selection(request) if request["action"] == "call" else {}
    return request, HttpConnection(url, headers, verify), expected


def request_bounds(request):
    timeout, budget = request["timeout_seconds"], request["max_report_bytes"]
    if type(timeout) not in (int, float) or not 0.05 <= timeout <= 120:
        raise Rejected("invalid_timeout")
    if type(budget) is not int or not 4096 <= budget <= MAX_INPUT:
        raise Rejected("invalid_report_budget")


def load_request(path, digest):
    request = pinned_json(path, digest)
    if (isinstance(request, dict) and type(request.get("schema_version")) is int
            and request["schema_version"] == 3):
        return load_http_request(request)
    discovery_only = (isinstance(request, dict)
                      and type(request.get("schema_version")) is int
                      and request["schema_version"] == 2)
    if discovery_only:
        fields(request, ("schema_version", "action", "launch", "timeout_seconds", "max_report_bytes"))
        if request["action"] != "discover":
            raise Rejected("unsupported_request_action")
    else:
        fields(request, ("schema_version", "launch", "expected_tools", "calls", "timeout_seconds", "max_report_bytes"))
        if type(request["schema_version"]) is not int or request["schema_version"] != 1:
            raise Rejected("unsupported_request_version")
    request_bounds(request)
    launch = request["launch"]
    fields(launch, ("mcp_json", "server", "cwd", "env", "mode"))
    fields(launch["mcp_json"], ("path", "sha256"))
    config = pinned_json(launch["mcp_json"]["path"], launch["mcp_json"]["sha256"])
    fields(config, ("mcpServers",))
    if not isinstance(config["mcpServers"], dict):
        raise Rejected("invalid_server_map")
    server = config["mcpServers"].get(text(launch["server"]))
    fields(server, ("command",), ("args", "cwd", "env"))
    command = text(server["command"], 8192)
    cwd = text(launch["cwd"], 8192)
    if not Path(command).is_absolute() or not Path(cwd).is_absolute() or not Path(cwd).is_dir():
        raise Rejected("absolute_command_and_existing_cwd_required")
    args = server.get("args", [])
    if not isinstance(args, list) or len(args) > 256 or any(not isinstance(a, str) or "\0" in a for a in args):
        raise Rejected("invalid_arguments")
    env = environment(launch["env"])
    if "cwd" in server and server["cwd"] != cwd:
        raise Rejected("launch_cwd_changed")
    if any(env.get(k) != v for k, v in environment(server.get("env", {})).items()):
        raise Rejected("launch_environment_changed")
    if get_default_environment().keys() - env.keys():
        raise Rejected("uncovered_sdk_environment")
    if launch["mode"] not in ("auto", "legacy"):
        raise Rejected("unsupported_connection_mode")
    if discovery_only:
        return request, StdioServerParameters(command=command, args=args, cwd=cwd, env=env), {}
    return request, StdioServerParameters(command=command, args=args, cwd=cwd, env=env), call_selection(request)


def call_selection(request):
    expected, validators = {}, {}
    tools = request["expected_tools"]
    if not isinstance(tools, list) or not 1 <= len(tools) <= 32:
        raise Rejected("invalid_expected_tools")
    for row in tools:
        fields(row, ("name", "input_schema"), ("output_schema",))
        name = text(row["name"], 256)
        if name in expected or not isinstance(row["input_schema"], dict):
            raise Rejected("invalid_expected_tool")
        if "output_schema" in row and row["output_schema"] is not None and not isinstance(row["output_schema"], dict):
            raise Rejected("invalid_expected_tool")
        expected[name] = row
        cls = validator_for(row["input_schema"])
        cls.check_schema(row["input_schema"])
        validators[name] = cls(row["input_schema"], registry=Registry())
    calls = request["calls"]
    if not isinstance(calls, list) or not 1 <= len(calls) <= 16:
        raise Rejected("invalid_calls")
    for call in calls:
        fields(call, ("name", "arguments"))
        name = text(call["name"], 256)
        if name not in expected or not isinstance(call["arguments"], dict):
            raise Rejected("call_not_selected")
        if not validators[name].is_valid(call["arguments"]):
            raise Rejected("invalid_call_arguments")
    return expected


async def list_tools(session):
    """Collect a complete bounded SDK-visible listing before exposing any tools."""
    found, cursor, seen, size = {}, None, set(), 0
    for _ in range(MAX_PAGES):
        page = await session.list_tools(params=types.PaginatedRequestParams(cursor=cursor) if cursor else None)
        if page.result_type != "complete":
            raise Rejected("catalog_nonfinal_result")
        size += len(encoded(page.model_dump(mode="json", by_alias=True, exclude_none=True)))
        if size > MAX_INPUT:
            raise Rejected("catalog_size_limit")
        for tool in page.tools:
            if tool.name in found or len(found) >= MAX_TOOLS:
                raise Rejected("catalog_duplicate_or_count_limit")
            text(tool.name, 256)
            found[tool.name] = tool
        cursor = page.next_cursor
        if cursor is None:
            break
        if not isinstance(cursor, str) or not cursor or len(cursor) > 4096 or cursor in seen:
            raise Rejected("catalog_cursor_invalid")
        seen.add(cursor)
    else:
        raise Rejected("catalog_page_limit")
    return found


async def discover(session, expected):
    tools = await list_tools(session)
    found = {}
    for name, tool in tools.items():
        row = {"name": tool.name, "input_schema": tool.input_schema}
        if "output_schema" in tool.model_fields_set:
            row["output_schema"] = tool.output_schema
        found[name] = row
    for name, row in expected.items():
        if name not in found or encoded(row) != encoded(found[name]):
            raise Rejected("selected_tool_missing_or_changed")
    return sorted(found.keys() - expected.keys())


async def execute(request, server, expected, report):
    # No callbacks/extensions or high-level call_tool retry driver.
    with open(os.devnull, "w") as errlog:
        @asynccontextmanager
        async def transport():
            if isinstance(server, HttpConnection):
                async def fixed_endpoint(outgoing):
                    if outgoing.url != server.url:
                        raise Rejected("http_endpoint_changed")

                # The SDK does not close a supplied HTTP client. Keep both
                # lifetimes inside the existing deadline and cancellation scope.
                # max_redirects also controls the SDK's own redirect loop.
                async with httpx2.AsyncClient(
                    headers=server.headers, verify=server.verify, trust_env=False,
                    auth=None, cookies=CookieJar(policy=NoCookies()),
                    follow_redirects=False, max_redirects=0,
                    timeout=httpx2.Timeout(request["timeout_seconds"]),
                    limits=httpx2.Limits(max_connections=4, max_keepalive_connections=2),
                    event_hooks={"request": [fixed_endpoint]},
                ) as http_client:
                    async with streamable_http_client(str(server.url), http_client=http_client) as streams:
                        yield streams
            else:
                async with stdio_client(server, errlog=errlog) as streams:
                    yield streams
            report["transport_context_closed"] = True

        options = request["connection"] if isinstance(server, HttpConnection) else request["launch"]
        async with Client(transport(), mode=options["mode"], cache=None) as client:
            report["protocol_version"] = client.protocol_version
            report["phase"] = "discovery"
            if request.get("action") == "discover":
                try:
                    tools = await list_tools(client.session)
                except Rejected as exc:
                    report["reason"] = str(exc)
                    return
                # Preserve explicit null and absence in the SDK-visible model.
                # Descriptions, annotations and metadata remain untrusted data.
                report["tools"] = [tool.model_dump(mode="json", by_alias=True, exclude_unset=True)
                                   for tool in tools.values()]
                if len(encoded(report)) + 512 > request["max_report_bytes"]:
                    del report["tools"]
                    report["reason"] = "catalog_report_limit"
                    return
                report["status"] = "observed"
                report["phase"] = "complete"
                return
            try:
                extra = await discover(client.session, expected)
            except Rejected as exc:
                report["reason"] = str(exc)
                return
            report["additional_tool_count"] = len(extra)
            report["additional_tool_names"] = extra
            if len(encoded(report)) + 2048 > request["max_report_bytes"]:
                report["reason"] = "catalog_report_limit"
                return
            for index, call in enumerate(request["calls"]):
                if index:
                    report["phase"] = "discovery"
                    try:
                        await discover(client.session, expected)
                    except Rejected as exc:
                        report["reason"] = str(exc)
                        return
                report["phase"] = "call"
                observation = {"index": index, "name": call["name"], "outcome": "unknown"}
                report["calls"].append(observation)
                result = await client.session.call_tool(call["name"], call["arguments"], allow_input_required=True)
                if isinstance(result, types.InputRequiredResult):
                    observation["outcome"] = "input_required"
                    report["reason"] = "input_required_no_retry"
                    return
                if not isinstance(result, types.CallToolResult) or result.result_type != "complete":
                    report["reason"] = "nonfinal_result_no_retry"
                    return
                observation["outcome"] = "tool_error" if result.is_error else "response_received"
                value = result.model_dump(mode="json", by_alias=True, exclude_none=True)
                observation["result"] = value
                if len(encoded(report)) + 512 > request["max_report_bytes"]:
                    del observation["result"]
                    report["reason"] = "response_omitted_report_limit"
                    return
                if result.is_error:
                    report["reason"] = "tool_error_no_retry"
                    return
            report["status"] = "observed"
            report["phase"] = "complete"


def new_report():
    return {"schema_version": 1, "status": "stopped", "phase": "preflight", "calls": [],
            "transport_context_closed": False}


async def verify(path, digest, report=None):
    if report is None:
        report = new_report()
    budget = 4096
    try:
        if importlib.metadata.version("mcp") != "2.2.0":
            raise Rejected("mcp_2_2_0_required")
        request, server, expected = load_request(path, digest)
        if request["schema_version"] == 2:
            report.update(schema_version=2, action="discover",
                          scope="untrusted_sdk_visible_tool_declarations")
        elif request["schema_version"] == 3:
            report.update(schema_version=3, action=request["action"], transport="streamable-http",
                          transport_context_scope="local_http_and_mcp_contexts")
            if request["action"] == "discover":
                report["scope"] = "untrusted_sdk_visible_tool_declarations"
            certificate = request["connection"].get("ca_certificate")
            report["tls_trust"] = "pinned_ca" if certificate else "system"
            if certificate:
                report["ca_certificate_sha256"] = certificate["sha256"]
        report["request_sha256"] = digest
        options = request["connection"] if request["schema_version"] == 3 else request["launch"]
        report["mcp_json_sha256"] = options["mcp_json"]["sha256"]
        budget = request["max_report_bytes"]
        report["phase"] = "connection"
        with anyio.fail_after(request["timeout_seconds"]):
            await execute(request, server, expected, report)
    except Rejected as exc:
        report["status"] = "stopped"
        report["reason"] = str(exc)
    except TimeoutError:
        report["status"] = "stopped"
        report["reason"] = "deadline_exceeded"
    except Exception:
        # Exception text can contain environment values, arguments or auth URLs.
        report["status"] = "stopped"
        report["reason"] = "sdk_or_input_failure"
    finally:
        if len(encoded(report)) + 128 > budget:
            if "tools" in report:
                del report["tools"]
                report["status"] = "stopped"
                report["reason"] = "catalog_report_limit"
            report.pop("additional_tool_names", None)
            for call in report["calls"]:
                call.pop("result", None)
                call.pop("name", None)
    return report


async def run(path, digest):
    result = new_report()
    interrupted = False
    # Keep OS handlers installed through SDK cleanup after the first signal.
    # Later signals are queued, not restored to default mid-shutdown.
    with anyio.open_signal_receiver(signal.SIGINT, signal.SIGTERM, signal.SIGHUP) as receiver:
        with anyio.CancelScope() as scope:
            async with anyio.create_task_group() as group:
                async def watch_signals():
                    nonlocal interrupted
                    async for _ in receiver:
                        interrupted = True
                        scope.cancel()
                        return
                group.start_soon(watch_signals)
                await verify(path, digest, result)
                group.cancel_scope.cancel()
    if interrupted:
        result["status"] = "stopped"
        result["reason"] = "interrupted_outcome_unknown"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True)
    parser.add_argument("--request-sha256", required=True)
    args = parser.parse_args()
    # Server logs and SDK diagnostics are untrusted and may contain auth details.
    logging.disable(logging.CRITICAL)
    report = anyio.run(run, args.request, args.request_sha256)
    sys.stdout.buffer.write(encoded(report) + b"\n")
    return 0 if report["status"] == "observed" else 2


if __name__ == "__main__":
    raise SystemExit(main())
