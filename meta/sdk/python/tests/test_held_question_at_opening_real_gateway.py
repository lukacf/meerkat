"""A question set pending before the gateway starts reaches an SDK host.

A member's question set is a durable decision of kind ``ask_user.questions``.
A gateway that starts over a state directory which already holds one open
tells its host of it with no list call
(:meth:`MetaRuntime.on_decision_notice`); the host's first
``decisions_list`` writes that decision's notice once more, and a later list
writes none.

No SDK call opens a question set, so this row needs a state directory that
was seeded before the gateway starts. The seed step is the ignored Rust test
``seed_a_pending_question_set_for_an_sdk_row`` of the Meta crate's
``decision_notice_at_opening`` test target: it holds one question set
through another owner on the store, as the stock tool's host does. With the
repository's cargo wrapper::

    META_SDK_HELD_QUESTION_STATE=<an empty directory> \\
        cargo test -p meerkat-meta --test decision_notice_at_opening \\
        -- --ignored --exact seed_a_pending_question_set_for_an_sdk_row

and then, from ``meta``::

    META_GATEWAY_BIN=<path to an rpc_gateway> \\
        META_SDK_HELD_QUESTION_STATE=<that directory> \\
        PYTHONPATH=sdk/python python3 -m pytest \\
        sdk/python/tests/test_held_question_at_opening_real_gateway.py -q

The seeded set waits one hour for its answers, so seed the directory shortly
before the row runs. The row answers nothing and leaves the set open.
"""
from __future__ import annotations

import asyncio
import os
from typing import Any

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime

_GATEWAY_BIN = os.environ.get("META_GATEWAY_BIN", "").strip()
_SEEDED_STATE = os.environ.get("META_SDK_HELD_QUESTION_STATE", "").strip()

_QUESTION_KIND = "ask_user.questions"

# How long the row waits for a notice it expects.
_WAIT_SECONDS = 30.0
# How long the row waits before it says that no further notice came.
_QUIET_SECONDS = 0.5

_MOB_TOML = """\
[mob]
id = "held-question-at-opening-real-gateway"

[profiles.default]
model = "gpt-5.5"
"""


async def _until(notices: list[dict[str, Any]], count: int) -> None:
    """Wait until ``count`` notices arrived."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + _WAIT_SECONDS
    while len(notices) < count:
        assert loop.time() < deadline, (count, notices)
        await asyncio.sleep(0.01)


@pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)
@pytest.mark.skipif(
    not (_SEEDED_STATE and os.path.isdir(_SEEDED_STATE)),
    reason="META_SDK_HELD_QUESTION_STATE not set to a seeded state directory",
)
@pytest.mark.asyncio
async def test_a_question_set_pending_at_the_gateways_start_is_noticed_before_any_list(
    tmp_path, monkeypatch
):
    # The gateway keeps its own identity files under the state home: give it
    # one inside this test's directory.
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "xdg-state"))
    notices: list[dict[str, Any]] = []
    runtime = MetaRuntime(
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob_inline(_MOB_TOML)
        .persistent_state(_SEEDED_STATE)
        ._config
    )
    runtime.on_decision_notice(notices.append)
    try:
        await runtime.connect()

        # The host has made no list call: the gateway tells of the open
        # question set on its own, once.
        await _until(notices, 1)
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 1, notices
        opening = notices[0]
        assert opening["kind"] == _QUESTION_KIND, opening
        assert opening["event"] == "held", opening
        assert opening["phase"] == "open", opening
        assert opening.get("presentation") is None, opening
        decision_id = opening["decision_id"]

        # The host's first list writes that decision's notice once more.
        first = await runtime.mob_handle().decisions_list()
        assert isinstance(first, dict), first
        await _until(notices, 2)
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 2, notices
        listed = notices[1]
        assert listed["decision_id"] == decision_id, listed
        assert listed["kind"] == _QUESTION_KIND, listed
        assert listed["event"] == "held", listed

        # A later list writes none.
        await runtime.mob_handle().decisions_list()
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 2, notices
    finally:
        await runtime.shutdown()
