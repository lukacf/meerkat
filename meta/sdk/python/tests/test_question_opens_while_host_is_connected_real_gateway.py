"""A question an agent asks while the SDK host is connected reaches the host.

One ``rpc_gateway`` process over a persistent state directory, for a launch
with a roster of one long-term agent, built through the self-hosted model
route with no credential, so the row needs no provider key and no seeded
state: the model is a local chat-completions fake that calls the stock tool
``ask_user`` once, when the agent is sent the row's trigger, and answers
every other request with text.

The roster matters: a gateway serves the questions of its agents only for a
launch with one. On a launch without a roster the model's call is answered
``no_human_surface`` at once, nothing is held and nothing is noticed.

The row follows one question from the agent to the person's side and back:

1. The host registers its handler (:meth:`MetaRuntime.on_decision_notice`)
   and makes no list call. Nothing is noticed while nothing is asked.
2. The agent is sent the trigger (:meth:`MetaRuntime.send`) and its model
   calls ``ask_user``. The
   handler receives one ``held`` notice of kind ``ask_user.questions`` for a
   decision that opened while the host was connected.
3. With the id the notice names the host reads the set
   (:meth:`MobHandle.gating_questions`; a set's ``pending_id`` is the
   decision's id): it holds what the model asked, and its origin names the
   asking agent.
4. The host answers (:meth:`MobHandle.gating_answer`). The set leaves the
   open sets, and the agent's model is handed the typed result on a later
   request: an ``ask_user_result`` input that says ``answered`` and carries
   the chosen choice.

The gateway keeps the SDK's default Console policy: it requires sign-in and
trusts no signing key, so no person can answer in its Console and the
embedding host is the one that can. That is the case the notice exists for.

What this row does not show
---------------------------

It shows nothing of a restart of a gateway, of a second host connection, or
of what a person is shown. The host answers as the embedding host (the stdio
decide trusts its host and records the ``approver_id`` it names). The row
records the calls of the SDK's notice handler only, so a failed wait says
that a notice was not observed by the handler in time, not that the gateway
wrote none. The rows of a question set that is open BEFORE the gateway
starts are in ``test_held_question_at_opening_real_gateway.py``.
"""
from __future__ import annotations

import asyncio
import json
import os
import re
from typing import Any

import pytest

from meerkat_meta import Meta
from meerkat_meta.identity_first_models import DurableAgentSpec
from meerkat_meta.types import GatingQuestionAnswer

from .test_wire_cross_mob_real_gateways import (
    _GATEWAY_BIN,
    _MODEL,
    _FakeModel,
    _config_toml,
)

pytestmark = pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)

_QUESTION_KIND = "ask_user.questions"
_AGENT = "agent-a"
# What the row sends the agent to make its model ask.
_ASK_NOW = "ASK-THE-PERSON-NOW-KILO-5"
# What the model asks: the arguments of its one ``ask_user`` call.
_ASKED = {
    "title": "Release plan",
    "questions": [
        {
            "id": "day",
            "prompt": "Which day?",
            "choices": [
                {"id": "mon", "label": "Monday"},
                {"id": "fri", "label": "Friday"},
            ],
        }
    ],
}
# The choice the host answers with.
_CHOSEN = "fri"
# The approver the host names. The stdio decide trusts its host and records
# the name it gives; nobody is authenticated by it.
_ANSWERS_AS = "sdk-question-row"

# The launch's mob: one profile, whose agents run a turn when they are sent
# something and none before.
_MOB_TOML = f"""\
[mob]
id = "question-opens-while-connected"

[profiles.worker]
model = "{_MODEL}"
provider = "self_hosted"
self_hosted_server_id = "local"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"""

# How long the row waits for an observation it expects.
_WAIT_SECONDS = 30.0
# How long the row watches before it says that no notice was observed.
_QUIET_SECONDS = 0.5


