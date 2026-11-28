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

    assert await client.mob_standing("quiet") == later
