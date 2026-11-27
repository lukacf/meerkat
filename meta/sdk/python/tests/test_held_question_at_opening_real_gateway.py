"""A question set pending before the gateway starts reaches an SDK host.

A member's question set is a durable decision of kind ``ask_user.questions``.
A gateway that starts over a state directory which already holds one open
tells its host of it with no list call
(:meth:`MetaRuntime.on_decision_notice`); the host's first
``decisions_list`` returns that one open set and writes its notice once
more, and a later list writes none.

A second row follows the notice to its end as a host would: with the id the
notice names it reads the set (:meth:`MobHandle.gating_questions`; a set's
``pending_id`` is the decision's id) and answers it
(:meth:`MobHandle.gating_answer`). The set then leaves the open sets and its
decision is no longer open. Its launch names a roster (one long-term agent
that is never sent anything): a gateway serves and lists the questions of
its agents only for a launch with a roster. On a launch without one, as the
first row's, the notice still arrives and ``decisions_list`` returns the
decision, but ``gating_questions`` returns no set.

The two rows run in the order of this file, and the second one ends the set:
a seeded directory serves ONE run of this file. The second row leaves a
marker file beside the directory (the directory's name with the suffix
``.sdk-held-question-answered``), and both rows refuse a directory that has
one. The TypeScript SDK has the same two rows
(``sdk/typescript/tests/held-question-at-opening-real-gateway.test.ts``);
they need a seeded directory of their own.

The pairing and its order
-------------------------

No SDK call opens a question set, so this row needs a state directory that
was seeded before the gateway starts. Three steps, in this order:

1. The seed step, the ignored Rust test
   ``seed_a_pending_question_set_for_an_sdk_row`` of the Meta crate's
   ``decision_notice_at_opening`` test target, run by name. It holds one
   question set through another owner on the store, as the stock tool's host
   does, and writes a carrier file (the set's decision id, when it was
   seeded and after how many seconds it expires) BESIDE the directory, not
   in it: in the directory's parent, named as the directory with the suffix
   ``.sdk-held-question-seed.json``. With the repository's cargo wrapper::

       META_SDK_HELD_QUESTION_STATE=<a fresh directory> \\
           cargo test -p meerkat-meta --test decision_notice_at_opening \\
           -- --ignored --exact seed_a_pending_question_set_for_an_sdk_row

2. A gateway built from the same revision (``rpc_gateway``).

3. This row, from ``meta``, before the seeded set expires (the carrier file
   says when it was seeded and after how long it expires; the row holds no
   number of its own for that)::

       META_GATEWAY_BIN=<path to that rpc_gateway> \\
           META_SDK_HELD_QUESTION_STATE=<that directory> \\
           PYTHONPATH=sdk/python python3 -m pytest \\
           sdk/python/tests/test_held_question_at_opening_real_gateway.py -q

The directory is the fixture's own and disposable. The seed step refuses a
path that is not absolute, a symbolic link or a path through one, a
directory that is not absent or empty, a carrier file that already exists,
any path in the home directory's tree (the home directory and everything
under it, compared in resolved form), and a directory inside
``$XDG_STATE_HOME`` or ``$XDG_CONFIG_HOME``. This row refuses the same
paths, a directory without its carrier file beside it, and an expired seed.
The gateway takes the directory as its persistent state, which is intended:
the decision store it must read lives there. It also writes the rest of its
own state there (its mob, sessions and logs). The carrier file is outside
the directory, so the gateway is given only what the store wrote.

The gateway process gets an environment built from an allow list, not this
process's environment with names removed: ``HOME`` and the four XDG homes
are directories of this test, and ``PATH``, the locale variables, ``TMPDIR``
and the SDK transport's two stderr switches are passed on when set (see
``_PASSED_ON``). Nothing else is inherited, so no provider key reaches the
gateway whatever its name.

What these rows do not show
---------------------------

They run on the revision they are run on, not on a joined line. They show
nothing of a restart of a gateway, of how often a notice is delivered across
restarts, or of what a person is shown. The set was seeded before the
gateway started: a question that opens while the host is connected is not
shown here. The second row answers as the embedding host (the stdio decide
trusts its host and records the ``approver_id`` it names); it does not show
that the asking agent receives the answers, since the seeded asker is no
agent of the row's roster (``test_question_opens_while_host_is_connected_real_gateway.py``
shows that, for a question an agent of the launch asks). The rows record the calls of the SDK's notice
handler only, so a failed wait says that a notice was not observed by the
handler in time, not that the gateway wrote none.
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
from meerkat_meta.identity_first_models import DurableAgentSpec
from meerkat_meta.runtime import MetaRuntime
from meerkat_meta.types import GatingQuestionAnswer

_GATEWAY_BIN = os.environ.get("META_GATEWAY_BIN", "").strip()
_SEEDED_STATE = os.environ.get("META_SDK_HELD_QUESTION_STATE", "").strip()

_QUESTION_KIND = "ask_user.questions"
_SEED_STEP = "seed_a_pending_question_set_for_an_sdk_row"
_SEED_CARRIER_SUFFIX = ".sdk-held-question-seed.json"
# The marker the answering row leaves beside a seeded directory whose set it
# ended: such a directory holds no open set for another run.
_ANSWERED_MARKER_SUFFIX = ".sdk-held-question-answered"
# The approver the answering row names. The stdio decide trusts its host and
# records the name it gives; nobody is authenticated by it.
_ANSWERS_AS = "sdk-held-question-row"
# The one agent of the answering row's roster. It is not the seeded asker.
_ROSTER_AGENT = "held-question-row-agent"

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

# The whole environment of the gateway process is an allow list: the names
# below, passed on from this process when set, and the homes the row sets.
# Nothing else is inherited, so no provider key reaches the gateway whatever
# its name.
_PASSED_ON = (
    # Programs the gateway starts by name are found as on the host.
    "PATH",
    # The locale: the text encoding of the gateway's own output.
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    # Where the gateway's temporary files go.
    "TMPDIR",
    # The SDK transport's own switches for where the gateway's stderr goes;
    # the transport reads them from the same mapping.
    "META_GATEWAY_STDERR",
    "META_GATEWAY_STDERR_FILE",
)
# Set by the row to directories of its own, so the gateway writes nothing to
# the user's home or XDG homes.
_OWN_HOMES = ("HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME")

_MOB_TOML = """\
[mob]
id = "held-question-at-opening-real-gateway"

