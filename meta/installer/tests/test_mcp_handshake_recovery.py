"""Named handshake timeout recovery with inert tasks and original cleanup."""
import ast
import asyncio
import hashlib
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from test_compiler import load_template


@pytest.fixture
def handshake(monkeypatch):
    module = load_template("mcp_connections")
    if os.environ.get("FORGE_TEST_MCP_HANDSHAKE_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-mcp-handshake-recovery-20261010/mcp_connections.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "023acfe50b7f800dfed7c18a2ed4ea21e6b4dd64bedb5fa690c4a5118c29324c"
        owner = next(node for node in ast.parse(source.read_bytes()).body
                     if isinstance(node, ast.ClassDef) and node.name == "MCPConnection")
        original = next(node for node in owner.body
                        if isinstance(node, ast.AsyncFunctionDef) and node.name == "start")
        namespace = {}
        exec(compile(ast.Module(body=[original], type_ignores=[]), str(source), "exec"), module.__dict__, namespace)
        module.MCPConnection.start = namespace["start"]
    observation = SimpleNamespace(module=module, entered=asyncio.Event(),
        cleanup_started=asyncio.Event(), release_cleanup=asyncio.Event(),
        cleanup_finished=asyncio.Event(), owner_tasks=[], mode="timeout", failure=None)
    original_wait = asyncio.wait_for
    never = asyncio.Event()

    async def serve(connection):
        observation.owner_tasks.append(asyncio.current_task())
        observation.entered.set()
        try:
            if observation.mode == "success":
                connection.ready.set_result(None)
            elif observation.mode == "failure":
                connection.ready.set_exception(observation.failure)
            await never.wait()
        finally:
            observation.cleanup_started.set()
            await observation.release_cleanup.wait()
            observation.cleanup_finished.set()

    async def wait(future, timeout):
        assert timeout == 45
        await observation.entered.wait()
        if observation.mode == "timeout":
            raise TimeoutError()
        return await future

    monkeypatch.setattr(module.MCPConnection, "_serve", serve)
    monkeypatch.setattr(module.asyncio, "wait_for", wait)
    observation.wait = original_wait
    observation.connection = module.MCPConnection({"name": "Observatory calendar",
        "transport": "http", "url": "https://private-endpoint.invalid/token-canary",
        "command": "secret-command-canary", "headers": {"Authorization": "secret-header-canary"}})
    return observation


async def settle_cleanup(observation, starter):
    await observation.wait(observation.cleanup_started.wait(), timeout=30)
    assert not starter.done() and not observation.cleanup_finished.is_set()
    assert not observation.connection.ready.cancelled(), "The original readiness future remains shielded."
    observation.release_cleanup.set()


async def test_timeout_names_connection_after_cleanup_and_allows_deliberate_retry(handshake):
    connection = handshake.connection
    starter = asyncio.create_task(connection.start())
    try:
        await settle_cleanup(handshake, starter)
        with pytest.raises(RuntimeError, match="Observatory calendar timed out; no handshake completed") as caught:
            await handshake.wait(starter, timeout=2)
        message = str(caught.value)
        assert "configured command or endpoint and local credentials" in message
        assert "bash start.sh --connections-check" in message
        assert all(secret not in message for secret in
                   ("private-endpoint", "token-canary", "secret-command-canary", "secret-header-canary"))
        assert caught.value.__cause__ is None
        assert handshake.cleanup_finished.is_set() and connection.task is None
        assert handshake.owner_tasks[0].done()
        handshake.mode = "success"
        handshake.entered.clear()
        handshake.cleanup_started.clear()
        handshake.cleanup_finished.clear()
        await handshake.wait(connection.start(), timeout=2)
        assert connection.ready.done() and connection.ready.result() is None
        assert connection.task is handshake.owner_tasks[1] and not connection.task.done()
        assert not handshake.cleanup_finished.is_set()
        await handshake.wait(connection.close(), timeout=2)
        assert handshake.cleanup_finished.is_set() and connection.task is None
    finally:
        handshake.release_cleanup.set()
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await connection.close()


async def test_caller_cancellation_keeps_original_cleanup_and_is_not_a_timeout(handshake):
    handshake.mode = "caller"
    connection = handshake.connection
    starter = asyncio.create_task(connection.start())
    try:
        await handshake.wait(handshake.entered.wait(), timeout=30)
        starter.cancel()
        await settle_cleanup(handshake, starter)
        with pytest.raises(asyncio.CancelledError):
            await handshake.wait(starter, timeout=2)
        assert handshake.cleanup_finished.is_set() and connection.task is None
        assert handshake.owner_tasks[0].done()
    finally:
        handshake.release_cleanup.set()
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await connection.close()


async def test_non_timeout_readiness_failure_is_preserved_after_cleanup(handshake):
    handshake.mode = "failure"
    handshake.failure = RuntimeError("Original inert handshake refusal.")
    connection = handshake.connection
    starter = asyncio.create_task(connection.start())
    try:
        await settle_cleanup(handshake, starter)
        with pytest.raises(RuntimeError) as caught:
            await handshake.wait(starter, timeout=2)
        assert caught.value is handshake.failure
        assert handshake.cleanup_finished.is_set() and connection.task is None
    finally:
        handshake.release_cleanup.set()
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await connection.close()


async def test_timeout_does_not_replace_a_close_failure(handshake, monkeypatch):
    connection = handshake.connection
    original_close = connection.close
    failure = PermissionError("Original inert cleanup refusal.")

    async def close():
        await original_close()
        raise failure

    monkeypatch.setattr(connection, "close", close)
    starter = asyncio.create_task(connection.start())
    try:
        await settle_cleanup(handshake, starter)
        with pytest.raises(PermissionError) as caught:
            await handshake.wait(starter, timeout=2)
        assert caught.value is failure
        assert isinstance(caught.value.__context__, TimeoutError)
        assert handshake.cleanup_finished.is_set() and connection.task is None
    finally:
        handshake.release_cleanup.set()
        if not starter.done():
            starter.cancel()
        await asyncio.gather(starter, return_exceptions=True)
        await original_close()
