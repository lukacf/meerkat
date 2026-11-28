"""``mob_standing``: where a mob stands, in the words of the wire.

``mob/standing`` answers ``busy``, ``finished``, ``closed``, ``stopped`` or
``unknown`` for a served mob, or the word for why a mob is not served. No
word says idle: a quiet running mob answers ``unknown`` with the evidence the
read did not have. The vocabularies may grow, so the wrapper keeps a word it
does not know as it was sent.
"""

from __future__ import annotations

from typing import Any

import pytest

from meerkat import MeerkatClient

# A quiet running mob, as a server sends it.
_QUIET: dict[str, Any] = {
    "mob_id": "quiet",
    "standing": "unknown",
    "causes": ["peer_delivery_not_traced", "handoffs_not_tracked"],
    "counts": {
        "roster_transitions": 0,
        "runs": 0,
        "parked_admission": 0,
        "foreground_owners": 0,
    },
}


def _answering_client(answer: dict[str, Any]) -> tuple[MeerkatClient, list[tuple[str, dict[str, Any]]]]:
    client = MeerkatClient()
    sent: list[tuple[str, dict[str, Any]]] = []

    async def fake_request(method, params):
        sent.append((method, params))
        return answer

    client._request = fake_request  # type: ignore[method-assign]
    return client, sent


@pytest.mark.asyncio
async def test_mob_standing_sends_the_mob_id_and_answers_the_standing() -> None:
    client, sent = _answering_client(_QUIET)

    answered = await client.mob_standing("quiet")

    assert answered == _QUIET
    assert answered["standing"] == "unknown"
    assert answered["causes"] == ["peer_delivery_not_traced", "handoffs_not_tracked"]
    assert sent == [("mob/standing", {"mob_id": "quiet"})]


@pytest.mark.asyncio
async def test_mob_standing_answers_a_mob_that_is_not_served_by_its_word_alone() -> None:
    dormant = {"mob_id": "asleep", "standing": "dormant"}
    client, _sent = _answering_client(dormant)

    answered = await client.mob_standing("asleep")

    assert answered == dormant
    assert "counts" not in answered
    assert "causes" not in answered


@pytest.mark.asyncio
async def test_mob_standing_keeps_a_word_this_sdk_does_not_know() -> None:
    # A later server may answer a word or a cause this SDK's types do not
    # list. The call does not fail, and nothing is rewritten to a known word.
    later = {
        "mob_id": "quiet",
        "standing": "a_word_from_a_later_server",
        "reason": "a_reason_from_a_later_server",
        "causes": ["a_cause_from_a_later_server"],
    }
    client, _sent = _answering_client(later)

    answered = await client.mob_standing("quiet")
    assert answered == later
    # Each unknown word is kept as it was sent. The SDK has no helper that
    # classifies a standing: nothing reads the word as idle or as finished,
    # and the reason and the cause are neither defaulted nor dropped.
    assert answered["standing"] == "a_word_from_a_later_server"
    assert answered["standing"] not in ("idle", "finished")
    assert answered["reason"] == "a_reason_from_a_later_server"
    assert answered["causes"] == ["a_cause_from_a_later_server"]


def test_the_generated_types_admit_a_word_this_sdk_does_not_know() -> None:
    # The three vocabularies may grow. The generated aliases say so: each is
    # its known words or any other string, so a type checker accepts an
    # answer that carries a word this SDK does not list.
    import typing

    from meerkat.generated import types as generated

    for alias, known in (
        (generated.WireMobStanding, "unknown"),
        (generated.WireMobStandingReason, "completed"),
        (generated.WireMobUnknownCause, "observation_timed_out"),
    ):
        alternatives = typing.get_args(alias)
        assert str in alternatives, alias
        listed = [
            word
            for alternative in alternatives
            if typing.get_origin(alternative) is typing.Literal
            for word in typing.get_args(alternative)
        ]
        assert known in listed, alias
