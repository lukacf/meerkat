"""Official MCP clients bound to the exact tools each class was granted."""
from __future__ import annotations

import asyncio
import base64
import binascii
import copy
from contextlib import AsyncExitStack
from datetime import timedelta
import hashlib
import json
import os
import re
import time

REFERENCE = re.compile(r"\$\{([A-Z][A-Z0-9_]{0,79})\}")


def configuration_value(field):
    """One effective value for setup, preflight, and MCP transport."""
    return os.environ.get(field.get("key", field.get("name"))) or (field.get("default", "") if not field.get("secret", True) else "")


def expand(value):
    def replace(match):
        key = match.group(1)
        if not os.environ.get(key):
            raise RuntimeError(f"Configure {key} locally in .env before connecting this MCP server.")
        return os.environ[key]
    return REFERENCE.sub(replace, value)


def configured_transport(config):
    """Preserve optional publisher inputs without weakening required references."""
    inputs = {item["name"]: item for item in config.get("setup_inputs", [])}
    def missing_optional(value):
        names = REFERENCE.findall(value)
        return any(not os.environ.get(name) and not inputs.get(name, {}).get("required", True)
                   and not inputs.get(name, {}).get("default") for name in names)
    def resolved(value):
        def replace(match):
            name = match.group(1)
            value = configuration_value({"name": name, **inputs.get(name, {})})
            if not value:
                raise RuntimeError(f"Configure {name} locally in .env before connecting this MCP server.")
            return value
        return REFERENCE.sub(replace, value)
    args = config.get("args", [])
    optional = set(config.get("optional_args", []))
    omitted = {index for index, value in enumerate(args) if index in optional and missing_optional(value)}
    for index in list(omitted):
        if index > 0 and index - 1 in optional and args[index - 1].startswith("-"):
            omitted.add(index - 1)
    return {**config, "command": resolved(config.get("command", "")),
            "url": resolved(config.get("url", "")),
            "args": [resolved(value) for index, value in enumerate(args) if index not in omitted],
            "env": {key: resolved(value) for key, value in config.get("env", {}).items() if not missing_optional(value)},
            "headers": {key: resolved(value) for key, value in config.get("headers", {}).items() if not missing_optional(value)}}


def validate_knowledge_scope(spec):
    """Match local intent to JWT claims before transport; Elephant authenticates."""
    if not any(item["tools"]["knowledge"] for item in spec["classes"]):
        return
    knowledge = spec.get("knowledge", {})
    token_key = knowledge.get("token_env", "ELEPHANT_TOKEN")
    space_key = knowledge.get("space_env", "ELEPHANT_SPACE_ID")
    token = os.environ.get(token_key, "").strip()
    expected = os.environ.get(space_key, "").strip()
    if not token or not expected:
        raise RuntimeError(f"Configure {token_key} and {space_key} locally before connecting Elephant.")
    if len(token) > 32768 or token.count(".") != 2:
        raise RuntimeError("Elephant requires a scoped JWT for this deployment. Opaque tokens cannot verify the configured space.")
    try:
        encoded = token.split(".")[1]
        claims = json.loads(base64.b64decode(encoded + "=" * (-len(encoded) % 4), altchars=b"-_", validate=True))
    except (ValueError, UnicodeError, binascii.Error):
        raise RuntimeError("The Elephant JWT claims could not be decoded.") from None
    if not isinstance(claims, dict) or claims.get("space_id") != expected:
        raise RuntimeError("Elephant JWT space does not match the configured project space. Request a token for the selected space.")
    expires = claims.get("exp")
    if isinstance(expires, bool) or not isinstance(expires, (int, float)) or not expires > time.time():
        raise RuntimeError("Elephant JWT is expired or has no valid expiry. Renew the token before connecting.")
    not_before = claims.get("nbf")
    if not_before is not None and (isinstance(not_before, bool) or not isinstance(not_before, (int, float)) or not_before > time.time()):
        raise RuntimeError("Elephant JWT is not yet valid or has an invalid activation time.")


def tool_alias(connection_id, name):
    # Provider tool names have conservative length/character constraints.
    clean = re.sub(r"[^a-zA-Z0-9_-]", "_", f"mcp_{connection_id}_{name}")[:48]
    digest = hashlib.sha256(f"{connection_id}\0{name}".encode()).hexdigest()[:10]
    return f"{clean}_{digest}"


