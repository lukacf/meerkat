"""A helper run may outlast the transport's default request timeout.

``meta/spawn_helper`` and ``meta/fork_helper`` answer only once the helper's
turn has finished, and the gateway sets no deadline of its own on that run, so
the SDK's wait for the reply is the only bound. These tests drive the real SDK
runtime and its persistent stdio transport against a stand-in gateway whose
helper replies come later than the transport's default request timeout
(lowered to half a second through the host transport hook, so the tests stay
short). The stand-in logs every request, so each test can prove what was sent.
"""
from __future__ import annotations

import asyncio
import json
import logging
import math
import sys
import time
from pathlib import Path

import pytest

from meerkat_meta._transport import PersistentTransport
from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime

_DEFAULT_WAIT = 0.5
_HELPER_RUN = 1.5
_HELPER = dict(result_label="review", max_text_bytes=4096)

# Each request is answered on its own thread after its step's ``sleep``; a
# method walks through its steps, holding the last. EOF on stdin exits at once.
_STAND_IN = r'''
import json, os, sys, threading, time

SCENARIO = json.loads(__SCENARIO__)
LOG = __LOG__
positions = {}
lock = threading.Lock()

def respond(request):
    method = request.get("method")
    with lock:
        with open(LOG, "a") as log:
            log.write(json.dumps({"id": request["id"], "method": method,
                                  "params": request.get("params")}) + "\n")
        steps = SCENARIO.get(method)
        if steps is None:
            step = {"result": {"http_base_url": "http://127.0.0.1:1"}
                    if method == "meta/init" else {}}
        else:
            index = min(positions.get(method, 0), len(steps) - 1)
            positions[method] = index + 1
            step = steps[index]
    time.sleep(step.get("sleep", 0))
    reply = {"jsonrpc": "2.0", "id": request["id"], "result": step["result"]}
    with lock:
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()

for raw in sys.stdin:
    if raw.strip():
        threading.Thread(target=respond, args=(json.loads(raw),), daemon=True).start()
os._exit(0)
'''


def _helper_reply(output: str) -> dict:
    return {"output": output, "tokens_used": 3, "session_id": f"session-{output}"}


class _StandIn:
    def __init__(self, tmp_path: Path, scenario: dict):
        self.log = tmp_path / "requests.jsonl"
        self.path = tmp_path / "stand_in_gateway.py"
        self.path.write_text(
            f"#!{sys.executable}\n"
            + _STAND_IN.replace("__SCENARIO__", repr(json.dumps(scenario)))
            .replace("__LOG__", repr(str(self.log)))
        )
        self.path.chmod(0o755)

    def calls(self, method: str) -> list[dict]:
        if not self.log.exists():
            return []
        entries = [json.loads(line) for line in self.log.read_text().splitlines()]
        return [entry for entry in entries if entry["method"] == method]


class _ShortDefaultWaitRuntime(MetaRuntime):
    """A host runtime whose transport waits half a second by default."""

    def _make_gateway_transport(self) -> PersistentTransport:
        return PersistentTransport(self._config.gateway_bin, timeout=_DEFAULT_WAIT)


async def _runtime(stand_in: _StandIn) -> MetaRuntime:
    runtime = _ShortDefaultWaitRuntime(Meta.builder().gateway(str(stand_in.path))._config)
    await runtime.connect()
    return runtime


@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_the_default_wait_gives_up_on_a_helper_that_outlives_it(tmp_path):
    """The control: with no ``transport_timeout`` the call raises at the
    transport's default request timeout while the helper still runs, after
    sending its request once."""
    stand_in = _StandIn(tmp_path, {
        "meta/spawn_helper": [{"sleep": _HELPER_RUN, "result": _helper_reply("late")}],
    })
    runtime = await _runtime(stand_in)
    try:
        started = time.monotonic()
        with pytest.raises(RuntimeError, match="persistent transport: timeout after 0.5s"):
            await runtime.mob_handle().spawn_helper("reviewer", "review the draft", **_HELPER)
        waited = time.monotonic() - started
    finally:
        await runtime.shutdown()

    assert waited < _HELPER_RUN
    assert len(stand_in.calls("meta/spawn_helper")) == 1


