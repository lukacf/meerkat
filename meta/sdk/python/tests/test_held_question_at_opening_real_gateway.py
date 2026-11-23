"""A question set pending before the gateway starts reaches an SDK host.

A member's question set is a durable decision of kind ``ask_user.questions``.
A gateway that starts over a state directory which already holds one open
tells its host of it with no list call
(:meth:`MetaRuntime.on_decision_notice`); the host's first
``decisions_list`` returns that one open set and writes its notice once
more, and a later list writes none.

The pairing and its order
-------------------------

No SDK call opens a question set, so this row needs a state directory that
was seeded before the gateway starts. Three steps, in this order:

1. The seed step, the ignored Rust test
   ``seed_a_pending_question_set_for_an_sdk_row`` of the Meta crate's
   ``decision_notice_at_opening`` test target, run by name. It holds one
   question set through another owner on the store, as the stock tool's host
   does, and writes the carrier file ``sdk-held-question-seed.json`` (the
   set's decision id and when it was seeded) into the directory. With the
   repository's cargo wrapper::

       META_SDK_HELD_QUESTION_STATE=<a fresh directory> \\
           cargo test -p meerkat-meta --test decision_notice_at_opening \\
           -- --ignored --exact seed_a_pending_question_set_for_an_sdk_row

2. A gateway built from the same revision (``rpc_gateway``).

3. This row, from ``meta``, within the hour the seeded set waits for its
   answers::

       META_GATEWAY_BIN=<path to that rpc_gateway> \\
           META_SDK_HELD_QUESTION_STATE=<that directory> \\
           PYTHONPATH=sdk/python python3 -m pytest \\
           sdk/python/tests/test_held_question_at_opening_real_gateway.py -q

The directory is the fixture's own and disposable. The seed step refuses a
path that is not absolute, a symbolic link or a path through one, a
directory that is not absent or empty, the home directory, and a directory
inside a state or configuration home of the user. This row refuses the same
paths, a directory without the carrier file, and a seed older than the hour.
The gateway takes the directory as its persistent state, which is intended:
the decision store it must read lives there. It also writes the rest of its
own state there (its mob, sessions and logs). ``HOME`` and the XDG homes of
the gateway process are directories of this test.

What this row does not show
---------------------------

It runs on the revision it is run on, not on a joined line. It shows nothing
of a restart of a gateway, of how often a notice is delivered across
restarts, or of what a person is shown; it answers nothing and leaves the
set open. It records the calls of the SDK's notice handler only, so a
failed wait says that a notice was not observed by the handler in time, not
that the gateway wrote none. There is no TypeScript row.
"""
from __future__ import annotations

import asyncio
import json
import os
import time
from pathlib import Path
from typing import Any

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime

_GATEWAY_BIN = os.environ.get("META_GATEWAY_BIN", "").strip()
_SEEDED_STATE = os.environ.get("META_SDK_HELD_QUESTION_STATE", "").strip()

_QUESTION_KIND = "ask_user.questions"
_SEED_STEP = "seed_a_pending_question_set_for_an_sdk_row"
_SEED_CARRIER = "sdk-held-question-seed.json"

# How long the row waits for an observation it expects.
_WAIT_SECONDS = 30.0
# How long the row watches before it says that no further notice was observed.
_QUIET_SECONDS = 0.5

_SKIP_REASON = (
    "this row needs both META_GATEWAY_BIN (a built rpc_gateway) and "
    "META_SDK_HELD_QUESTION_STATE (a directory seeded by the ignored Rust test "
    f"{_SEED_STEP} of the Meta crate's decision_notice_at_opening target, "
    "run by name with --ignored)"
)

# Provider keys the gateway process must not inherit.
_PROVIDER_KEYS = (
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "RKAT_ANTHROPIC_API_KEY",
    "RKAT_OPENAI_API_KEY",
    "RKAT_GEMINI_API_KEY",
)

_MOB_TOML = """\
[mob]
id = "held-question-at-opening-real-gateway"

[profiles.default]
model = "gpt-5.5"
"""


def _seeded_state() -> Path:
    """The seeded directory, or a failure that says why it is not used."""
    state = Path(_SEEDED_STATE)
    assert state.is_absolute(), f"META_SDK_HELD_QUESTION_STATE is not an absolute path: {state}"
    assert not state.is_symlink(), f"the seeded directory is a symbolic link: {state}"
    assert os.path.realpath(state.parent) == str(state.parent), (
        f"the path of the seeded directory holds a symbolic link or a relative part: {state}"
    )
    assert state.is_dir(), f"the seeded directory does not exist: {state}; run {_SEED_STEP} first"
    home = os.environ.get("HOME", "")
    kept = [os.environ.get("XDG_STATE_HOME", ""), os.environ.get("XDG_CONFIG_HOME", "")]
    if home:
        assert state != Path(home), f"the seeded directory is the home directory: {state}"
        kept += [os.path.join(home, ".local", "state"), os.path.join(home, ".config")]
    for location in kept:
        assert not (location and state.is_relative_to(location)), (
            f"the seeded directory lies in a state or configuration home of the user: {state}"
        )
    assert (state / _SEED_CARRIER).is_file(), (
        f"{state} holds no {_SEED_CARRIER}: it was not seeded by {_SEED_STEP} "
        "(run that step by name with --ignored on a fresh directory)"
    )
    return state