def _has_regex_lookaround(pattern):
    if not isinstance(pattern, str):
        return False
    index, in_class, class_content, class_caret = 0, False, False, False
    while index < len(pattern):
        character = pattern[index]
        if character == "\\":
            if in_class:
                class_content = True
            index += 2
            continue
        if in_class:
            if character == "]" and class_content:
                in_class = False
            elif character == "^" and not class_content and not class_caret:
                class_caret = True
            else:
                class_content = True
        elif character == "[":
            in_class, class_content, class_caret = True, False, False
        elif pattern.startswith("(?#", index):
            index += 3
            while index < len(pattern) and pattern[index] != ")":
                index += 2 if pattern[index] == "\\" else 1
        elif pattern.startswith(("(?=", "(?!", "(?<=", "(?<!"), index):
            return True
        index += 1
    return False


def model_tool_schema(schema, provider):
    """Project unsupported OpenAI regex syntax without altering owner schemas."""
    if provider != "openai":
        return schema
    projected, changed = copy.deepcopy(schema), False

    def visit(node):
        nonlocal changed
        if not isinstance(node, dict):
            return node
        node = dict(node)
        if _has_regex_lookaround(node.get("pattern")):
            del node["pattern"]
            changed = True
        for key in ("properties", "patternProperties", "$defs", "definitions", "dependentSchemas", "dependencies"):
            children = node.get(key)
            if isinstance(children, dict):
                node[key] = {name: visit(child) for name, child in children.items()}
        for key in ("additionalProperties", "unevaluatedProperties", "propertyNames", "additionalItems",
                    "contains", "unevaluatedItems", "not", "if", "then", "else", "contentSchema"):
            if key in node:
                node[key] = visit(node[key])
        for key in ("allOf", "anyOf", "oneOf", "prefixItems", "items"):
            children = node.get(key)
            if isinstance(children, list):
                node[key] = [visit(child) for child in children]
            elif key == "items" and key in node:
                node[key] = visit(children)
        return node

    projected = visit(projected)
    return projected if changed else schema


def _original_schema_validator(schema):
    from jsonschema.validators import validator_for
    from referencing import Registry
    original = copy.deepcopy(schema)
    try:
        validator_class = validator_for(original)
        validator_class.check_schema(original)
        return validator_class(original, registry=Registry())
    except Exception:
        raise ValueError("The MCP tool input schema cannot be validated.") from None


