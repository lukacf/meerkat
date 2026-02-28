"""Tests for skills v2.1: SkillKey, SkillRef, and Session.invoke_skill()."""

import warnings

import pytest

from meerkat import MeerkatClient, SkillKey
from meerkat.session import Session, _normalize_skill_ref
from meerkat.types import RunResult, Usage


# ---------------------------------------------------------------------------
# SkillKey and normalization
# ---------------------------------------------------------------------------

def test_skill_key_is_frozen():
    key = SkillKey(source_uuid="abc", skill_name="email-extractor")
    assert key.source_uuid == "abc"
    assert key.skill_name == "email-extractor"
    with pytest.raises(AttributeError):
        key.source_uuid = "xyz"  # type: ignore[misc]


def test_normalize_skill_ref_passes_skill_key_through():
    key = SkillKey(source_uuid="abc", skill_name="tool")
    result = _normalize_skill_ref(key)
    assert result is key


def test_normalize_skill_ref_parses_legacy_string():
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        result = _normalize_skill_ref("dc256086-0d2f-4f61-a307-320d4148107f/email-extractor")

    assert result == SkillKey(source_uuid="dc256086-0d2f-4f61-a307-320d4148107f", skill_name="email-extractor")
    assert len(caught) >= 1
    assert any(item.category is DeprecationWarning for item in caught)


def test_normalize_skill_ref_strips_leading_slash():
    with warnings.catch_warnings(record=True):
        warnings.simplefilter("always")
        result = _normalize_skill_ref("/dc256086/nested/skill")

    assert result == SkillKey(source_uuid="dc256086", skill_name="nested/skill")


def test_normalize_skill_ref_rejects_single_segment():
    with pytest.raises(ValueError, match="Invalid skill reference"):
        _normalize_skill_ref("just-a-name")


# ---------------------------------------------------------------------------
# Session.invoke_skill() integration (mock)
# ---------------------------------------------------------------------------

class _MockClient:
    """Minimal mock of MeerkatClient for testing Session."""

    def __init__(self):
        self._calls: list[dict] = []
        self._send_calls: list[dict] = []
        self._peers_calls: list[str] = []
        self._send_and_stream_calls: list[dict] = []

    def has_capability(self, cap: str) -> bool:
        return cap == "skills"

    def require_capability(self, cap: str) -> None:
        if not self.has_capability(cap):
            raise RuntimeError(f"Missing capability: {cap}")

    async def _start_turn(
        self,
        session_id,
        prompt,
        *,
        skill_refs=None,
        skill_references=None,
        flow_tool_overlay=None,
    ):
        self._calls.append({
            "session_id": session_id,
            "prompt": prompt,
            "skill_refs": skill_refs,
            "skill_references": skill_references,
            "flow_tool_overlay": flow_tool_overlay,
        })
        return RunResult(
            session_id=session_id,
            text="ok",
            usage=Usage(input_tokens=10, output_tokens=5),
        )

    async def _send(self, session_id, **kwargs):
        self._send_calls.append({"session_id": session_id, "kwargs": kwargs})
        return {"queued": True, "echo": kwargs}

    async def _peers(self, session_id):
        self._peers_calls.append(session_id)
        return {
            "peers": [
                {"id": "peer-a", "name": "alpha"},
                {"id": "peer-b", "name": "beta"},
            ]
        }

    async def send_and_stream(self, session_id, **kwargs):
        self._send_and_stream_calls.append({"session_id": session_id, "kwargs": kwargs})

        class _DummyStream:
            stream_id = "stream-1"

        return (
            {
                "kind": "input_accepted",
                "interaction_id": "i-1",
                "stream_reserved": True,
            },
            _DummyStream(),
        )


def _make_session() -> tuple[Session, _MockClient]:
    client = _MockClient()
    initial = RunResult(session_id="s-1", text="init", usage=Usage())
    return Session(client, initial), client


@pytest.mark.asyncio
async def test_invoke_skill_with_skill_key():
    session, client = _make_session()
    key = SkillKey(source_uuid="abc-123", skill_name="email-extractor")
    result = await session.invoke_skill(key, "run the extractor")

    assert result.text == "ok"
    assert len(client._calls) == 1
    call = client._calls[0]
    assert call["prompt"] == "run the extractor"
    assert call["skill_refs"] == [key]


