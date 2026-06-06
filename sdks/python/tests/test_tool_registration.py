"""Tests for post-connect tool registration fault handling.

`MeerkatClient._register_tool_with_server` runs on a detached
`asyncio.ensure_future` task launched from the synchronous `@client.tool`
decorator. A genuine server rejection of `tools/register` is a real fault and
must NOT be laundered into a silent success: it is logged and re-raised. Only
the truly-benign shutdown/disconnect codes return quietly.
"""

import pytest

from meerkat.client import MeerkatClient
from meerkat.errors import MeerkatError


@pytest.mark.asyncio
async def test_register_tool_propagates_server_rejection() -> None:
    """A server rejection MeerkatError is re-raised, not swallowed."""
    client = MeerkatClient()

    async def reject(_method: str, _params: object) -> object:
        raise MeerkatError("TOOL_REJECTED", "server rejected tools/register")

    client._request = reject  # type: ignore[assignment]

    with pytest.raises(MeerkatError) as excinfo:
        await client._register_tool_with_server("t", "", None)

    assert excinfo.value.code == "TOOL_REJECTED"


@pytest.mark.asyncio
async def test_register_tool_records_server_rejection_as_state() -> None:
    """A server rejection is recorded as caller-inspectable state, keyed by
    tool name, in addition to being re-raised."""
    client = MeerkatClient()

    async def reject(_method: str, _params: object) -> object:
        raise MeerkatError("TOOL_REJECTED", "server rejected tools/register")

    client._request = reject  # type: ignore[assignment]

    with pytest.raises(MeerkatError):
        await client._register_tool_with_server("t", "", None)

    assert client._tool_registration_errors["t"].code == "TOOL_REJECTED"


@pytest.mark.asyncio
async def test_register_tool_swallows_benign_disconnect() -> None:
    """Clean shutdown / disconnect codes return None without raising."""
    client = MeerkatClient()

    for benign_code in ("CLIENT_CLOSED", "CONNECTION_CLOSED", "NOT_CONNECTED"):
        async def disconnect(_method: str, _params: object, _code: str = benign_code) -> object:
            raise MeerkatError(_code, "transport closed")

        client._request = disconnect  # type: ignore[assignment]

        result = await client._register_tool_with_server("t", "", None)
        assert result is None
