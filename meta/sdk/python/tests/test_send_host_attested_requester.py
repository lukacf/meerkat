"""``send(..., requester=...)``: a person the host attests as the original
requester of the content it relays. A gateway that predates the field would
ignore it and send without a requester, so the SDK refuses one, typed, unless
the gateway advertises the ``send_host_attested_requester`` feature
capability. A requester does not combine with ``idempotency_key``.
"""
import asyncio

import pytest

from meerkat_meta import CapabilityUnavailableError
from meerkat_meta.runtime import MetaRuntime

from .test_identity_first_turn_tickets import _make_runtime

PERSON = {"kind": "person", "id": "person-7"}


class RequesterTransport:
    """Answers like a gateway that serves the requester when ``serves`` is set,
    and like one that predates it otherwise."""

    def __init__(self, *, serves: bool):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self.serves = serves

    def sent(self, method: str) -> list[dict]:
        return [call["params"] for call in self.calls if call.get("method") == method]

    async def send_async(self, request, *, timeout=None):
        await asyncio.sleep(0)
        self.calls.append(request)
        method = request.get("method")
        if method == "meta/capabilities":
            features = ["send_host_attested_requester"] if self.serves else []
            result = {
                "contract_version": "0.5.0",
                "methods": ["meta/send"],
                "feature_capabilities": features,
                "loaded_modules": [],
            }
        elif method == "meta/send":
            result = {"fencing_token": 1}
        else:
            result = {}
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": result}

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass


@pytest.mark.asyncio
async def test_a_requester_is_forwarded_to_a_gateway_that_serves_it():
    transport = RequesterTransport(serves=True)
    runtime: MetaRuntime = _make_runtime(transport)
    await runtime.send("keeper", "hello", requester=PERSON, track_turn=True)
    assert transport.sent("meta/send") == [
        {"identity": "keeper", "content": "hello", "track_turn": True, "requester": PERSON}
    ]


@pytest.mark.asyncio
async def test_without_a_requester_the_send_is_unchanged():
    transport = RequesterTransport(serves=True)
    runtime: MetaRuntime = _make_runtime(transport)
    await runtime.send("keeper", "hello")
    assert transport.sent("meta/send") == [{"identity": "keeper", "content": "hello"}]
    assert transport.sent("meta/capabilities") == []


@pytest.mark.asyncio
async def test_a_gateway_that_does_not_serve_the_requester_refuses_it_typed():
    transport = RequesterTransport(serves=False)
    runtime: MetaRuntime = _make_runtime(transport)
    with pytest.raises(CapabilityUnavailableError):
        await runtime.send("keeper", "hello", requester=PERSON)
    assert transport.sent("meta/send") == []


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "requester",
    [
        {"kind": "agent", "id": "person-7"},
        {"kind": "person"},
        {"kind": "person", "id": ""},
        {"kind": "person", "id": "person-7", "display_name": "P"},
        "person-7",
    ],
)
async def test_a_malformed_requester_is_refused_before_any_call(requester):
    transport = RequesterTransport(serves=True)
    runtime: MetaRuntime = _make_runtime(transport)
    with pytest.raises(ValueError):
        await runtime.send("keeper", "hello", requester=requester)
    assert transport.calls == []


@pytest.mark.asyncio
async def test_a_requester_does_not_combine_with_a_key():
    transport = RequesterTransport(serves=True)
    runtime: MetaRuntime = _make_runtime(transport)
    with pytest.raises(ValueError, match="does not combine with requester"):
        await runtime.send("keeper", "hello", requester=PERSON, idempotency_key="k1")
    assert transport.calls == []
