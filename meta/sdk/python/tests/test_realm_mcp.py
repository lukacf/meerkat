"""Meta SDK wrappers of the stdio gateway's realm MCP administration
(``meta/realm_mcp/{servers, install, set_secret, remove_secret}``).

Each wrapper sends its method with exactly the params it names; every typed
outcome is its own error (``realm_mcp_refused`` with a closed vocabulary of
kinds, ``realm_mcp_unavailable``, ``realm_mcp_outcome_unknown``); and a
secret value is sent once, in the ``set_secret`` request, and kept out of
every exception and log record.
"""

import asyncio
import logging

import pytest

from meerkat_meta.runtime import MetaRuntime, _rpc_error_from_payload

SERVER = {
    "name": "search",
    "command": "sh",
    "args": ["-c", "sleep 30"],
    "secret_env": ["SEARCH_API_KEY"],
}
SECRET = "realm-mcp-secret-7f3a9c41"


class FakeTransport:
    """Records RPC calls and answers each with the next scripted reply."""

    def __init__(self, replies=None):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self._replies = list(replies or [])

    def send_sync(self, request):
        self.calls.append(request)
        reply = self._replies.pop(0) if self._replies else {"result": {}}
        return {"jsonrpc": "2.0", "id": request.get("id"), **reply}

    async def send_async(self, request, *, timeout=None):
        return self.send_sync(request)

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass


def make_runtime(replies=None):
    from meerkat_meta.agent_builder import CallbackDispatcher

    transport = FakeTransport(replies=replies)
    runtime = MetaRuntime.__new__(MetaRuntime)
    runtime._config = None
    runtime._transport = transport
    runtime._running = True
    runtime._rust_http_base = None
    runtime._lifecycle_lock = asyncio.Lock()
    runtime._shutdown_task = None
    runtime._dispatcher = CallbackDispatcher()
    return runtime, transport


@pytest.mark.asyncio
async def test_each_wrapper_sends_its_method_with_exactly_its_params():
    runtime, transport = make_runtime()
    await runtime.realm_mcp_servers()
    await runtime.realm_mcp_install(SERVER, expected_generation=3)
    await runtime.realm_mcp_set_secret(SERVER, "SEARCH_API_KEY", SECRET)
    await runtime.realm_mcp_remove_secret(SERVER, "SEARCH_API_KEY")
    await runtime.realm_mcp_remove_secret(SERVER, "SEARCH_API_KEY", removal="obsolete")
    assert [(call["method"], call["params"]) for call in transport.calls] == [
        ("meta/realm_mcp/servers", {}),
        ("meta/realm_mcp/install", {"server": SERVER, "expected_generation": 3}),
        (
            "meta/realm_mcp/set_secret",
            {"server": SERVER, "env": "SEARCH_API_KEY", "value": SECRET},
        ),
        (
            "meta/realm_mcp/remove_secret",
            {"server": SERVER, "env": "SEARCH_API_KEY", "removal": "current"},
        ),
        (
            "meta/realm_mcp/remove_secret",
            {"server": SERVER, "env": "SEARCH_API_KEY", "removal": "obsolete"},
        ),
    ]


@pytest.mark.asyncio
async def test_a_removal_is_current_or_obsolete():
    runtime, transport = make_runtime()
    with pytest.raises(ValueError):
        await runtime.realm_mcp_remove_secret(SERVER, "SEARCH_API_KEY", removal="all")
    assert transport.calls == []


def test_each_outcome_is_its_own_typed_error():
    from meerkat_meta import (
        RealmMcpOutcomeUnknownError,
        RealmMcpRefusedError,
        RealmMcpUnavailableError,
    )

    refused = _rpc_error_from_payload(
        {
            "code": -32602,
            "message": "the realm's own document moved",
            "data": {
                "kind": "realm_mcp_refused",
                "realm_mcp_refusal": {
                    "kind": "generation_conflict",
                    "expected": 3,
                    "current": 4,
                },
            },
        },
        request_id="1",
        method="meta/realm_mcp/install",
    )
    assert isinstance(refused, RealmMcpRefusedError)
    assert refused.refusal_kind == "generation_conflict"
    assert refused.refusal == {"kind": "generation_conflict", "expected": 3, "current": 4}

    unavailable = _rpc_error_from_payload(
        {
            "code": -32004,
            "message": "the credential owner cannot be held now",
            "data": {"kind": "realm_mcp_unavailable"},
        },
        request_id="2",
        method="meta/realm_mcp/set_secret",
    )
    assert isinstance(unavailable, RealmMcpUnavailableError)

    unknown = _rpc_error_from_payload(
        {
            "code": -32603,
            "message": "the write's outcome is unknown",
            "data": {"kind": "realm_mcp_outcome_unknown"},
        },
        request_id="3",
        method="meta/realm_mcp/install",
    )
    assert isinstance(unknown, RealmMcpOutcomeUnknownError)

    plain = _rpc_error_from_payload(
        {"code": -32602, "message": "Invalid params"},
        request_id="4",
        method="meta/realm_mcp/install",
    )
    assert not isinstance(plain, RealmMcpRefusedError)


def test_the_refusal_kinds_are_a_closed_vocabulary():
    from meerkat_meta import REALM_MCP_REFUSAL_KINDS

    assert REALM_MCP_REFUSAL_KINDS == frozenset(
        {
            "no_document",
            "server_exists",
            "server_not_found",
            "generation_conflict",
            "invalid_server",
            "invalid_secret_update",
            "definition_changed",
            "still_current",
        }
    )


@pytest.mark.asyncio
async def test_the_secret_value_stays_out_of_exceptions_and_logs(caplog):
    from meerkat_meta import RealmMcpRefusedError

    runtime, _transport = make_runtime(
        replies=[
            {
                "error": {
                    "code": -32602,
                    "message": "MCP server 'search' is no longer installed as reviewed",
                    "data": {
                        "kind": "realm_mcp_refused",
                        "realm_mcp_refusal": {"kind": "definition_changed"},
                    },
                }
            }
        ]
    )
    caplog.set_level(logging.DEBUG)
    with pytest.raises(RealmMcpRefusedError) as raised:
        await runtime.realm_mcp_set_secret(SERVER, "SEARCH_API_KEY", SECRET)
    error = raised.value
    assert SECRET not in str(error)
    assert SECRET not in repr(error)
    assert SECRET not in repr(error.data)
    assert SECRET not in repr(error.__dict__)
    assert all(SECRET not in record.getMessage() for record in caplog.records)
