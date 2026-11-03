"""Inert request cancellation preserves the existing MCP session owner."""
import asyncio
from contextlib import asynccontextmanager
from types import SimpleNamespace

import pytest

from test_compiler import load_template


@pytest.mark.parametrize("interrupt,preserve_tool_errors", [
    ("caller", False), ("caller", True), ("owner", False)])
async def test_cancelled_active_request_cleans_up_before_next_call_and_owner_close(
        monkeypatch, interrupt, preserve_tool_errors):
    import mcp
    from mcp.client import stdio
    from mcp.types import CallToolResult, TextContent

    module = load_template("mcp_connections")
    active_started, cleanup_started, release_cleanup, cleanup_finished = (
        asyncio.Event() for _ in range(4))
    closing_started, closing_cleaned = asyncio.Event(), asyncio.Event()
    never = asyncio.Event()
    contexts, calls, callers = [], [], []

    @asynccontextmanager
    async def transport(parameters):
        contexts.append(("transport-enter", asyncio.current_task()))
        try:
            yield object(), object()
        finally:
            contexts.append(("transport-exit", asyncio.current_task()))

    class Session:
        def __init__(self, *args, **kwargs):
            pass

        async def __aenter__(self):
            contexts.append(("session-enter", asyncio.current_task()))
            return self

        async def __aexit__(self, *args):
            contexts.append(("session-exit", asyncio.current_task()))

        async def initialize(self):
            pass

        async def list_tools(self, cursor=None):
            return SimpleNamespace(tools=[SimpleNamespace(name=name)
                for name in ("withheld", "next", "error", "closing")], nextCursor=None)

        async def call_tool(self, name, arguments, read_timeout_seconds):
            assert read_timeout_seconds.total_seconds() == 120
            calls.append((name, dict(arguments)))
            if name == "withheld":
                active_started.set()
                try:
                    await never.wait()
                finally:
                    cleanup_started.set()
                    await release_cleanup.wait()
                    cleanup_finished.set()
            elif name == "closing":
                closing_started.set()
                try:
                    await never.wait()
                finally:
                    closing_cleaned.set()
            elif name == "next":
                assert cleanup_finished.is_set()
            return CallToolResult(content=[TextContent(type="text", text=name)],
                                  isError=name == "error")

    monkeypatch.setattr(mcp, "ClientSession", Session)
    monkeypatch.setattr(stdio, "stdio_client", transport)
    connection = module.MCPConnection({"name": "Inert session", "transport": "stdio",
        "command": "never-executed", "preserve_tool_errors": preserve_tool_errors})

    async def queued(name, arguments, entered):
        entered.set()
        return await connection.call(name, arguments)

    try:
        await connection.start()
        owner = connection.task
        first = asyncio.create_task(connection.call("withheld", {"case": "cancel"}))
        callers.append(first)
        await asyncio.wait_for(active_started.wait(), timeout=30)
        second_entered = asyncio.Event()
        second = asyncio.create_task(queued("next", {"case": "after-cancel"}, second_entered))
        callers.append(second)
        await asyncio.wait_for(second_entered.wait(), timeout=30)
        assert connection.queue.qsize() == 1 and not second.done()
        if interrupt == "owner":
            closing_owner = asyncio.create_task(connection.close())
            callers.append(closing_owner)
            await asyncio.wait_for(cleanup_started.wait(), timeout=30)
            assert not cleanup_finished.is_set() and not closing_owner.done()
            release_cleanup.set()
            await asyncio.wait_for(closing_owner, timeout=2)
            with pytest.raises(RuntimeError, match="stopped during a tool call"):
                await asyncio.wait_for(first, timeout=2)
            with pytest.raises(RuntimeError, match="connection is unavailable"):
                await asyncio.wait_for(second, timeout=2)
            assert cleanup_finished.is_set() and connection.task is None and owner.done()
            assert calls == [("withheld", {"case": "cancel"})]
            assert [name for name, _ in contexts] == ["transport-enter", "session-enter",
                                                    "session-exit", "transport-exit"]
            assert all(task is owner for _, task in contexts)
            return
        first.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(first, timeout=2)
        await asyncio.wait_for(cleanup_started.wait(), timeout=30)
        assert not cleanup_finished.is_set() and not second.done()
        assert calls == [("withheld", {"case": "cancel"})]
        assert connection.task is owner and not owner.done()
        release_cleanup.set()
        value = await asyncio.wait_for(second, timeout=2)
        assert value["content"][0]["text"] == "next" and value["isError"] is False
        assert cleanup_finished.is_set()
        if preserve_tool_errors:
            failure = await asyncio.wait_for(connection.call("error", {}), timeout=2)
            assert failure["isError"] is True and failure["content"][0]["text"] == "error"
        else:
            with pytest.raises(RuntimeError, match="MCP tool error reported an error"):
                await asyncio.wait_for(connection.call("error", {}), timeout=2)
        value = await asyncio.wait_for(connection.call("next", {"case": "after-error"}), timeout=2)
        assert value["isError"] is False and connection.task is owner and not owner.done()

        closing = asyncio.create_task(connection.call("closing", {}))
        callers.append(closing)
        await asyncio.wait_for(closing_started.wait(), timeout=30)
        unadmitted_entered = asyncio.Event()
        unadmitted = asyncio.create_task(queued("next", {"case": "closed"}, unadmitted_entered))
        callers.append(unadmitted)
        await asyncio.wait_for(unadmitted_entered.wait(), timeout=30)
        assert connection.queue.qsize() == 1
        await asyncio.wait_for(connection.close(), timeout=2)
        assert closing_cleaned.is_set() and connection.task is None and owner.done()
        with pytest.raises(RuntimeError, match="stopped during a tool call"):
            await asyncio.wait_for(closing, timeout=2)
        with pytest.raises(RuntimeError, match="connection is unavailable"):
            await asyncio.wait_for(unadmitted, timeout=2)
        assert calls == [("withheld", {"case": "cancel"}), ("next", {"case": "after-cancel"}),
            ("error", {}), ("next", {"case": "after-error"}), ("closing", {})]
        assert [name for name, _ in contexts] == ["transport-enter", "session-enter",
                                                "session-exit", "transport-exit"]
        assert all(task is owner for _, task in contexts)
    finally:
        release_cleanup.set()
        await connection.close()
        for caller in callers:
            if not caller.done():
                caller.cancel()
        await asyncio.gather(*callers, return_exceptions=True)
