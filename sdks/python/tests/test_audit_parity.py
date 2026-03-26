import pytest
from unittest.mock import AsyncMock, MagicMock
from meerkat.client import MeerkatClient
from meerkat.types import SessionInfo
from meerkat.mob import Mob

@pytest.mark.asyncio
async def test_list_sessions_with_labels():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    
    mock_response = {
        "sessions": [
            {
                "session_id": "sid1",
                "labels": {"env": "test"},
                "is_active": True,
                "model": "gpt-4",
                "provider": "openai"
            }
        ]
    }
    client._request = AsyncMock(return_value=mock_response)
    
    sessions = await client.list_sessions()
    assert len(sessions) == 1
    assert sessions[0].session_id == "sid1"
    assert sessions[0].labels == {"env": "test"}
    assert sessions[0].model == "gpt-4"

@pytest.mark.asyncio
async def test_spawn_many():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value=[{"ok": True, "meerkat_id": "m1"}])
    
    mob = Mob(client, "mob1")
    specs = [{"profile": "p1", "meerkat_id": "m1"}]
    results = await mob.spawn_many(specs)
    
    client._request.assert_called_with("mob/spawn_many", {
        "mob_id": "mob1",
        "specs": specs
    })
    assert results[0]["meerkat_id"] == "m1"

@pytest.mark.asyncio
async def test_create_session_with_labels():
    client = MeerkatClient()
    client._process = MagicMock()
    client._dispatcher = MagicMock()
    client._request = AsyncMock(return_value={"session_id": "s1", "text": "hello"})
    
    await client.create_session("hi", labels={"foo": "bar"})
    
    # Check that labels were passed to _request
    args, kwargs = client._request.call_args
    assert args[0] == "session/create"
    assert args[1]["labels"] == {"foo": "bar"}
