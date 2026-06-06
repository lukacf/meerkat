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
from typing import Any, Callable, Awaitable

from .types import ContentBlock

# A callback tool may return either a plain string (delivered as a single text
# block) or a typed list of :class:`ContentBlock` (delivered faithfully as the
# wire `WireToolResultContent` block array). We do NOT collapse block lists to a
# stringified blob — multimodal results must round-trip as structured content.
ToolResult = str | list[ContentBlock]


@dataclass
class ToolRegistration:
    """A registered callback tool."""

    name: str
    description: str
    input_schema: dict[str, Any]
    handler: Callable[[dict[str, Any]], Awaitable[ToolResult]]


class ToolRegistry:
    """In-process registry of callback tool definitions and handlers."""

    def __init__(self) -> None:
        self._tools: dict[str, ToolRegistration] = {}

    def register(
        self,
        name: str,
        handler: Callable[[dict[str, Any]], Awaitable[ToolResult]],
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

    async def handle(
        self, name: str, arguments: dict[str, Any]
    ) -> tuple[ToolResult, bool]:
        """Execute a tool handler. Returns (content, is_error).

        The content is the wire `WireToolResultContent` shape: either a plain
        string or a typed list of :class:`ContentBlock`. A block list is passed
        through faithfully (serialized as structured wire content) rather than
        coerced to a stringified blob; only genuine non-list scalars are
        stringified.
        """
        tool = self._tools.get(name)
        if tool is None:
            return f"Unknown tool: {name}", True
        try:
            result = await tool.handler(arguments)
            if isinstance(result, list):
                # Typed multimodal block list — deliver as structured content.
                return result, False
            if isinstance(result, str):
                return result, False
            # Genuine scalar (e.g. number/bool) — stringify for the text path.
            return str(result), False
        except Exception as exc:
            return f"Tool error: {exc}", True

    def __len__(self) -> int:
        return len(self._tools)

    def __bool__(self) -> bool:
        return bool(self._tools)
