"""Typed retained-history reads are independent of Live mutation controls."""

import copy
import json
from pathlib import Path
from typing import get_args, get_type_hints
from unittest.mock import AsyncMock

import pytest

from meerkat import MeerkatClient, MobMemberLiveObservationsResult
from meerkat.errors import MeerkatError
from meerkat.mob import Mob
from meerkat.generated.types import (
    LiveObservationPage,
    LiveObservationRecord,
    LiveObservationSnapshot,
    LiveTranscriptObservation,
)


def test_history_codegen_preserves_named_transitive_record_graph():
    assert get_type_hints(MobMemberLiveObservationsResult)["page"] is LiveObservationPage
    assert get_type_hints(LiveObservationPage)["snapshot"] is LiveObservationSnapshot
    assert get_args(get_type_hints(LiveObservationPage)["records"]) == (LiveObservationRecord,)
    assert get_type_hints(LiveObservationRecord)["observation"] is LiveTranscriptObservation


def page_fixture():
    root = Path(__file__).resolve().parents[3]
    return json.loads(
        (root / "meerkat-contracts/tests/fixtures/live-observation-page-v1.json").read_text()
    )


@pytest.mark.asyncio
async def test_retained_history_client_and_mob_issue_only_read_request():
    client = MeerkatClient()
    client._request = AsyncMock(return_value=page_fixture())
    result = await client.mob_member_live_observations(
        "mob-history", "speaker", channel_id="channel-a", cursor="opaque", limit=7
    )
    assert isinstance(result, MobMemberLiveObservationsResult)
    assert result.page.records[0].observation.text == "heard\n\0not a Message"
    assert result.page.snapshot.coverage == "known_local_gap"
    assert result.provenance == "host_claimed"
    client._request.assert_awaited_once_with(
        "mob/member_live_observations",
        {
            "mob_id": "mob-history",
            "agent_identity": "speaker",
            "channel_id": "channel-a",
            "cursor": "opaque",
            "limit": 7,
        },
    )
    client._request.reset_mock()
    mob = Mob(client, "mob-history")
    await mob.member_live_observations("speaker")
    client._request.assert_awaited_once_with(
        "mob/member_live_observations",
        {"mob_id": "mob-history", "agent_identity": "speaker"},
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("limit", [0, -1, 257, 1.5, True])
async def test_invalid_limit_never_sends(limit):
    client = MeerkatClient()
    client._request = AsyncMock()
    with pytest.raises(ValueError):
        await client.mob_member_live_observations("mob-history", "speaker", limit=limit)
    client._request.assert_not_awaited()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "path,value",
    [
        (("page", "encoding_profile"), "v9"),
        (("page", "owner", "kind"), "other"),
        (("page", "snapshot", "coverage"), "complete"),
        (("page", "has_more"), "false"),
        (("page", "records", 0, "observation", "direction"), "other"),
        (("page", "records", 0, "observation", "text"), None),
    ],
)
async def test_generated_history_parser_fails_closed(path, value):
    raw = copy.deepcopy(page_fixture())
    parent = raw
    for part in path[:-1]:
        parent = parent[part]
    parent[path[-1]] = value
    client = MeerkatClient()
    client._request = AsyncMock(return_value=raw)
    with pytest.raises(MeerkatError):
        await client.mob_member_live_observations("mob-history", "speaker")