def _seed(state: Path) -> tuple[str, str]:
    """The seeded decision id and a note of the seed's age.

    Fails, before anything is started, when the seeded set has expired.
    """
    carrier = json.loads((state / _SEED_CARRIER).read_text(encoding="utf-8"))
    decision_id = carrier["decision_id"]
    assert carrier["kind"] == _QUESTION_KIND, carrier
    age_seconds = time.time() - carrier["seeded_at_ms"] / 1000.0
    expires_minutes = carrier["expires_after_seconds"] // 60
    note = (
        f"seeded {int(age_seconds // 60)} minutes ago; "
        f"the question set expires after {expires_minutes}"
    )
    print(note)
    assert age_seconds < carrier["expires_after_seconds"], (
        f"the seed is too old to use: {note}. Run {_SEED_STEP} again on a fresh directory"
    )
    return decision_id, note


async def _observed(notices: list[dict[str, Any]], count: int, what: str, note: str) -> None:
    """Wait until the handler received ``count`` notices."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + _WAIT_SECONDS
    while len(notices) < count:
        assert loop.time() < deadline, (
            f"{what}: not observed by the handler within {_WAIT_SECONDS:g} s "
            f"(the handler received {len(notices)} of {count}; {note})"
        )
        await asyncio.sleep(0.01)


async def _answered(call: Any, what: str, note: str) -> Any:
    """The answer of ``call``, or a failure that names what was awaited."""
    try:
        return await asyncio.wait_for(call, timeout=_WAIT_SECONDS)
    except asyncio.TimeoutError:
        pytest.fail(f"{what}: not received within {_WAIT_SECONDS:g} s ({note})")


@pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN) and _SEEDED_STATE),
    reason=_SKIP_REASON,
)
@pytest.mark.asyncio
async def test_a_question_set_pending_at_the_gateways_start_is_noticed_before_any_list(
    tmp_path, monkeypatch
):
    state = _seeded_state()
    seeded_id, note = _seed(state)

    # The gateway process inherits this process's environment when its
    # transport is built: give it a home and XDG homes inside this test's
    # own directory, and no provider key.
    for name in ("HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
        home = tmp_path / name.lower()
        home.mkdir()
        monkeypatch.setenv(name, str(home))
    for name in _PROVIDER_KEYS:
        monkeypatch.delenv(name, raising=False)

    notices: list[dict[str, Any]] = []
    runtime = MetaRuntime(
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob_inline(_MOB_TOML)
        .persistent_state(str(state))
        ._config
    )
    runtime.on_decision_notice(notices.append)
    try:
        await _answered(runtime.connect(), "the gateway's start (connect)", note)

        # The host has made no list call: the gateway tells of the open
        # question set on its own, once.
        await _observed(notices, 1, "the notice of the seeded question set before any list", note)
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 1, (
            "more than one notice was observed by the handler before any list, "
            f"watched for {_QUIET_SECONDS:g} s after the first: {notices}"
        )
        opening = notices[0]
        assert opening["decision_id"] == seeded_id, (opening, seeded_id)
        assert opening["kind"] == _QUESTION_KIND, opening
        assert opening["event"] == "held", opening
        assert opening["phase"] == "open", opening
        assert opening.get("presentation") is None, opening

        # The host's first list returns exactly that one open question set
        # and writes its notice once more.
        first = await _answered(
            runtime.mob_handle().decisions_list(), "the first list's answer", note
        )
        listed = first["decisions"]
        assert len(listed) == 1, f"the first list returned {len(listed)} decisions: {first}"
        assert listed[0]["decision_id"] == seeded_id, (listed[0], seeded_id)
        assert listed[0]["kind"] == _QUESTION_KIND, listed[0]
        assert listed[0]["phase"] == "open", listed[0]
        await _observed(notices, 2, "the notice of the seeded question set at the first list", note)
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 2, (
            "more than one notice was observed by the handler at the first list, "
            f"watched for {_QUIET_SECONDS:g} s after it: {notices}"
        )
        at_list = notices[1]
        assert at_list["decision_id"] == seeded_id, (at_list, seeded_id)
        assert at_list["kind"] == _QUESTION_KIND, at_list
        assert at_list["event"] == "held", at_list

        # A later list writes none.
        await _answered(runtime.mob_handle().decisions_list(), "the second list's answer", note)
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 2, (
            "a further notice was observed by the handler at the second list, "
            f"watched for {_QUIET_SECONDS:g} s after its answer: {notices}"
        )
    finally:
        await runtime.shutdown()
