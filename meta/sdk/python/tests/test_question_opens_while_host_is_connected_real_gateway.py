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
   calls ``ask_user``. The handler receives one ``held`` notice of kind
   ``ask_user.questions`` for a decision that opened while the host was
   connected. The row counts the requests the host had written to the
   gateway before that notice arrived: none under ``meta/decisions/`` or
   ``meta/gating/``.
3. With the id the notice names the host reads the set
   (:meth:`MobHandle.gating_questions`; a set's ``pending_id`` is the
   decision's id): it holds what the model asked, and its origin names the
   asking agent.
4. The host answers (:meth:`MobHandle.gating_answer`). The set leaves the
   open sets, and the agent's model is sent the end of its request on a later
   model request: an ``ask_user_result`` input, a label on a line of its own
   and then the result as one JSON document. The row parses that document:
   it carries the request id the call's own result gave, the outcome
   ``answered`` and the chosen choice.

The gateway keeps the SDK's default Console policy: it requires sign-in and
trusts no signing key, so no person can answer in its Console and the
embedding host is the one that can. That is the case the notice exists for.

How the row knows what the host asked
-------------------------------------

Every line the SDK writes to the gateway's stdin and every line it reads
from the gateway's stdout passes one function each of the SDK's transport.
The row wraps both before the gateway starts and keeps the direction and the
method of each line, so "no list call before the notice" is a count over
everything this host wrote, the SDK's own requests included.

The gateway process gets an environment built from an allow list, not this
process's environment with names removed: ``HOME`` and the four XDG homes
are directories of this test, so the key pair a gateway writes for an agent
it runs lands under the test's own directory, and no provider key reaches
the gateway whatever its name.

What this row does not show
---------------------------

It shows nothing of a restart of a gateway, of a second host connection, or
of what a person is shown. The host answers as the embedding host (the stdio
decide trusts its host and records the ``approver_id`` it names). The model
is the row's own fake, so the row shows what the agent's model is sent, not
what a model does with it. The rows of a question set that is open BEFORE
the gateway starts are in ``test_held_question_at_opening_real_gateway.py``.
"""
from __future__ import annotations

import asyncio
import json
import os
import threading
from typing import Any

import pytest

from meerkat_meta import Meta
from meerkat_meta._transport import PersistentTransport
from meerkat_meta.identity_first_models import DurableAgentSpec
from meerkat_meta.types import GatingQuestionAnswer

from .test_held_question_at_opening_real_gateway import _only_the_allow_list_is_inherited
from .test_wire_cross_mob_real_gateways import (
    _MODEL,
    _FakeModel,
    _config_toml,
)

# The gateway the row starts: the one this variable names, and no other.
# The row is skipped only when the variable is not set. A variable that is
# set and names no executable file fails the row: a path that was supplied
# and is wrong is not a reason to skip.
_GATEWAY_BIN = os.environ.get("META_GATEWAY_BIN", "").strip()

pytestmark = pytest.mark.skipif(
    not _GATEWAY_BIN,
    reason="META_GATEWAY_BIN is not set: the row needs it to name a built rpc_gateway",
)

_QUESTION_KIND = "ask_user.questions"
_AGENT = "agent-a"
# What the row sends the agent to make its model ask.
_ASK_NOW = "ASK-THE-PERSON-NOW-KILO-5"
# The id the model gives its one ``ask_user`` call.
_CALL_ID = "call-ask-1"
# What the model asks: the arguments of that call.
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
# The label of the input that carries the end of a request, on a line of its
# own before the result.
_RESULT_LABEL = "ask_user_result"
# The requests of a host that read or decide decisions and question sets.
_DECISION_METHODS = ("meta/decisions/", "meta/gating/")
_NOTICE_METHOD = "meta/decisions/notice"

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


class _Wire:
    """Every line between this host and its gateway, in order: which way it
    went and its method (``None`` for an answer to a request)."""

    def __init__(self) -> None:
        self._lines: list[tuple[str, str | None]] = []
        self._lock = threading.Lock()

    def wrap_the_transport(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Record every line of every gateway started after this call. The
        transport's two functions come back when the test ends."""
        write_line = PersistentTransport._write_line
        read_line = PersistentTransport._read_line
        wire = self

        def recorded_write(transport, obj, *rest, **named):
            wire._add("sent", obj)
            return write_line(transport, obj, *rest, **named)

        def recorded_read(transport, line, *rest, **named):
            try:
                message = json.loads(line)
            except ValueError:
                message = None
            wire._add("received", message)
            return read_line(transport, line, *rest, **named)

        monkeypatch.setattr(PersistentTransport, "_write_line", recorded_write)
        monkeypatch.setattr(PersistentTransport, "_read_line", recorded_read)

    def _add(self, way: str, message: Any) -> None:
        method = message.get("method") if isinstance(message, dict) else None
        with self._lock:
            self._lines.append((way, method if isinstance(method, str) else None))

    def requests_sent(self) -> list[str]:
        """The method of every request the host wrote, in order."""
        with self._lock:
            return [method for way, method in self._lines if way == "sent" and method]

    def requests_sent_before_the_first(self, method: str) -> list[str] | None:
        """The method of every request the host wrote before the first line
        of ``method`` it received; ``None`` when it received none."""
        with self._lock:
            lines = list(self._lines)
        for index, line in enumerate(lines):
            if line == ("received", method):
                return [sent for way, sent in lines[:index] if way == "sent" and sent]
        return None


def _decision_requests(methods: list[str]) -> list[str]:
    return [method for method in methods if method.startswith(_DECISION_METHODS)]


class _AskingModel(_FakeModel):
    """The cross-mob rows' chat-completions fake, scripted to call
    ``ask_user`` once: at the first request that carries the trigger. It
    keeps the messages of every request as it was sent them."""

    def __init__(self) -> None:
        self._asked = False
        self._messages: list[list[dict[str, Any]]] = []
        super().__init__()

    def answer(self, body: dict) -> list[dict]:
        messages = body.get("messages", [])
        text = json.dumps(messages)
        with self._lock:
            self.requests.append(text)
            self._messages.append(messages if isinstance(messages, list) else [])
            if _ASK_NOW in text and not self._asked:
                self._asked = True
                delta: dict[str, Any] = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": _CALL_ID,
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

    def asked(self) -> bool:
        with self._lock:
            return self._asked

    def results_of_the_call(self) -> list[dict[str, Any]]:
        """The result of the model's ``ask_user`` call, parsed, once for
        each request that carried it: the content of the tool message that
        names the call's id."""
        with self._lock:
            requests = list(self._messages)
        found = []
        for messages in requests:
            for message in messages:
                if message.get("role") == "tool" and message.get("tool_call_id") == _CALL_ID:
                    result = _document_of(_text_of(message))
                    assert result is not None, f"the call's result is no JSON object: {message}"
                    found.append(result)
        return found

    def ended_requests(self, request_id: str) -> list[dict[str, Any]]:
        """The end of the request ``request_id`` as the model was sent it,
        parsed, once for each model request that carried it: an input whose
        first line is the label and whose rest is the result as one JSON
        document."""
        with self._lock:
            requests = list(self._messages)
        found = []
        for messages in requests:
            for message in messages:
                label, _, document = _text_of(message).partition("\n")
                if message.get("role") != "user" or label != _RESULT_LABEL:
                    continue
                result = _document_of(document)
                if result is not None and result.get("request_id") == request_id:
                    found.append(result)
        return found


def _document_of(text: str) -> dict[str, Any] | None:
    """``text`` as one JSON object, or ``None`` when it is not one."""
    try:
        document = json.loads(text)
    except ValueError:
        return None
    return document if isinstance(document, dict) else None


def _text_of(message: dict[str, Any]) -> str:
    """The text of a chat-completions message: its content, or its text
    parts joined."""
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(
            part.get("text", "") for part in content if isinstance(part, dict)
        )
    return ""


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
    tmp_path, monkeypatch
):
    assert os.path.isfile(_GATEWAY_BIN) and os.access(_GATEWAY_BIN, os.X_OK), (
        f"META_GATEWAY_BIN names no executable file: {_GATEWAY_BIN}"
    )
    # The gateway inherits the allow list and homes of this test only, and
    # every line between it and this host is recorded from its start.
    _only_the_allow_list_is_inherited(tmp_path, monkeypatch)
    wire = _Wire()
    wire.wrap_the_transport(monkeypatch)

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
        assert wire.requests_sent_before_the_first(_NOTICE_METHOD) is None, (
            "the gateway wrote a notice before anything was asked"
        )

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

        # Counted on the wire: before the notice line the host had written no
        # request that lists, reads or decides a decision or a question set.
        before_the_notice = wire.requests_sent_before_the_first(_NOTICE_METHOD)
        assert before_the_notice is not None, "the handler holds a notice the wire log does not"
        assert len(_decision_requests(before_the_notice)) == 0, (
            "the host wrote decision or question requests before the notice: "
            f"{_decision_requests(before_the_notice)} (all it wrote before: {before_the_notice})"
        )

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

        # The asking agent's model is sent the end of its request. The call
        # itself answered at once that the questions were asked, under a
        # request id; a later model request carries an ask_user_result input
        # for that request id. Both are parsed as the model was sent them.
        await _until(
            lambda: len(fake.ended_requests(noticed["decision_id"])) >= 1,
            "a model request that carries the end of the asked request",
        )
        of_the_call = fake.results_of_the_call()
        assert of_the_call, "no model request carried the result of the ask_user call"
        assert of_the_call[0]["outcome"] == "asked", of_the_call[0]
        assert of_the_call[0]["request_id"] == noticed["decision_id"], (of_the_call[0], noticed)
        ended = fake.ended_requests(of_the_call[0]["request_id"])[0]
        assert ended["outcome"] == "answered", ended
        assert ended["answers"] == [
            {"question_id": "day", "answer": {"kind": "choice", "choice_id": _CHOSEN}}
        ], ended

        # The answer wrote no further notice.
        await asyncio.sleep(_QUIET_SECONDS)
        assert len(notices) == 1, (
            "a further notice was observed by the handler after the answer, "
            f"watched for {_QUIET_SECONDS:g} s: {notices}"
        )

        # Over the whole row the host wrote the three decision and question
        # requests the row itself made, and no other.
        assert _decision_requests(wire.requests_sent()) == [
            "meta/gating/questions",
            "meta/gating/decide",
            "meta/gating/questions",
        ], wire.requests_sent()
    finally:
        if runtime is not None:
            await runtime.shutdown()
        fake.close()
