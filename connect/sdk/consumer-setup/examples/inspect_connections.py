"""Borrow an embedding host's SetupAdapter and consume its real MCP tool.

The host must put its trusted consumer-setup source directory on the Python
import path and compose the adapter with its authenticated, filtered reader.
This module does not create a connection inventory or a host identity.
"""
from mcp import Client

from mcp_adapter import create_server


async def inspect_connections(adapter):
    """Return current candidate observations; the host retains adapter custody.

Absence and read refusal raise instead of being converted to an empty inventory.
The caller must still use its existing admission, grant and binding owners
before any reuse. It must call adapter.aclose() when its setup session ends.
"""
    async with Client(create_server(adapter)) as client:
        available = {tool.name for tool in (await client.list_tools()).tools}
        if "setup_connections" not in available:
            raise RuntimeError("connection_reader_unavailable")
        result = await client.call_tool("setup_connections", {})
        if result.is_error:
            raise RuntimeError("connection_observation_unavailable")
        return result.structured_content