@pytest.mark.asyncio
async def test_invoke_skill_with_legacy_string_emits_deprecation():
    session, client = _make_session()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        await session.invoke_skill(
            "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor",
            "run",
        )

    assert len(caught) >= 1
    assert any(item.category is DeprecationWarning for item in caught)
    assert len(client._calls) == 1


@pytest.mark.asyncio
async def test_session_send_routes_to_client_send():
    session, client = _make_session()
    result = await session.send(kind="peer_message", to="agent-b", body="hello")

    assert result["queued"] is True
    assert len(client._send_calls) == 1
    assert client._send_calls[0]["session_id"] == "s-1"
    assert client._send_calls[0]["kwargs"] == {
        "kind": "peer_message",
        "to": "agent-b",
        "body": "hello",
    }


@pytest.mark.asyncio
async def test_session_peers_returns_peer_list():
    session, client = _make_session()
    peers = await session.peers()

    assert len(client._peers_calls) == 1
    assert client._peers_calls[0] == "s-1"
    assert peers == [
        {"id": "peer-a", "name": "alpha"},
        {"id": "peer-b", "name": "beta"},
    ]


@pytest.mark.asyncio
async def test_session_send_and_stream_routes_to_client_method():
    session, client = _make_session()
    receipt, stream = await session.send_and_stream(
        kind="input",
        body="hello",
        source="rpc",
        stream="reserve_interaction",
    )

    assert receipt["interaction_id"] == "i-1"
    assert stream.stream_id == "stream-1"
    assert len(client._send_and_stream_calls) == 1
    assert client._send_and_stream_calls[0]["session_id"] == "s-1"


@pytest.mark.asyncio
async def test_client_list_mob_prefabs_calls_rpc_method():
    client = object.__new__(MeerkatClient)
    calls: list[tuple[str, dict]] = []

    async def fake_request(method, params):
        calls.append((method, params))
        return {
            "prefabs": [
                {"key": "coding_swarm", "toml_template": 'id = "coding_swarm"'},
                {"key": "pipeline", "toml_template": 'id = "pipeline"'},
            ]
        }

    client._request = fake_request  # type: ignore[attr-defined]
    prefabs = await MeerkatClient.list_mob_prefabs(client)
    assert [entry["key"] for entry in prefabs] == ["coding_swarm", "pipeline"]
    assert calls == [("mob/prefabs", {})]


@pytest.mark.asyncio
async def test_client_list_mob_tools_and_call_tool():
    client = object.__new__(MeerkatClient)
    calls: list[tuple[str, dict]] = []

    async def fake_request(method, params):
        calls.append((method, params))
        if method == "mob/tools":
            return {"tools": [{"name": "mob_create"}]}
        if method == "mob/call":
            return {"ok": True, "mob_id": "m1"}
        return {}

    client._request = fake_request  # type: ignore[attr-defined]
    tools = await MeerkatClient.list_mob_tools(client)
    result = await MeerkatClient.call_mob_tool(
        client,
        "mob_create",
        {"prefab": "coding_swarm"},
    )
    assert tools == [{"name": "mob_create"}]
    assert result["mob_id"] == "m1"
    assert calls == [
        ("mob/tools", {}),
        ("mob/call", {"name": "mob_create", "arguments": {"prefab": "coding_swarm"}}),
    ]


@pytest.mark.asyncio
async def test_set_config_returns_envelope():
    client = object.__new__(MeerkatClient)
    calls: list[tuple[str, dict]] = []

    async def fake_request(method, params):
        calls.append((method, params))
        return {"config": {"agent": {"model": "x"}}, "generation": 7}

    client._request = fake_request  # type: ignore[attr-defined]
    envelope = await MeerkatClient.set_config(
        client,
        {"agent": {"model": "x"}},
        expected_generation=6,
    )
    assert envelope["generation"] == 7
    assert calls == [
        ("config/set", {"config": {"agent": {"model": "x"}}, "expected_generation": 6})
    ]


@pytest.mark.asyncio
async def test_client_list_mob_prefabs_propagates_errors():
    client = object.__new__(MeerkatClient)

    async def fake_request(_method, _params):
        raise RuntimeError("transport down")

    client._request = fake_request  # type: ignore[attr-defined]
    with pytest.raises(RuntimeError, match="transport down"):
        await MeerkatClient.list_mob_prefabs(client)
