"""A host turn authored from the identity's registered row.

The host reads the full registered row (``meta/registered_spec``), checks
its own selection against it, and sends with the row's ``spec_digest`` as
``expected_spec_digest``. The gateway re-checks the digest against the
registered row under the identity's lifecycle lock before delivery; a
changed row is refused with -32006 and nothing submitted. These tests pin
the SDK side of that contract: the read, the forwarded digest and the
typed refusal.
"""
import asyncio

import pytest

from meerkat_meta import REGISTERED_SPEC_CHANGED_CODE, RegisteredSpecChangedError
from meerkat_meta.identity_first_models import RegisteredSpec
from meerkat_meta.runtime import MetaRuntime

from .test_identity_first_turn_tickets import _make_runtime


class RowTransport:
    """Answers like the gateway for one identity whose registered row the
    test can replace: a send naming another row's digest is refused before
    delivery and recorded as not delivered."""

    def __init__(self):
        self.calls: list[dict] = []
        self.request_timeout = 60.0
        self.delivered: list[str] = []
        self.row = {"identity": "keeper", "profile": "worker", "additional_instructions": []}
        self.digest = "sha256:aaaa"

    def replace_row(self, instructions: list[str], digest: str) -> None:
        self.row = {**self.row, "additional_instructions": instructions}
        self.digest = digest

    async def send_async(self, request, *, timeout=None):
        await asyncio.sleep(0)
        self.calls.append(request)
        method = request.get("method")
        params = request.get("params") or {}
        if method == "meta/registered_spec":
            result = {"identity": params["identity"], "spec": self.row, "spec_digest": self.digest}
        elif method == "meta/send":
            expected = params.get("expected_spec_digest")
            if expected is not None and expected != self.digest:
                return {
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "error": {
                        "code": -32006,
                        "message": "the registered row of keeper is no longer the row the "
                        "input was authored from; NOTHING was submitted",
                        "data": {
                            "kind": "registered_spec_changed",
                            "identity": "keeper",
                            "expected_spec_digest": expected,
                            "submitted": False,
                        },
                    },
                }
            self.delivered.append(params["content"])
            result = {"fencing_token": 1, "turn": {"ticket": "t-1"}}
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
async def test_registered_spec_reads_the_full_row_and_its_digest():
    transport = RowTransport()
    runtime: MetaRuntime = _make_runtime(transport)

    row = await runtime.registered_spec("keeper")

    assert isinstance(row, RegisteredSpec)
    assert row.identity == "keeper"
    assert row.spec["profile"] == "worker"
    assert row.additional_instructions == []
    assert row.spec_digest == "sha256:aaaa"
    assert transport.calls[-1]["method"] == "meta/registered_spec"
    assert transport.calls[-1]["params"] == {"identity": "keeper"}


@pytest.mark.asyncio
async def test_a_send_from_a_superseded_row_is_refused_typed_and_the_current_row_is_sent():
    transport = RowTransport()
    runtime: MetaRuntime = _make_runtime(transport)
    stale = await runtime.registered_spec("keeper")

    # A same-profile instructions change commits on the gateway.
    transport.replace_row(["Answer in one sentence."], "sha256:bbbb")

    with pytest.raises(RegisteredSpecChangedError) as refused:
        await runtime.send(
            "keeper", "stale", track_turn=True, expected_spec_digest=stale.spec_digest
        )
    assert refused.value.code == REGISTERED_SPEC_CHANGED_CODE == -32006
    assert refused.value.method == "meta/send"
    assert refused.value.data["kind"] == "registered_spec_changed"
    assert refused.value.data["submitted"] is False
    assert transport.delivered == []

    current = await runtime.agent("keeper").registered_spec()
    assert current.additional_instructions == ["Answer in one sentence."]
    sent = await runtime.agent("keeper").send(
        "current", track_turn=True, expected_spec_digest=current.spec_digest
    )
    assert sent.turn_ticket == "t-1"
    assert transport.delivered == ["current"]
    send_params = [c["params"] for c in transport.calls if c["method"] == "meta/send"]
    assert [p.get("expected_spec_digest") for p in send_params] == [
        "sha256:aaaa",
        "sha256:bbbb",
    ]


@pytest.mark.asyncio
async def test_a_send_without_an_expected_row_does_not_name_one():
    transport = RowTransport()
    runtime: MetaRuntime = _make_runtime(transport)

    await runtime.send("keeper", "plain")

    assert "expected_spec_digest" not in transport.calls[-1]["params"]