[profiles.default]
model = "gpt-5.5"
"""


def _carrier_of(state: Path) -> Path:
    """The carrier file of a seeded directory: beside it, never inside it."""
    return state.parent / (state.name + _SEED_CARRIER_SUFFIX)


def _answered_marker_of(state: Path) -> Path:
    """The marker of a seeded directory whose set was answered: beside it."""
    return state.parent / (state.name + _ANSWERED_MARKER_SUFFIX)


def _seeded_state() -> Path:
    """The seeded directory, or a failure that says why it is not used."""
    state = Path(_SEEDED_STATE)
    assert state.is_absolute(), f"META_SDK_HELD_QUESTION_STATE is not an absolute path: {state}"
    assert not state.is_symlink(), f"the seeded directory is a symbolic link: {state}"
    assert os.path.realpath(state.parent) == str(state.parent), (
        f"the path of the seeded directory holds a symbolic link or a relative part: {state}"
    )
    assert state.is_dir(), f"the seeded directory does not exist: {state}; run {_SEED_STEP} first"
    resolved_state = Path(os.path.realpath(state))
    for name in ("HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME"):
        location = os.environ.get(name, "")
        if not location:
            continue
        resolved_location = os.path.realpath(location)
        assert not resolved_state.is_relative_to(resolved_location), (
            f"the seeded directory lies in the tree of {name} ({resolved_location}): {state}"
        )
    carrier = _carrier_of(state)
    assert carrier.is_file() and not carrier.is_symlink(), (
        f"there is no carrier file {carrier} beside {state}: it was not seeded by "
        f"{_SEED_STEP} (run that step by name with --ignored on a fresh directory)"
    )
    marker = _answered_marker_of(state)
    assert not marker.exists(), (
        f"the question set of {state} was answered by an earlier run of this file "
        f"({marker} says so): run {_SEED_STEP} again on a fresh directory"
    )
    return state


def _seed(state: Path) -> tuple[str, str]:
    """The seeded decision id and a note of the seed's age.

    Fails, before anything is started, when the seeded set has expired. The
    expiry is the carrier's own; the row holds no number for it.
    """
    carrier = json.loads(_carrier_of(state).read_text(encoding="utf-8"))
    decision_id = carrier["decision_id"]
    assert carrier["kind"] == _QUESTION_KIND, carrier
    age_seconds = time.time() - carrier["seeded_at_ms"] / 1000.0
    expires_minutes = carrier["expires_after_seconds"] // 60
    note = (
        f"seeded {int(age_seconds // 60)} minutes ago; "
        f"the question set expires {expires_minutes} minutes after its seed"
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


def _only_the_allow_list_is_inherited(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Make this process's environment the gateway's allow list.

    The gateway process takes this process's environment when its transport
    is built, which happens after this call. Every name that is not passed on
    is removed, then the homes are set to directories of the test. The names
    removed here come back when the test ends.
    """
    for name in list(os.environ):
        if name not in _PASSED_ON:
            monkeypatch.delenv(name)
    for name in _OWN_HOMES:
        home = tmp_path / name.lower()
        home.mkdir()
        monkeypatch.setenv(name, str(home))
    not_allowed = sorted(set(os.environ) - set(_PASSED_ON) - set(_OWN_HOMES))
    assert not not_allowed, f"names outside the allow list would be inherited: {not_allowed}"


