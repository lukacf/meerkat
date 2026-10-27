"""Inert official-client import failures settle the original connection owner."""
import ast
import asyncio
import builtins
from contextlib import asynccontextmanager
import hashlib
import os
from pathlib import Path
from types import ModuleType, SimpleNamespace

import pytest

from test_compiler import load_template


PRIVATE_VALUES = ("private-endpoint-canary", "private-command-canary",
                  "private-header-canary", "private-import-payload-canary")


@pytest.fixture
def clients(monkeypatch):
    module = load_template("mcp_connections")
    if os.environ.get("FORGE_TEST_MCP_IMPORT_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-mcp-import-recovery-20261010/mcp_connections.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "1aece96340de686ecb7be0b0e3e97a1474e31c9a417e8d068cbf58010bf211e4"
        owner = next(node for node in ast.parse(source.read_bytes()).body
                     if isinstance(node, ast.ClassDef) and node.name == "MCPConnection")
        original = next(node for node in owner.body
                        if isinstance(node, ast.AsyncFunctionDef) and node.name == "_serve")
        namespace = {}
        exec(compile(ast.Module(body=[original], type_ignores=[]), str(source), "exec"), module.__dict__, namespace)
        module.MCPConnection._serve = namespace["_serve"]
    observed = SimpleNamespace(module=module, imports=[], contexts=[], calls=[],
        parameters=[], failure=None, block_initialize=False,
        initialize_entered=asyncio.Event(), cleanup_entered=asyncio.Event(),
        release_cleanup=asyncio.Event(), cleanup_finished=asyncio.Event())
    original_import = builtins.__import__
    observed.wait = asyncio.wait_for

    @asynccontextmanager
    async def stdio(parameters):
        observed.parameters.append(parameters)
        observed.contexts.append(("transport-enter", asyncio.current_task()))
        try:
            yield object(), object()
        finally:
            observed.contexts.append(("transport-exit", asyncio.current_task()))

    @asynccontextmanager
    async def http(url, headers, timeout):
        assert timeout == 30
        observed.parameters.append((url, dict(headers)))
        observed.contexts.append(("transport-enter", asyncio.current_task()))
        try:
            yield object(), object(), object()
        finally:
            observed.contexts.append(("transport-exit", asyncio.current_task()))

    class Session:
        def __init__(self, reader, writer, read_timeout_seconds):
            assert read_timeout_seconds.total_seconds() == 120

        async def __aenter__(self):
            observed.contexts.append(("session-enter", asyncio.current_task()))
            return self

        async def __aexit__(self, *args):
            observed.contexts.append(("session-exit", asyncio.current_task()))
            if observed.block_initialize:
                observed.cleanup_entered.set()
                await observed.release_cleanup.wait()
                observed.cleanup_finished.set()

        async def initialize(self):
            observed.initialize_entered.set()
            if observed.block_initialize:
                await asyncio.Event().wait()

        async def list_tools(self, cursor=None):
            assert cursor is None
            return SimpleNamespace(tools=[SimpleNamespace(name="inspect")], nextCursor=None)

        async def call_tool(self, name, arguments, read_timeout_seconds):
            assert read_timeout_seconds.total_seconds() == 120
            observed.calls.append((name, dict(arguments)))
            return SimpleNamespace(isError=False,
                model_dump=lambda **kwargs: {"isError": False, "content": [{"type": "text", "text": "inert result"}]})

    package, stdio_module, http_module = (ModuleType(name) for name in
        ("mcp", "mcp.client.stdio", "mcp.client.streamable_http"))
    package.ClientSession = Session
    package.StdioServerParameters = lambda **kwargs: SimpleNamespace(**kwargs)
    stdio_module.stdio_client = stdio
    http_module.streamablehttp_client = http
    held_modules = {item.__name__: item for item in (package, stdio_module, http_module)}

    def controlled_import(name, globals=None, locals=None, fromlist=(), level=0):
        if name in held_modules and level == 0:
            observed.imports.append(name)
            if name == "mcp" and observed.failure in {"first-package", "transitive"}:
                missing = "mcp" if observed.failure == "first-package" else "private_transitive_dependency"
                raise ModuleNotFoundError(f"private-import-payload-canary: {missing}", name=missing)
            if observed.failure == "stdio" and name == "mcp.client.stdio":
                raise ImportError("private-import-payload-canary: stdio dependency", name=name)
            if observed.failure == "http" and name == "mcp.client.streamable_http":
                raise ImportError("private-import-payload-canary: HTTP dependency", name=name)
            return held_modules[name]
        return original_import(name, globals, locals, fromlist, level)

    monkeypatch.setattr(builtins, "__import__", controlled_import)
    observed.connection = module.MCPConnection({"name": "Observatory instruments",
        "transport": "stdio", "command": "private-command-canary",
        "url": "https://private-endpoint-canary.invalid/",
        "headers": {"Authorization": "private-header-canary"}})
    return observed


async def expect_safe_start_failure(clients, monkeypatch, error_type):
    connection = clients.connection
    cleanup_entered, release_cleanup = asyncio.Event(), asyncio.Event()
    cleanup_finished = asyncio.Event()
    original_close = connection.close
    owners, deadlines = [], []
    queued = asyncio.get_running_loop().create_future()
    await connection.queue.put(("inspect", {}, queued))

    async def wait_for_readiness(future, timeout):
        assert timeout == 45, "The production handshake deadline remains unchanged."
        owner = connection.task
        await asyncio.gather(owner, return_exceptions=True)
        if not connection.ready.done():
            # Only accelerate the historical deadline after its owner has died.
            deadlines.append(owner)
            raise TimeoutError()
        return await future

    async def close():
        owners.append(connection.task)
        await original_close()
        cleanup_entered.set()
        await release_cleanup.wait()
        cleanup_finished.set()

    monkeypatch.setattr(clients.module.asyncio, "wait_for", wait_for_readiness)
    monkeypatch.setattr(connection, "close", close)
    starter = asyncio.create_task(connection.start())
    try:
        await clients.wait(cleanup_entered.wait(), timeout=30)
        assert not starter.done() and not cleanup_finished.is_set()
        assert len(owners) == 1 and owners[0].done() and connection.task is None
        assert connection.ready.done(), "The original readiness future must complete instead of waiting for a timeout."
        assert not connection.ready.cancelled() and not deadlines
        assert queued.done() and connection.queue.empty()
        with pytest.raises(RuntimeError, match="MCP connection is unavailable"):
            queued.result()
        release_cleanup.set()
        with pytest.raises(RuntimeError) as caught:
            await clients.wait(starter, timeout=2)
        assert str(caught.value) == (
            f"MCP connection Observatory instruments failed ({error_type}). "
            "Check the endpoint, command and local credentials.")
        assert all(value not in str(caught.value) for value in PRIVATE_VALUES)
        assert caught.value.__cause__ is None and caught.value.__context__ is None
        assert cleanup_finished.is_set() and connection.task is None
        assert clients.contexts == [] and clients.parameters == [] and clients.calls == []
    finally:
        release_cleanup.set()
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await original_close()
        if queued.done() and not queued.cancelled():
            queued.exception()
        else:
            queued.cancel()
        if connection.ready.done() and not connection.ready.cancelled():
            connection.ready.exception()
        elif not connection.ready.done():
            connection.ready.cancel()


@pytest.mark.parametrize("failure,error_type,imports", [
    ("first-package", "ModuleNotFoundError", ["mcp"]),
    ("transitive", "ModuleNotFoundError", ["mcp"]),
    ("stdio", "ImportError", ["mcp", "mcp.client.stdio"]),
    ("http", "ImportError", ["mcp", "mcp.client.stdio", "mcp.client.streamable_http"]),
])
async def test_import_failure_completes_readiness_after_original_owner_cleanup(
        clients, monkeypatch, failure, error_type, imports):
    clients.failure = failure
    await expect_safe_start_failure(clients, monkeypatch, error_type)
    assert clients.imports == imports


async def test_invalid_transport_configuration_retains_safe_failure_and_cleanup(clients, monkeypatch):
    monkeypatch.delenv("FORGE_IMPORT_RECOVERY_REQUIRED", raising=False)
    clients.connection.config["command"] = "${FORGE_IMPORT_RECOVERY_REQUIRED}"
    await expect_safe_start_failure(clients, monkeypatch, "RuntimeError")
    assert clients.imports == ["mcp", "mcp.client.stdio", "mcp.client.streamable_http"]


@pytest.mark.parametrize("transport", ["stdio", "http"])
async def test_successful_inert_clients_keep_transport_contexts_in_original_owner(clients, transport):
    connection = clients.connection
    connection.config["transport"] = transport
    try:
        await clients.wait(connection.start(), timeout=2)
        owner = connection.task
        assert connection.ready.result() is None and not owner.done()
        result = await clients.wait(connection.call("inspect", {"subject": "inert"}), timeout=2)
        assert result == {"isError": False, "content": [{"type": "text", "text": "inert result"}]}
        assert clients.calls == [("inspect", {"subject": "inert"})]
        await clients.wait(connection.close(), timeout=2)
        assert owner.done() and connection.task is None
        assert [name for name, _ in clients.contexts] == ["transport-enter", "session-enter", "session-exit", "transport-exit"]
        assert all(task is owner for _, task in clients.contexts)
        assert clients.imports == ["mcp", "mcp.client.stdio", "mcp.client.streamable_http"]
    finally:
        await connection.close()


async def test_start_cancellation_joins_original_inert_transport_cleanup(clients):
    connection = clients.connection
    clients.block_initialize = True
    starter = asyncio.create_task(connection.start())
    try:
        await clients.wait(clients.initialize_entered.wait(), timeout=30)
        owner = connection.task
        starter.cancel()
        await clients.wait(clients.cleanup_entered.wait(), timeout=30)
        assert not starter.done() and not owner.done() and not clients.cleanup_finished.is_set()
        assert not connection.ready.cancelled()
        clients.release_cleanup.set()
        with pytest.raises(asyncio.CancelledError):
            await clients.wait(starter, timeout=2)
        assert clients.cleanup_finished.is_set() and owner.done() and connection.task is None
        assert connection.ready.done() and not connection.ready.cancelled()
        assert isinstance(connection.ready.exception(), RuntimeError)
        assert [name for name, _ in clients.contexts] == ["transport-enter", "session-enter", "session-exit", "transport-exit"]
        assert all(task is owner for _, task in clients.contexts)
    finally:
        clients.release_cleanup.set()
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await connection.close()
        if connection.ready.done() and not connection.ready.cancelled():
            connection.ready.exception()
