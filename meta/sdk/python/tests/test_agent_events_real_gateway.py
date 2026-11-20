"""A member's run reaches the event subscribers of a real gateway.

One ``rpc_gateway`` process, one member, built through the self-hosted model
route with no credential, so the rows need no provider key. The model is the
local chat-completions fake of the cross-mob rows, which answers every
ordinary message with text.

A new member runs once when it is spawned, before anything is sent to it.
Each row lets that first run finish, sends one message and then does nothing
but wait: the last events of the run the send started, ``turn_completed`` and
``run_completed``, must reach the subscriber without a later run to push them
through. The agent rows subscribe before the send and after its run has
finished; either way the gateway replays the member's earlier events, so they
see both runs. The mob row subscribes before the send and sees that run only
(the mob stream does not replay).
"""
from __future__ import annotations

import asyncio
import os
from pathlib import Path

import pytest

from .gateway_test_support import gateway_transport_factory

from meerkat_meta import Meta

from .test_wire_cross_mob_real_gateways import (
    _GATEWAY_BIN,
    _FakeModel,
    _config_toml,
    _mob_toml,
)

pytestmark = pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)

# How long a run and the delivery of its events may take together. The model
# answers at once; a subscriber that waits for a later run never gets there.
_RUN_SECONDS = 20.0
# How long a subscription gets to connect before the send, where the stream
# does not replay.
_CONNECT_SECONDS = 3.0

_ONE_RUN = ["RunStarted", "TurnCompleted", "RunCompleted"]


async def _gateway(tmp_path: Path, mob_id: str, base_url: str):
    root = tmp_path / mob_id
    root.mkdir()
    (root / "mob.toml").write_text(_mob_toml(mob_id))
    (root / "config.toml").write_text(_config_toml(base_url))
    return await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .gateway_transport(gateway_transport_factory(_GATEWAY_BIN, root))
        .mob(str(root / "mob.toml"))
        .meerkat_config(root / "config.toml")
        .console_auth_required(False)
        .build()
    )


class _Subscriber:
    """Reads a subscription until the stream ends, keeping each event's kind
    (the name of its typed event class)."""

    def __init__(self, events) -> None:
        self._kinds: list[str] = []
        self._task = asyncio.ensure_future(self._read(events))

    async def _read(self, events) -> None:
        async for event in events:
            self._kinds.append(type(event.event).__name__)

    async def kinds_once_runs_completed(self, runs: int, seconds: float) -> list[str]:
        """The kinds received once ``runs`` runs have completed, or after
        ``seconds`` without that."""
        deadline = asyncio.get_running_loop().time() + seconds
        while (
            self._kinds.count("RunCompleted") < runs
            and asyncio.get_running_loop().time() < deadline
        ):
            await asyncio.sleep(0.05)
        return list(self._kinds)

    async def stopped(self) -> None:
        """Call once the gateway has shut down: its end of the stream is what
        lets a read still waiting for bytes return."""
        await asyncio.wait({self._task}, timeout=_RUN_SECONDS)
        if not self._task.done():
            self._task.cancel()
        await asyncio.gather(self._task, return_exceptions=True)


async def _runs_finished(handle, fake: _FakeModel, runs: int) -> bool:
    """Whether the member's first ``runs`` runs have finished within
    ``_RUN_SECONDS``: the model was asked that many times and the member has
    no run open. Reads no event stream."""
    deadline = asyncio.get_running_loop().time() + _RUN_SECONDS
    while asyncio.get_running_loop().time() < deadline:
        if len(fake.requests) >= runs:
            progress = (await handle.member_status("agent-1")).progress
            if (
                progress is not None
                and progress.run_state == "idle"
                and progress.in_flight_work == 0
            ):
                return True
        await asyncio.sleep(0.1)
    return False


def _key_events(kinds: list[str]) -> list[str]:
    return [kind for kind in kinds if kind in _ONE_RUN]


def _late(runs: int, kinds: list[str]) -> str:
    return (
        f"the events of {runs} finished run(s) did not all reach the subscriber "
        f"within {_RUN_SECONDS:.0f} s, with nothing sent after the last run; "
        f"received {kinds}"
    )


@pytest.mark.asyncio
async def test_a_subscriber_receives_run_completed_with_nothing_sent_after_the_run(tmp_path):
    fake = _FakeModel()
    runtime = subscriber = None
    try:
        runtime = await _gateway(tmp_path, "agent-events-live", fake.base_url)
        handle = runtime.mob_handle()
        await handle.ensure_member("agent-1", "worker")
        assert await _runs_finished(handle, fake, 1), "the member's first run did not finish"

        subscriber = _Subscriber(handle.subscribe_agent("agent-1"))
        await handle.send("agent-1", "Hello")
        kinds = await subscriber.kinds_once_runs_completed(2, _RUN_SECONDS)
        assert await _runs_finished(handle, fake, 2), "the send's run did not finish"
    finally:
        if runtime is not None:
            await runtime.shutdown()
        if subscriber is not None:
            await subscriber.stopped()
        fake.close()

    assert _key_events(kinds) == _ONE_RUN + _ONE_RUN, _late(2, kinds)


@pytest.mark.asyncio
async def test_a_subscriber_that_connects_after_the_run_receives_it_whole(tmp_path):
    fake = _FakeModel()
    runtime = subscriber = None
    try:
        runtime = await _gateway(tmp_path, "agent-events-replay", fake.base_url)
        handle = runtime.mob_handle()
        await handle.ensure_member("agent-1", "worker")
        assert await _runs_finished(handle, fake, 1), "the member's first run did not finish"

        await handle.send("agent-1", "Hello")
        assert await _runs_finished(handle, fake, 2), "the send's run did not finish"
        subscriber = _Subscriber(handle.subscribe_agent("agent-1"))
        kinds = await subscriber.kinds_once_runs_completed(2, _RUN_SECONDS)
    finally:
        if runtime is not None:
            await runtime.shutdown()
        if subscriber is not None:
            await subscriber.stopped()
        fake.close()

    assert _key_events(kinds) == _ONE_RUN + _ONE_RUN, _late(2, kinds)


@pytest.mark.asyncio
async def test_a_mob_subscriber_receives_run_completed_with_nothing_sent_after_the_run(
    tmp_path,
):
    fake = _FakeModel()
    runtime = subscriber = None
    try:
        runtime = await _gateway(tmp_path, "mob-events-live", fake.base_url)
        handle = runtime.mob_handle()
        await handle.ensure_member("agent-1", "worker")
        assert await _runs_finished(handle, fake, 1), "the member's first run did not finish"

        subscriber = _Subscriber(handle.subscribe_mob())
        await asyncio.sleep(_CONNECT_SECONDS)
        await handle.send("agent-1", "Hello")
        kinds = await subscriber.kinds_once_runs_completed(1, _RUN_SECONDS)
        assert await _runs_finished(handle, fake, 2), "the send's run did not finish"
    finally:
        if runtime is not None:
            await runtime.shutdown()
        if subscriber is not None:
            await subscriber.stopped()
        fake.close()

    assert _key_events(kinds) == _ONE_RUN, _late(1, kinds)
