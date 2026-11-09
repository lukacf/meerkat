"""Rich content blocks for callback tool-handler results.

A callback tool handler (registered via :meth:`SessionBuildOptions.register_tool`
or :meth:`AgentBuildDraft.register_tool`) normally returns a JSON-serializable
value, which the model sees as text. To hand the model an **image** or multiple
content blocks, wrap them with :func:`tool_content` and return that — the gateway
then delivers them as real content blocks instead of text::

    def screenshot_tool(args):
        png_b64 = capture()  # base64-encoded PNG bytes
        return tool_content(
            text_block("Here is the screenshot:"),
            image_block("image/png", png_b64),
        )

Rich content is **opt-in**: only a :class:`ToolResultContent` (from
:func:`tool_content`) is delivered as content blocks. A plain return value — a
string, dict, or even a bare ``list`` — keeps the default text behavior, so a
tool that returns ordinary list/dict data is never reinterpreted. The block
shapes mirror the runtime's ``ContentBlock`` wire format (internally tagged by
``type``); blocks are parsed strictly, so extra keys on a block are dropped.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any

__all__ = [
    "text_block",
    "image_block",
    "image_blob_block",
    "tool_content",
    "ToolResultContent",
]


def text_block(text: str) -> dict[str, Any]:
    """A text content block."""
    return {"type": "text", "text": text}


def image_block(media_type: str, data: str) -> dict[str, Any]:
    """An inline image content block.

    Args:
        media_type: MIME type, e.g. ``"image/png"`` or ``"image/jpeg"``.
        data: Base64-encoded image bytes.
    """
    return {"type": "image", "media_type": media_type, "source": "inline", "data": data}


def image_blob_block(media_type: str, blob_id: str) -> dict[str, Any]:
    """An image content block referencing a durable blob by id.

    Use when the image already lives in the runtime blob store; prefer
    :func:`image_block` when the handler has the bytes in hand.
    """
    return {"type": "image", "media_type": media_type, "source": "blob", "blob_id": blob_id}


@dataclass(frozen=True)
class ToolResultContent:
    """Explicit rich tool result: a list of content blocks for the model.

    Return an instance (most easily via :func:`tool_content`) from a callback
    tool handler to deliver images / multiple blocks. Returning anything else
    keeps the default single-text-block behavior.
    """

    blocks: list[dict[str, Any]]


def tool_content(*blocks: dict[str, Any]) -> ToolResultContent:
    """Bundle content blocks into a rich tool result.

    Build the blocks with :func:`text_block`, :func:`image_block`, or
    :func:`image_blob_block`.
    """
    return ToolResultContent(list(blocks))
