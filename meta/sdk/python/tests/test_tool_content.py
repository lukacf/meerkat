"""Tests for rich tool-result content blocks (callback tools returning images)."""
import pytest
from meerkat_meta import (
    ToolResultContent,
    image_block,
    image_blob_block,
    text_block,
    tool_content,
)
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.models import SessionBuildOptions


class TestContentBlockHelpers:
    def test_text_block(self):
        assert text_block("hi") == {"type": "text", "text": "hi"}

    def test_image_block_inline(self):
        assert image_block("image/png", "aGVsbG8=") == {
            "type": "image",
            "media_type": "image/png",
            "source": "inline",
            "data": "aGVsbG8=",
        }

    def test_image_blob_block(self):
        assert image_blob_block("image/jpeg", "blob-123") == {
            "type": "image",
            "media_type": "image/jpeg",
            "source": "blob",
            "blob_id": "blob-123",
        }


class TestToolContentWrapper:
    def test_tool_content_builds_marker(self):
        tc = tool_content(text_block("a"), image_block("image/png", "aGVsbG8="))
        assert isinstance(tc, ToolResultContent)
        assert tc.blocks == [
            {"type": "text", "text": "a"},
            {"type": "image", "media_type": "image/png", "source": "inline", "data": "aGVsbG8="},
        ]


class TestHandlerReturnsContentBlocks:
    """An explicit tool_content(...) return is delivered as content_blocks; a
    plain return keeps the legacy single-text/content path (opt-in)."""

    @pytest.mark.asyncio
    async def test_tool_content_becomes_content_blocks(self):
        blocks = [text_block("see this:"), image_block("image/png", "aGVsbG8=")]

        class _Builder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                opts.register_tool("shot", lambda args: tool_content(*blocks))

        d = CallbackDispatcher()
        d.register_builder(_Builder())
        await d.handle_callback("callback/build_agent", {"options": {"scope_id": "s1"}})
        result = await d.handle_callback(
            "callback/call_tool",
            {"scope_id": "s1", "tool": "shot", "arguments": {}},
        )
        assert result == {"content_blocks": blocks}
        # The image block survives verbatim.
        assert result["content_blocks"][1] == {
            "type": "image",
            "media_type": "image/png",
            "source": "inline",
            "data": "aGVsbG8=",
        }

    @pytest.mark.asyncio
    async def test_plain_list_return_stays_content_not_content_blocks(self):
        """A bare list return (no tool_content) must NOT be treated as blocks."""
        data = [{"type": "text", "text": "this is data"}]

        class _Builder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                opts.register_tool("rows", lambda args: data)

        d = CallbackDispatcher()
        d.register_builder(_Builder())
        await d.handle_callback("callback/build_agent", {"options": {"scope_id": "s1"}})
        result = await d.handle_callback(
            "callback/call_tool",
            {"scope_id": "s1", "tool": "rows", "arguments": {}},
        )
        assert result == {"content": data}
        assert "content_blocks" not in result