@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_spawn_helper_transport_timeout_waits_for_the_whole_helper_run(tmp_path):
    """A ``transport_timeout`` past the helper's run returns its result, and
    the gateway receives exactly the params it receives without one."""
    stand_in = _StandIn(tmp_path, {
        "meta/spawn_helper": [{"sleep": _HELPER_RUN, "result": _helper_reply("reviewed")}],
    })
    runtime = await _runtime(stand_in)
    try:
        result = await runtime.mob_handle().spawn_helper(
            "reviewer", "review the draft", **_HELPER, transport_timeout=30.0
        )
    finally:
        await runtime.shutdown()

    assert result.output == "reviewed"
    assert result.session_id == "session-reviewed"
    assert [call["params"] for call in stand_in.calls("meta/spawn_helper")] == [{
        "agent_identity": "reviewer",
        "task": "review the draft",
        "result_label": "review",
        "max_text_bytes": 4096,
    }]


@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_fork_helper_transport_timeout_waits_for_the_whole_helper_run(tmp_path):
    stand_in = _StandIn(tmp_path, {
        "meta/fork_helper": [{"sleep": _HELPER_RUN, "result": _helper_reply("forked")}],
    })
    runtime = await _runtime(stand_in)
    try:
        result = await runtime.mob_handle().fork_helper(
            "lead",
            "reviewer",
            "review the draft",
            **_HELPER,
            fork_context={"type": "last_messages", "count": 4},
            transport_timeout=30.0,
        )
    finally:
        await runtime.shutdown()

    assert result.output == "forked"
    assert [call["params"] for call in stand_in.calls("meta/fork_helper")] == [{
        "source_member_id": "lead",
        "agent_identity": "reviewer",
        "task": "review the draft",
        "result_label": "review",
        "max_text_bytes": 4096,
        "fork_context": {"type": "last_messages", "count": 4},
    }]


@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_an_expired_wait_leaves_the_outcome_unknown_and_drops_the_late_reply(
    tmp_path, caplog
):
    """When ``transport_timeout`` runs out the call raises while the helper
    still runs. The SDK sends nothing more for it, and the helper's reply,
    arriving later, is dropped instead of answering another call."""
    caplog.set_level(logging.DEBUG, logger="meerkat_meta")
    stand_in = _StandIn(tmp_path, {
        "meta/spawn_helper": [
            {"sleep": _HELPER_RUN, "result": _helper_reply("first")},
            {"result": _helper_reply("second")},
        ],
    })
    runtime = await _runtime(stand_in)
    try:
        handle = runtime.mob_handle()
        with pytest.raises(RuntimeError, match="persistent transport: timeout after 0.2s"):
            await handle.spawn_helper("reviewer", "first task", **_HELPER, transport_timeout=0.2)
        (first,) = stand_in.calls("meta/spawn_helper")

        def dropped() -> bool:
            return any(
                f"dropping response for non-pending id={first['id']}" in record.getMessage()
                for record in caplog.records
            )

        deadline = time.monotonic() + 10
        while not dropped() and time.monotonic() < deadline:
            await asyncio.sleep(0.05)
        assert dropped(), "the first helper's reply arrives with no waiter left"

        second = await handle.spawn_helper(
            "reviewer", "second task", **_HELPER, transport_timeout=30.0
        )
    finally:
        await runtime.shutdown()

    assert second.output == "second"
    assert [call["params"]["task"] for call in stand_in.calls("meta/spawn_helper")] == [
        "first task",
        "second task",
    ]


@pytest.mark.asyncio
@pytest.mark.timeout(60)
@pytest.mark.parametrize("bad", [0, -1.0, math.nan, math.inf, True, 1e10])
async def test_a_transport_timeout_the_transport_cannot_wait_sends_nothing(tmp_path, bad):
    """Each value is refused before the request is written: not a positive
    finite number, or (``1e10``) longer than ``threading.TIMEOUT_MAX``, which
    ``Event.wait`` would refuse only after the helper was already spawned."""
    stand_in = _StandIn(tmp_path, {
        "meta/spawn_helper": [{"result": _helper_reply("unused")}],
    })
    runtime = await _runtime(stand_in)
    try:
        with pytest.raises(ValueError, match="positive finite number"):
            await runtime.mob_handle().spawn_helper(
                "reviewer", "review the draft", **_HELPER, transport_timeout=bad
            )
    finally:
        await runtime.shutdown()

    assert stand_in.calls("meta/spawn_helper") == []