class _OneAgentRoster:
    """A roster of one long-term agent: what makes the launch one whose
    gateway serves and lists its agents' questions. The agent is not the
    seeded asker and is never sent anything, so no model is asked."""

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity=_ROSTER_AGENT,
                profile="default",
                addressability="addressable",
                runtime_mode_override="turn_driven",
            )
        ]


def _runtime_over(state: Path, *, roster: Any | None = None) -> MetaRuntime:
    """A runtime whose gateway takes the seeded directory as its state,
    for a launch with ``roster`` when one is given."""
    builder = (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob_inline(_MOB_TOML)
        .persistent_state(str(state))
    )
    if roster is not None:
        builder = builder.roster(roster)
    return MetaRuntime(builder._config)


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
    _only_the_allow_list_is_inherited(tmp_path, monkeypatch)

    notices: list[dict[str, Any]] = []
    runtime = _runtime_over(state)
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


@pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN) and _SEEDED_STATE),
    reason=_SKIP_REASON,
)
@pytest.mark.asyncio
async def test_the_host_reads_and_answers_the_question_set_its_notice_names(
    tmp_path, monkeypatch
):
    state = _seeded_state()
    seeded_id, note = _seed(state)
    _only_the_allow_list_is_inherited(tmp_path, monkeypatch)

    notices: list[dict[str, Any]] = []
    # A launch with a roster: the gateway serves and lists its agents'
    # questions only then.
    runtime = _runtime_over(state, roster=_OneAgentRoster())
    runtime.on_decision_notice(notices.append)
    try:
        await _answered(runtime.connect(), "the gateway's start (connect)", note)
        handle = runtime.mob_handle()

        # The notice is all the host has: the id of an open decision and its
        # kind, which says that an agent asked a person something.
        await _observed(notices, 1, "the notice of the seeded question set before any list", note)
        noticed = notices[0]
        assert noticed["event"] == "held", noticed
        assert noticed["kind"] == _QUESTION_KIND, noticed
        assert noticed["decision_id"] == seeded_id, (noticed, seeded_id)

        # With that id the host reads what was asked: the one open set whose
        # pending id is the decision's id.
        open_sets = await _answered(
            handle.gating_questions(), "the open question sets (gating_questions)", note
        )
        named = [one for one in open_sets if one.pending_id == noticed["decision_id"]]
        assert len(named) == 1, (
            f"gating_questions returned {len(named)} sets with the noticed id "
            f"{noticed['decision_id']}: {open_sets}"
        )
        asked = named[0]
        assert asked.questions, asked
        assert all(question.choices for question in asked.questions), asked
        assert asked.subject_digest == noticed["rendering_ref"], (asked, noticed)

        # The host answers every question with its first choice, as the set
        # it read offers them.
        result = await _answered(
            handle.gating_answer(
                asked.pending_id,
                approver_id=_ANSWERS_AS,
                presented_digest=asked.subject_digest,
                answers=[
                    GatingQuestionAnswer.choice(question.id, question.choices[0].id)
                    for question in asked.questions
                ],
            ),
            "the answer's result (gating_answer)",
            note,
        )
        # The set is ended from here on: say so beside the directory before
        # anything else can fail.
        _answered_marker_of(state).write_text(
            json.dumps({"decision_id": seeded_id, "outcome": result.outcome}), encoding="utf-8"
        )
        assert result.pending_id == seeded_id, (result, seeded_id)
        assert result.kind == "question_set", result
        assert result.outcome == "answered", result

        # The answered set is no longer among the open sets, and its
        # decision is no longer open.
        still_open = await _answered(
            handle.gating_questions(), "the open question sets after the answer", note
        )
        assert [one.pending_id for one in still_open] == [], still_open
        listed = await _answered(
            handle.decisions_list(), "the list after the answer (decisions_list)", note
        )
        phases = {
            row["decision_id"]: row["phase"]
            for row in [*listed.get("decisions", []), *listed.get("statuses", [])]
        }
        assert seeded_id in phases, (listed, seeded_id)
        assert phases[seeded_id] != "open", phases

        # A list notices open decisions only: the ended set gets no further
        # notice.
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 1, (
            "a further notice was observed by the handler after the answer, "
            f"watched for {_QUIET_SECONDS:g} s after the list: {notices}"
        )
    finally:
        await runtime.shutdown()