class MCPConnection:
    def __init__(self, config):
        self.config = config
        self.queue = asyncio.Queue()
        self.task = None
        self.tools = {}
        self.ready = None

    async def start(self):
        self.ready = asyncio.get_running_loop().create_future()
        self.task = asyncio.create_task(self._serve())
        try:
            await asyncio.wait_for(asyncio.shield(self.ready), 45)
        except BaseException:
            await self.close()
            raise

    async def _serve(self):
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client
        from mcp.client.streamable_http import streamablehttp_client
        active = None
        try:
            # anyio transport contexts open and close in this same task.
            async with AsyncExitStack() as stack:
                config = configured_transport(self.config)
                if config["transport"] == "stdio":
                    permitted = {key: os.environ[key] for key in ("PATH", "HOME", "TMPDIR", "TEMP", "SYSTEMROOT", "LANG") if key in os.environ}
                    permitted.update(config.get("env", {}))
                    params = StdioServerParameters(command=config["command"], args=config.get("args", []), env=permitted)
                    reader, writer = await stack.enter_async_context(stdio_client(params))
                else:
                    headers = config.get("headers", {})
                    reader, writer, _ = await stack.enter_async_context(streamablehttp_client(config["url"], headers=headers, timeout=30))
                session = await stack.enter_async_context(ClientSession(reader, writer, read_timeout_seconds=timedelta(seconds=120)))
                await session.initialize()
                cursor = None
                seen_cursors = set()
                while True:
                    page = await session.list_tools(cursor=cursor)
                    for tool in page.tools:
                        if tool.name in self.tools:
                            raise RuntimeError("MCP server returned duplicate tool names.")
                        self.tools[tool.name] = tool
                        if len(self.tools) > 1000:
                            raise RuntimeError("MCP server advertised more than 1000 tools.")
                    cursor = page.nextCursor
                    if not cursor:
                        break
                    if cursor in seen_cursors:
                        raise RuntimeError("MCP server returned a repeated tools cursor.")
                    seen_cursors.add(cursor)
                self.ready.set_result(None)
                while True:
                    item = await self.queue.get()
                    if item is None:
                        break
                    name, arguments, active = item
                    if active.cancelled():
                        active = None
                        continue
                    try:
                        result = await session.call_tool(name, arguments, read_timeout_seconds=timedelta(seconds=120))
                        if result.isError and not self.config.get('preserve_tool_errors', False):
                            # Preserve the server's structured error as a tool error,
                            # without converting failure into a successful answer.
                            raise RuntimeError(f"MCP tool {name} reported an error: {result.model_dump_json(by_alias=True)[:10000]}")
                        value = result.model_dump(mode="json", by_alias=True, exclude_none=True)
                        if len(json.dumps(value)) > 2_000_000:
                            raise RuntimeError("MCP result exceeds the 2 MB tool response limit.")
                        if not active.done():
                            active.set_result(value)
                    except Exception as error:
                        if not active.done():
                            active.set_exception(error)
                    finally:
                        active = None
        except BaseException as error:
            if self.ready is not None and not self.ready.done():
                self.ready.set_exception(RuntimeError(f"MCP connection {self.config['name']} failed ({type(error).__name__}). Check the endpoint, command and local credentials."))
            if active is not None and not active.done():
                active.set_exception(RuntimeError("MCP connection stopped during a tool call."))
            while not self.queue.empty():
                item = self.queue.get_nowait()
                if item and not item[2].done():
                    item[2].set_exception(RuntimeError("MCP connection is unavailable."))
            if isinstance(error, asyncio.CancelledError):
                raise

    async def call(self, name, arguments):
        if name not in self.tools:
            raise ValueError("Unknown MCP tool.")
        if not self.task or self.task.done():
            raise RuntimeError("MCP connection is unavailable. Restart after checking its configuration.")
        future = asyncio.get_running_loop().create_future()
        await self.queue.put((name, arguments, future))
        return await future

    async def close(self):
        if self.task:
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)
            self.task = None


class MCPPool:
    def __init__(self, spec):
        self.spec = spec
        self.connections = {}

    async def start(self):
        validate_knowledge_scope(self.spec)
        used = {grant["connection_id"] for item in self.spec["classes"] for grant in item["tools"]["mcp"]}
        if any(item["tools"]["knowledge"] for item in self.spec["classes"]):
            used.add("elephant")
        try:
            for config in self.spec["mcpConnections"]:
                if config["id"] in used:
                    connection = MCPConnection(config)
                    self.connections[config["id"]] = connection
                    await connection.start()
            # Fail before building agents if a class refers to a missing tool.
            for item in self.spec["classes"]:
                self.for_class(item)
        except BaseException:
            await self.close()
            raise

    def for_class(self, item):
        grants = list(item["tools"]["mcp"])
        if item["tools"]["knowledge"]:
            grants.append({"connection_id": "elephant", "tool_names": []})
        output = []
        for grant in grants:
            connection_id = grant["connection_id"]
            connection = self.connections.get(connection_id)
            if connection is None:
                raise RuntimeError(f"Granted MCP connection {connection_id} was not initialized.")
            names = grant["tool_names"] or sorted(connection.tools)
            missing = set(names) - set(connection.tools)
            if missing:
                raise RuntimeError(f"Class {item['name']} selected tools absent from {connection_id}: {', '.join(sorted(missing))}")
            for name in names:
                tool = connection.tools[name]
                schema = model_tool_schema(tool.inputSchema, item.get("provider"))
                validator = _original_schema_validator(tool.inputSchema) if schema is not tool.inputSchema else None
                async def handler(arguments, *, _connection=connection, _name=name, _validator=validator):
                    if _validator is not None:
                        try:
                            _validator.validate(arguments)
                        except Exception:
                            raise ValueError("MCP tool arguments do not match the advertised input schema.") from None
                    return await _connection.call(_name, arguments)
                output.append({"name": tool_alias(connection_id, name), "handler": handler,
                               "description": f"{connection.config['name']} / {name}: {tool.description or ''}"[:16000],
                               "schema": schema})
        return output

    async def close(self):
        for connection in reversed(list(self.connections.values())):
            await connection.close()
        self.connections.clear()
