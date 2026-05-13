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

from dataclasses import dataclass
from typing import Any, Awaitable, Callable, TypedDict, TypeAlias

from .types import ContentInput


class ToolCallbackResult(TypedDict, total=False):
    """Structured result returned from a callback tool."""

    content: ContentInput
    is_error: bool
    isError: bool


ToolCallbackReturn: TypeAlias = ContentInput | tuple[ContentInput, bool] | ToolCallbackResult


@dataclass
class ToolRegistration:
    """A registered callback tool."""

    name: str
    description: str
    input_schema: dict[str, Any]
    handler: Callable[[dict[str, Any]], Awaitable[ToolCallbackReturn]]


class ToolRegistry:
    """In-process registry of callback tool definitions and handlers."""

    def __init__(self) -> None:
        self._tools: dict[str, ToolRegistration] = {}

    def register(
        self,
        name: str,
        handler: Callable[[dict[str, Any]], Awaitable[ToolCallbackReturn]],
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

    async def handle(self, name: str, arguments: dict[str, Any]) -> tuple[ContentInput, bool]:
        """Execute a tool handler. Returns (content, is_error)."""
        tool = self._tools.get(name)
        if tool is None:
            return f"Unknown tool: {name}", True
        try:
            result = await tool.handler(arguments)
            if isinstance(result, tuple):
                content, is_error = result
                return content, bool(is_error)
            if isinstance(result, dict) and "content" in result:
                return result["content"], bool(
                    result.get("is_error", result.get("isError", False))
                )
            return result, False
        except Exception as exc:
            return f"Tool error: {exc}", True

    def __len__(self) -> int:
        return len(self._tools)

    def __bool__(self) -> bool:
        return bool(self._tools)
