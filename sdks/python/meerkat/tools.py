"""Callback tool registry for providing tool implementations to the agent.

Example::

    from meerkat import MeerkatClient

    client = MeerkatClient()

    @client.tool("search", description="Search the web", input_schema={"type": "object"})
    async def handle_search(arguments: dict) -> str:
        return f"Results for {arguments.get('q', '')}"

    async with client:
        session = await client.create_session("Find info about Meerkat")
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Awaitable


@dataclass
class ToolRegistration:
    """A registered callback tool."""

    name: str
    description: str
    input_schema: dict[str, Any]
    handler: Callable[[dict[str, Any]], Awaitable[str]]


class ToolRegistry:
    """In-process registry of callback tool definitions and handlers."""

    def __init__(self) -> None:
        self._tools: dict[str, ToolRegistration] = {}

    def register(
        self,
        name: str,
        handler: Callable[[dict[str, Any]], Awaitable[str]],
        *,
        description: str = "",
        input_schema: dict[str, Any] | None = None,
    ) -> None:
        """Register a tool handler."""
        self._tools[name] = ToolRegistration(
            name=name,
            description=description or f"Tool: {name}",
            input_schema=input_schema or {"type": "object"},
            handler=handler,
        )

    def definitions(self) -> list[dict[str, Any]]:
        """Return wire-format tool definitions for RPC registration."""
        return [
            {
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            }
            for t in self._tools.values()
        ]

    async def handle(self, name: str, arguments: dict[str, Any]) -> tuple[str, bool]:
        """Execute a tool handler. Returns (content, is_error)."""
        tool = self._tools.get(name)
        if tool is None:
            return f"Unknown tool: {name}", True
        try:
            result = await tool.handler(arguments)
            return str(result), False
        except Exception as exc:
            return f"Tool error: {exc}", True

    def __len__(self) -> int:
        return len(self._tools)

    def __bool__(self) -> bool:
        return bool(self._tools)