class _AskingModel(_FakeModel):
    """The cross-mob rows' chat-completions fake, scripted to call
    ``ask_user`` once: at the first request that carries the trigger."""

    def __init__(self) -> None:
        self._asked = False
        super().__init__()

    def answer(self, body: dict) -> list[dict]:
        text = json.dumps(body.get("messages", []))
        with self._lock:
            self.requests.append(text)
            if _ASK_NOW in text and not self._asked:
                self._asked = True
                delta: dict[str, Any] = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call-ask-1",
                            "type": "function",
                            "function": {"name": "ask_user", "arguments": json.dumps(_ASKED)},
                        }
                    ],
                }
                finish = "tool_calls"
            else:
                delta = {"role": "assistant", "content": "observed"}
                finish = "stop"
        base = {"id": "fake", "object": "chat.completion.chunk", "created": 1, "model": _MODEL}
        return [
            {**base, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {**base, "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
            {
                **base,
                "choices": [],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            },
        ]

    def request_matching(self, *patterns: str) -> str | None:
        """A request the model was sent in which every pattern is found."""
        with self._lock:
            for text in self.requests:
                if all(re.search(pattern, text) for pattern in patterns):
                    return text
        return None

    def asked(self) -> bool:
        with self._lock:
            return self._asked


async def _until(condition, what: str) -> None:
    """Wait until ``condition()`` holds, or fail naming what was awaited."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + _WAIT_SECONDS
    while not condition():
        assert loop.time() < deadline, f"{what}: not observed within {_WAIT_SECONDS:g} s"
        await asyncio.sleep(0.02)


class _OneAgentRoster:
    """The launch's roster: one long-term agent, of the profile ``worker``."""

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(identity=_AGENT, profile="worker", addressability="addressable")
        ]


@pytest.mark.asyncio
async def test_a_question_an_agent_asks_while_the_host_is_connected_is_noticed_read_and_answered(
    tmp_path,
):
    root = tmp_path / "question-opens-while-connected"
    root.mkdir()
    (root / "mob.toml").write_text(_MOB_TOML)
    fake = _AskingModel()
    runtime = None
    notices: list[dict[str, Any]] = []
    try:
        (root / "config.toml").write_text(_config_toml(fake.base_url))
        runtime = await (
            Meta.builder()
            .gateway(_GATEWAY_BIN)
            .mob(str(root / "mob.toml"))
            .meerkat_config(root / "config.toml")
            .persistent_state(str(root / "state"))
            .roster(_OneAgentRoster())
            .build()
        )
        runtime.on_decision_notice(notices.append)
        handle = runtime.mob_handle()

        # Nothing was asked, so nothing is noticed.
        await asyncio.sleep(_QUIET_SECONDS)
        assert notices == [], f"a notice was observed before anything was asked: {notices}"

        # The agent is sent the trigger; its model calls ask_user. The host
        # makes no list call: the gateway tells it of the decision that
        # opened while it was connected.
        await runtime.send(_AGENT, f"{_ASK_NOW}: ask which day to release.")
        await _until(fake.asked, "the model's ask_user call")
        await _until(lambda: len(notices) >= 1, "the notice of the question the agent asked")
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 1, (
            "more than one notice was observed by the handler with no list call, "
            f"watched for {_QUIET_SECONDS:g} s after the first: {notices}"
        )
        noticed = notices[0]
        assert noticed["event"] == "held", noticed
        assert noticed["kind"] == _QUESTION_KIND, noticed
        assert noticed["phase"] == "open", noticed
        assert noticed.get("presentation") is None, noticed

        # With that id the host reads what was asked, and who asked.
        open_sets = await handle.gating_questions()
        named = [one for one in open_sets if one.pending_id == noticed["decision_id"]]
        assert len(named) == 1, (
            f"gating_questions returned {len(named)} sets with the noticed id "
            f"{noticed['decision_id']}: {open_sets}"
        )
        asked = named[0]
        assert asked.subject_digest == noticed["rendering_ref"], (asked, noticed)
        assert asked.origin is not None and asked.origin.identity == _AGENT, asked
        assert asked.title == _ASKED["title"], asked
        assert [question.id for question in asked.questions] == ["day"], asked
        assert [choice.id for choice in asked.questions[0].choices] == ["mon", "fri"], asked

        # The host answers. The set leaves the open sets.
        result = await handle.gating_answer(
            asked.pending_id,
            approver_id=_ANSWERS_AS,
            presented_digest=asked.subject_digest,
            answers=[GatingQuestionAnswer.choice("day", _CHOSEN)],
        )
        assert result.pending_id == noticed["decision_id"], (result, noticed)
        assert result.kind == "question_set", result
        assert result.outcome == "answered", result
        still_open = await handle.gating_questions()
        assert [one.pending_id for one in still_open] == [], still_open

        # The asking agent's model is handed the typed result: an
        # ask_user_result input that says answered and carries the choice.
        # The request is matched as JSON text inside JSON text, so a quote
        # may stand behind backslashes.
        await _until(
            lambda: fake.request_matching(
                r"ask_user_result",
                r'outcome\\*"\s*:\s*\\*"answered\b',
                rf'choice_id\\*"\s*:\s*\\*"{_CHOSEN}\b',
            )
            is not None,
            "a model request that carries the answered result",
        )

        # The answer wrote no further notice.
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 1, (
            "a further notice was observed by the handler after the answer, "
            f"watched for {_QUIET_SECONDS:g} s: {notices}"
        )
    finally:
        if runtime is not None:
            await runtime.shutdown()
        fake.close()
